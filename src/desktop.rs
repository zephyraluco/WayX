use crate::window::{self as window_state, WindowState};
use crate::{
    backend::{Framebuffer, InputEvent, Rect, event},
    bridge::SharedBackend,
};
use anyhow::{Context, Result, anyhow};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use winit::raw_window_handle::HasWindowHandle;
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
use winit::raw_window_handle::RawWindowHandle;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

const SHADER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) index: u32) -> Output {
    var points=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));
    var out:Output; out.position=vec4<f32>(points[index],0.,1.);
    out.uv=vec2<f32>((points[index].x+1.)*0.5, (1.-points[index].y)*0.5); return out;
}
@fragment fn fs(in:Output)->@location(0) vec4<f32> { return textureSample(image,image_sampler,in.uv); }
"#;
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}
impl Gpu {
    async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<(Self, wgpu::Adapter)> {
        let adapter=instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference:wgpu::PowerPreference::LowPower, compatible_surface:surface, force_fallback_adapter:false,
        }).await.context("No usable GPU adapter (install a Vulkan/Metal/DX12 driver or Mesa software Vulkan)")?;
        tracing::info!(adapter=?adapter.get_info(),"render adapter");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("WayX GPU"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                },
                None,
            )
            .await?;
        Ok((Self { device, queue }, adapter))
    }
}
struct Image {
    texture: wgpu::Texture,
    binding: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    width: u32,
    height: u32,
    revision: u64,
}
impl Image {
    fn new(gpu: &Gpu, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("XLibre framebuffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let binding = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("WayX composition"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("WayX framebuffer blit"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs",
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
            });
        Self {
            texture,
            binding,
            pipeline,
            width,
            height,
            revision: 0,
        }
    }
    fn upload(&mut self, gpu: &Gpu, fb: &Framebuffer) {
        if self.revision == fb.revision {
            return;
        }
        let rect = if self.revision == 0 {
            Rect {
                x: 0,
                y: 0,
                width: fb.width,
                height: fb.height,
            }
        } else {
            fb.damage.unwrap_or(Rect {
                x: 0,
                y: 0,
                width: fb.width,
                height: fb.height,
            })
        };
        let offset = ((rect.y as usize * fb.width as usize) + rect.x as usize) * 4;
        gpu.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x,
                    y: rect.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &fb.pixels[offset..],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(fb.width * 4),
                rows_per_image: Some(fb.height),
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
        );
        self.revision = fb.revision;
    }
    fn overlay(&self, gpu: &Gpu, preedit: &crate::preedit::PreeditImage, spot: (i16, i16)) {
        let width = preedit.width.min(self.width);
        let height = preedit.height.min(self.height);
        let x = (spot.0.max(0) as u32).min(self.width - width);
        let y = (spot.1.max(0) as u32).min(self.height - height);
        gpu.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &preedit.pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(preedit.width * 4),
                rows_per_image: Some(preedit.height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
    fn draw(&self, gpu: &Gpu, target: &wgpu::TextureView) {
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("WayX frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit(Some(encoder.finish()));
    }
}
struct Native {
    id: u32,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    gpu: Gpu,
    config: wgpu::SurfaceConfiguration,
    image: Image,
    cursor: (f64, f64),
    applied_state: Option<WindowState>,
    remote_size: (u32, u32),
    resize_sync: crate::geometry::ResizeSync,
    issued_size: Option<(u32, u32)>,
    title: String,
    transient_owner: Option<u32>,
    composing: bool,
    pressed_keys: std::collections::HashSet<u32>,
    ime_allowed: bool,
    ime_spot: (i16, i16),
    ime_context: Option<(u32, u16, u16)>,
    preedit: Option<crate::preedit::PreeditImage>,
    retry_at: Option<Instant>,
    occluded: bool,
}
impl Native {
    async fn new(
        event_loop: &ActiveEventLoop,
        id: u32,
        title: &str,
        size: (u32, u32),
        state: &WindowState,
        resize_ack: u32,
        owner: Option<&Window>,
    ) -> Result<Self> {
        let (width, height) = size;
        let popup = state.flags & window_state::OVERRIDE_REDIRECT != 0;
        let mut attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::PhysicalSize::new(width, height))
            .with_decorations(!popup)
            .with_active(!popup);
        if state.flags & window_state::ROOTLESS != 0 {
            attributes =
                attributes.with_position(winit::dpi::PhysicalPosition::new(state.x, state.y));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            if popup {
                attributes = attributes
                    .with_override_redirect(true)
                    .with_x11_window_type(vec![WindowType::PopupMenu]);
            } else if state.transient_for != 0 {
                attributes = attributes.with_x11_window_type(vec![WindowType::Dialog]);
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(owner) = owner {
            use winit::platform::windows::WindowAttributesExtWindows;
            if let RawWindowHandle::Win32(handle) = owner.window_handle()?.as_raw() {
                attributes = attributes.with_owner_window(handle.hwnd.get());
            }
        }
        let window = Arc::new(event_loop.create_window(attributes)?);
        let _ = owner;
        let initial_target = state.constrain(width, height);
        let mut resize_sync =
            crate::geometry::ResizeSync::new((width, height), initial_target, resize_ack);
        // Establish the initial size before GPU setup: a real user resize can
        // arrive while the adapter is being initialized.
        let initial_size = window.inner_size();
        if (initial_size.width, initial_size.height) == initial_target {
            resize_sync.native_resize(initial_target);
        }
        window.set_ime_allowed(false);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all()),
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let (gpu, adapter) = Gpu::new(&instance, Some(&surface)).await?;
        anyhow::ensure!(
            width <= gpu.device.limits().max_texture_dimension_2d
                && height <= gpu.device.limits().max_texture_dimension_2d,
            "Framebuffer dimensions exceed GPU limits"
        );
        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("Surface incompatible with adapter")?;
        surface.configure(&gpu.device, &config);
        let image = Image::new(&gpu, width, height, config.format);
        Ok(Self {
            id,
            window,
            surface,
            gpu,
            config,
            image,
            cursor: (0., 0.),
            applied_state: None,
            remote_size: (width, height),
            resize_sync,
            issued_size: None,
            title: title.to_owned(),
            transient_owner: None,
            composing: false,
            pressed_keys: std::collections::HashSet::new(),
            ime_allowed: false,
            ime_spot: (0, 0),
            ime_context: None,
            preedit: None,
            retry_at: None,
            occluded: false,
        })
    }
    fn apply_state(
        &mut self,
        state: &WindowState,
        width: u32,
        height: u32,
        title: &str,
        resize_ack: u32,
    ) -> Result<()> {
        if self.title != title {
            self.window.set_title(title);
            self.title = title.to_owned();
        }
        self.remote_size = (width, height);
        self.resize_sync.server_update((width, height), resize_ack);
        if state.flags & window_state::ROOTLESS == 0 {
            self.sync_geometry(state);
            return Ok(());
        }
        let previous = self.applied_state;
        if previous.is_none_or(|old| old.x != state.x || old.y != state.y) {
            let position = winit::dpi::PhysicalPosition::new(state.x, state.y);
            if self.window.outer_position().ok() != Some(position) {
                self.window.set_outer_position(position);
            }
        }
        if previous != Some(*state) {
            self.window
                .set_decorations(state.flags & window_state::OVERRIDE_REDIRECT == 0);
            self.window.set_resizable(
                !(state.min_width != 0
                    && state.max_width == state.min_width
                    && state.min_height != 0
                    && state.max_height == state.min_height),
            );
            self.window
                .set_min_inner_size(Some(winit::dpi::PhysicalSize::new(
                    state.min_width.max(1),
                    state.min_height.max(1),
                )));
            let max = if state.max_width == 0 && state.max_height == 0 {
                None
            } else {
                Some(winit::dpi::PhysicalSize::new(
                    if state.max_width == 0 {
                        crate::backend::MAX_DIMENSION
                    } else {
                        state.max_width
                    },
                    if state.max_height == 0 {
                        crate::backend::MAX_DIMENSION
                    } else {
                        state.max_height
                    },
                ))
            };
            self.window.set_max_inner_size(max);
            self.window
                .set_resize_increments(Some(winit::dpi::PhysicalSize::new(
                    state.width_inc.max(1),
                    state.height_inc.max(1),
                )));
            self.window.set_window_level(
                if state.flags & (window_state::ABOVE | window_state::OVERRIDE_REDIRECT) != 0 {
                    winit::window::WindowLevel::AlwaysOnTop
                } else {
                    winit::window::WindowLevel::Normal
                },
            );
            if previous.is_none_or(|old| (old.flags ^ state.flags) & window_state::MINIMIZED != 0) {
                self.window
                    .set_minimized(state.flags & window_state::MINIMIZED != 0);
            }
            if previous.is_none_or(|old| (old.flags ^ state.flags) & window_state::MAXIMIZED != 0) {
                self.window
                    .set_maximized(state.flags & window_state::MAXIMIZED != 0);
            }
            if previous.is_none_or(|old| (old.flags ^ state.flags) & window_state::FULLSCREEN != 0)
            {
                self.window.set_fullscreen(
                    (state.flags & window_state::FULLSCREEN != 0)
                        .then_some(winit::window::Fullscreen::Borderless(None)),
                );
            }
            self.applied_state = Some(*state);
        }
        self.sync_geometry(state);
        Ok(())
    }
    fn sync_geometry(&mut self, state: &WindowState) {
        if state.flags & window_state::MINIMIZED != 0 || self.window.is_minimized().unwrap_or(false)
        {
            return;
        }
        if let Some((width, height)) = self.resize_sync.target() {
            let size = winit::dpi::PhysicalSize::new(width, height);
            if self.window.inner_size() == size {
                self.resize_sync.native_resize((width, height));
                self.issued_size = None;
            } else if self.issued_size != Some((width, height)) {
                self.issued_size = Some((width, height));
                if let Some(actual) = self.window.request_inner_size(size) {
                    let constrained = state.constrain(actual.width, actual.height);
                    self.resize_sync.native_resize(constrained);
                }
            }
        } else {
            self.issued_size = None;
        }
    }
    fn x11_id(&self) -> Result<Option<u32>> {
        Ok(match self.window.window_handle()?.as_raw() {
            #[cfg(all(unix, not(target_os = "macos")))]
            RawWindowHandle::Xlib(handle) => Some(u32::try_from(handle.window)?),
            #[cfg(all(unix, not(target_os = "macos")))]
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            _ => None,
        })
    }
    fn sync_transient(&mut self, owner: Option<u32>) -> Result<()> {
        if self.transient_owner == owner {
            return Ok(());
        }
        if let Some(id) = self.x11_id()? {
            use x11rb::{
                connection::Connection,
                protocol::xproto::{AtomEnum, ConnectionExt, PropMode},
                wrapper::ConnectionExt as _,
            };
            let (connection, _) = x11rb::connect(None)?;
            if let Some(owner) = owner {
                connection
                    .change_property32(
                        PropMode::REPLACE,
                        id,
                        AtomEnum::WM_TRANSIENT_FOR,
                        AtomEnum::WINDOW,
                        &[owner],
                    )?
                    .check()?;
            } else {
                connection
                    .delete_property(id, AtomEnum::WM_TRANSIENT_FOR.into())?
                    .check()?;
            }
            connection.flush()?;
        }
        self.transient_owner = owner;
        Ok(())
    }
    fn resize_surface(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.gpu.device, &self.config);
    }
    fn render(&mut self, fb: &Framebuffer) -> Result<bool> {
        if self.window.inner_size().width == 0 || self.window.inner_size().height == 0 {
            return Ok(false);
        }
        if fb.width != self.image.width || fb.height != self.image.height {
            anyhow::ensure!(
                fb.width <= self.gpu.device.limits().max_texture_dimension_2d
                    && fb.height <= self.gpu.device.limits().max_texture_dimension_2d,
                "Framebuffer dimensions exceed GPU limits"
            );
            self.image = Image::new(&self.gpu, fb.width, fb.height, self.config.format);
        }
        self.image.upload(&self.gpu, fb);
        if let Some(preedit) = &self.preedit {
            self.image.overlay(&self.gpu, preedit, self.ime_spot);
        }
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        self.image
            .draw(&self.gpu, &frame.texture.create_view(&Default::default()));
        frame.present();
        Ok(true)
    }
}
// A minimized native window can report 0x0. Forward its state without replacing
// the X11 application's framebuffer or its restore geometry with a 1x1 image.
fn resize_input(id: u32, state: &WindowState, width: u32, height: u32, flags: u32) -> InputEvent {
    if width == 0 || height == 0 {
        return InputEvent {
            kind: event::STATE,
            window: id,
            flags,
            ..Default::default()
        };
    }
    let (width, height) = if state.flags & window_state::ROOTLESS != 0 {
        state.constrain(width, height)
    } else {
        (width, height)
    };
    InputEvent {
        kind: event::RESIZE,
        window: id,
        flags,
        x: f64::from(width),
        y: f64::from(height),
        ..Default::default()
    }
}
struct App {
    backend: SharedBackend,
    wake_pending: Arc<AtomicBool>,
    ui_wake: crate::notify::UiWake,
    selection_watch: Option<crate::selection::ReadinessWatch>,
    native: HashMap<WindowId, Native>,
    clipboard: Option<arboard::Clipboard>,
    preedit_painter: Option<crate::preedit::Painter>,
    clipboard_revision: u64,
    last_clipboard: Instant,
    error: Option<anyhow::Error>,
    demo: bool,
    rootless: bool,
    frame_limit: Option<u32>,
    presented_frames: u32,
    selection: Option<crate::selection::SelectionBridge>,
    selection_config: Option<(u16, std::path::PathBuf)>,
    worker: Option<std::thread::JoinHandle<Result<i32>>>,
}
impl App {
    fn input(&mut self, input: InputEvent) {
        let result = self.backend.lock().unwrap().push_input(input);
        if let Err(error) = result {
            tracing::error!(%error,"input queue rejected event");
            self.error = Some(error.into());
        }
    }
    fn finish_worker(&mut self, event_loop: &ActiveEventLoop) -> Result<bool> {
        if !self
            .backend
            .lock()
            .map_err(|_| anyhow!("backend mutex poisoned"))?
            .worker_exited
            && !self
                .worker
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return Ok(false);
        }
        let Some(worker) = self.worker.take() else {
            event_loop.exit();
            return Ok(true);
        };
        let code = worker
            .join()
            .map_err(|_| anyhow!("XLibre worker panicked"))??;
        anyhow::ensure!(code == 0, "XLibre exited with status {code}");
        event_loop.exit();
        Ok(true)
    }
    fn sync(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if let Err(error) = self.sync_inner(event_loop) {
            // Any internal X11 operation can race normal server teardown, not just tick().
            let deadline = Instant::now() + Duration::from_millis(100);
            while self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            if self.worker.is_none() || !self.finish_worker(event_loop)? {
                return Err(error);
            }
        }
        Ok(())
    }
    fn sync_inner(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if let Some(watch) = &self.selection_watch {
            watch.pause()?;
        }
        if self.finish_worker(event_loop)? {
            return Ok(());
        }
        if self.selection.is_none()
            && self.backend.lock().unwrap().windows.contains_key(&1)
            && let Some((display, auth)) = self.selection_config.take()
        {
            let mut selection = crate::selection::SelectionBridge::connect_local(
                display,
                &auth,
                self.backend.clone(),
            )?;
            selection.enable_ime()?;
            if self.rootless {
                selection.enable_window_management()?;
            }
            self.selection_watch = Some(selection.watch(self.ui_wake.clone())?);
            self.selection = Some(selection);
            tracing::debug!("internal X11 bridge ready");
        }
        if let Some(selection) = &mut self.selection {
            selection.tick()?;
        }
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| anyhow!("backend mutex poisoned"))?;
        self.native
            .retain(|_, native| backend.windows.get(&native.id).is_some_and(|w| w.mapped));
        let mut resized = Vec::new();
        for window in backend.windows.values() {
            if !window.mapped {
                continue;
            }
            if !self.native.values().any(|native| native.id == window.id) {
                let owner = self
                    .native
                    .values()
                    .find(|native| native.id == window.state.transient_for)
                    .map(|native| native.window.as_ref());
                let native = pollster::block_on(Native::new(
                    event_loop,
                    window.id,
                    &window.title,
                    (window.framebuffer.width, window.framebuffer.height),
                    &window.state,
                    window.resize_ack,
                    owner,
                ))?;
                self.native.insert(native.window.id(), native);
            }
            if let Some(native) = self
                .native
                .values_mut()
                .find(|native| native.id == window.id)
            {
                native.apply_state(
                    &window.state,
                    window.framebuffer.width,
                    window.framebuffer.height,
                    &window.title,
                    window.resize_ack,
                )?;
                if let Some((sequence, (width, height))) = native.resize_sync.take_input() {
                    let mut input =
                        resize_input(native.id, &window.state, width, height, window.state.flags);
                    input.detail = sequence;
                    resized.push(input);
                }
            }
        }
        for input in resized {
            backend.push_input(input)?;
        }
        let owners = self
            .native
            .values()
            .map(|native| Ok((native.id, native.x11_id()?)))
            .collect::<Result<HashMap<_, _>>>()?;
        for native in self.native.values_mut() {
            if let Some(window) = backend.windows.get(&native.id) {
                native
                    .sync_transient(owners.get(&window.state.transient_for).copied().flatten())?;
            }
            if let Some(window) = backend.windows.get(&native.id)
                && (native.image.revision != window.framebuffer.revision
                    || native.image.width != window.framebuffer.width
                    || native.image.height != window.framebuffer.height)
                && !native.occluded
                && !native.window.is_minimized().unwrap_or(false)
                && native.retry_at.is_none()
            {
                native.window.request_redraw();
            }
        }
        if let Some(id) = backend.requested_focus.take()
            && let Some(native) = self.native.values().find(|native| native.id == id)
            && !native.window.has_focus()
        {
            native.window.focus_window();
        }
        if self.last_clipboard.elapsed() >= Duration::from_millis(250) {
            if let Some(clipboard) = &mut self.clipboard {
                if self.clipboard_revision != backend.clipboard_revision {
                    clipboard
                        .set_text(backend.clipboard.clone())
                        .context("Write native clipboard")?;
                    self.clipboard_revision = backend.clipboard_revision;
                } else if let Ok(text) = clipboard.get_text()
                    && text != backend.clipboard
                {
                    backend.set_clipboard(&text)?;
                    backend.push_input(InputEvent {
                        kind: event::CLIPBOARD,
                        ..Default::default()
                    })?;
                    self.clipboard_revision = backend.clipboard_revision;
                }
            }
            self.last_clipboard = Instant::now();
        }
        // X11 replies require the DDX thread to acquire the backend mutex.
        drop(backend);
        if let Some(ime) = self
            .selection
            .as_ref()
            .and_then(|selection| selection.ime.as_ref())
        {
            for native in self.native.values_mut() {
                let cursor = if native.window.has_focus() {
                    ime.cursor(native.id)?
                } else {
                    None
                };
                let allowed = cursor.is_some();
                let context = if allowed { ime.focus_token() } else { None };
                if context != native.ime_context && native.ime_allowed {
                    native.window.set_ime_allowed(false);
                    native.ime_allowed = false;
                    native.composing = false;
                    native.preedit = None;
                    native.image.revision = 0;
                    native.window.request_redraw();
                }
                native.ime_context = context;
                if allowed != native.ime_allowed {
                    native.window.set_ime_allowed(allowed);
                    native.ime_allowed = allowed;
                    native.composing = false;
                    native.preedit = None;
                    native.image.revision = 0;
                    native.window.request_redraw();
                }
                if let Some((x, y)) = cursor {
                    if native.ime_spot != (x, y) {
                        native.image.revision = 0;
                        native.window.request_redraw();
                        native.ime_spot = (x, y);
                    }
                    let size = native.window.inner_size();
                    let sx = size.width as f64 / native.remote_size.0.max(1) as f64;
                    let sy = size.height as f64 / native.remote_size.1.max(1) as f64;
                    native.window.set_ime_cursor_area(
                        winit::dpi::PhysicalPosition::new(x as f64 * sx, y as f64 * sy),
                        winit::dpi::PhysicalSize::new(sx.max(1.), (16. * sy).max(1.)),
                    );
                }
            }
        }
        for native in self.native.values() {
            if native.image.revision == 0
                && !native.occluded
                && native.retry_at.is_none()
                && !native.window.is_minimized().unwrap_or(false)
            {
                native.window.request_redraw();
            }
        }
        if let Some(selection) = &mut self.selection
            && selection.queue_pending_event()?
        {
            (self.ui_wake)();
        } else if let Some(watch) = &self.selection_watch {
            watch.arm()?;
        }
        Ok(())
    }
}
impl ApplicationHandler<()> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, (): ()) {
        // Coalesce the whole event batch; about_to_wait performs one synchronization.
        self.wake_pending.store(false, Ordering::Release);
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.clipboard = match arboard::Clipboard::new() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(%e,"native clipboard unavailable");
                None
            }
        };
        if let Err(error) = self.sync(event_loop) {
            self.error = Some(error);
            event_loop.exit();
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.error.is_some() {
            event_loop.exit();
            return;
        }
        if let Err(error) = self.sync(event_loop) {
            self.error = Some(error);
            event_loop.exit();
        }
        let mut deadline = self.last_clipboard + Duration::from_millis(250);
        for native in self.native.values_mut() {
            if let Some(retry) = native.retry_at {
                if retry <= Instant::now() {
                    native.retry_at = None;
                    native.window.request_redraw();
                } else {
                    deadline = deadline.min(retry);
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, ev: WindowEvent) {
        let Some(native) = self.native.get_mut(&window_id) else {
            return;
        };
        let id = native.id;
        let mut input = InputEvent {
            window: id,
            ..Default::default()
        };
        match ev {
            WindowEvent::CloseRequested => {
                input.kind = event::CLOSE;
                if self.demo {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                let actual = native.window.inner_size();
                if size.width != 0 && size.height != 0 && actual != size {
                    return;
                }
                native.resize_surface(actual.width, actual.height);
                native.window.request_redraw();
                if self.demo {
                    return;
                }
                let flags = (u32::from(native.window.is_maximized()) * window_state::MAXIMIZED)
                    | (u32::from(native.window.is_minimized().unwrap_or(false))
                        * window_state::MINIMIZED)
                    | (u32::from(native.window.fullscreen().is_some()) * window_state::FULLSCREEN);
                let state = native.applied_state.unwrap_or_default();
                input = resize_input(id, &state, size.width, size.height, flags);
                if input.kind == event::RESIZE {
                    native
                        .resize_sync
                        .native_resize((input.x as u32, input.y as u32));
                    if let Some((sequence, (width, height))) = native.resize_sync.take_input() {
                        native.issued_size = None;
                        input.detail = sequence;
                        input.x = width as f64;
                        input.y = height as f64;
                    } else if (state.flags ^ flags)
                        & (window_state::MAXIMIZED
                            | window_state::MINIMIZED
                            | window_state::FULLSCREEN)
                        != 0
                    {
                        input.kind = event::STATE;
                    } else {
                        return;
                    }
                }
            }
            WindowEvent::Moved(position) => {
                input.kind = event::MOVE;
                input.x = position.x as f64;
                input.y = position.y as f64;
            }
            WindowEvent::Occluded(occluded) => {
                native.occluded = occluded;
                if !occluded {
                    native.window.request_redraw();
                }
                return;
            }
            WindowEvent::RedrawRequested => {
                if native.occluded || native.window.is_minimized().unwrap_or(false) {
                    return;
                }
                let mut backend = self.backend.lock().unwrap();
                if let Some(window) = backend.windows.get_mut(&id) {
                    match native.render(&window.framebuffer) {
                        Err(error) => {
                            self.error = Some(error);
                            event_loop.exit();
                        }
                        Ok(true) => {
                            native.retry_at = None;
                            tracing::debug!(window = id, "frame presented");
                            if self.frame_limit.is_some() {
                                native.window.request_redraw();
                            }
                            self.presented_frames += 1;
                            if self
                                .frame_limit
                                .is_some_and(|limit| self.presented_frames >= limit)
                            {
                                event_loop.exit();
                            }
                        }
                        Ok(false) => {
                            if native.window.inner_size().width != 0
                                && native.window.inner_size().height != 0
                            {
                                native.retry_at = Some(Instant::now() + Duration::from_millis(100));
                            }
                        }
                    }
                    window.framebuffer.damage = None;
                }
                return;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let size = native.window.inner_size();
                native.cursor = (
                    position.x * native.image.width as f64 / size.width.max(1) as f64,
                    position.y * native.image.height as f64 / size.height.max(1) as f64,
                );
                input.kind = event::MOTION;
                input.x = native.cursor.0;
                input.y = native.cursor.1;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                input.kind = event::BUTTON;
                input.flags = u32::from(state == ElementState::Pressed);
                input.detail = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                    MouseButton::Back => 8,
                    MouseButton::Forward => 9,
                    MouseButton::Other(x) => x as u32,
                };
                input.x = native.cursor.0;
                input.y = native.cursor.1;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                input.kind = event::SCROLL;
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
                    MouseScrollDelta::PixelDelta(p) => (p.x / 40., p.y / 40.),
                };
                input.x = x;
                input.y = y;
            }
            WindowEvent::Focused(focus) => {
                if !focus {
                    native.composing = false;
                    native.preedit = None;
                    native.image.revision = 0;
                    native.window.request_redraw();
                    native.pressed_keys.clear();
                    if let Some(ime) = self
                        .selection
                        .as_mut()
                        .and_then(|selection| selection.ime.as_mut())
                        && let Err(error) = ime.cancel(id)
                    {
                        tracing::warn!(%error, "IME cancellation failed");
                    }
                }
                input.kind = event::FOCUS;
                input.flags = u32::from(focus);
            }
            WindowEvent::KeyboardInput {
                event: ke,
                is_synthetic: false,
                ..
            } => {
                if native.composing {
                    return;
                }
                let PhysicalKey::Code(key) = ke.physical_key else {
                    return;
                };
                let Some(code) = hid_usage(key) else {
                    tracing::debug!(?key, "unmapped physical key");
                    return;
                };
                input.kind = event::KEY;
                input.detail = code;
                input.flags = u32::from(ke.state == ElementState::Pressed);
                if ke.repeat {
                    return;
                }
                if ke.state == ElementState::Pressed {
                    native.pressed_keys.insert(code);
                } else {
                    native.pressed_keys.remove(&code);
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                native.composing = false;
                native.preedit = None;
                native.image.revision = 0;
                native.window.request_redraw();
                if let Some(ime) = self
                    .selection
                    .as_mut()
                    .and_then(|selection| selection.ime.as_mut())
                    && let Err(error) = ime.commit(id, &text)
                {
                    tracing::warn!(%error, "IME commit rejected");
                }
                return;
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                let starting = !native.composing && !text.is_empty();
                native.composing = !text.is_empty();
                // A platform can deliver the first key-down before Preedit.
                // Balance those presses before suppressing composition keys.
                let release: Vec<_> = if starting {
                    native.pressed_keys.drain().collect()
                } else {
                    Vec::new()
                };
                native.preedit = None;
                native.image.revision = 0;
                native.window.request_redraw();
                if let Some(ime) = self
                    .selection
                    .as_mut()
                    .and_then(|selection| selection.ime.as_mut())
                {
                    match ime.preedit_at(id, &text, cursor) {
                        Ok(()) if !text.is_empty() && !ime.inline_preedit() => {
                            let painter = self
                                .preedit_painter
                                .get_or_insert_with(crate::preedit::Painter::new);
                            native.preedit =
                                Some(painter.paint(&text, cursor, native.remote_size.0));
                        }
                        Ok(()) => {}
                        Err(error) => tracing::warn!(%error, "IME preedit rejected"),
                    }
                }
                for code in release {
                    self.input(InputEvent {
                        kind: event::KEY,
                        window: id,
                        detail: code,
                        ..Default::default()
                    });
                }
                return;
            }
            WindowEvent::Ime(Ime::Disabled) => {
                native.composing = false;
                native.preedit = None;
                native.image.revision = 0;
                native.window.request_redraw();
                if let Some(ime) = self
                    .selection
                    .as_mut()
                    .and_then(|selection| selection.ime.as_mut())
                    && let Err(error) = ime.cancel(id)
                {
                    tracing::warn!(%error, "IME cancellation failed");
                }
                return;
            }
            _ => return,
        }
        self.input(input);
        if self.demo {
            let mut backend = self.backend.lock().unwrap();
            // The demo has no X server to drain its input queue.
            while backend.poll_input().is_some() {}
        }
    }
}
/// Stable USB HID keyboard page values, independent of winit enum representation.
pub fn hid_usage(key: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match key {
        KeyA => 4,
        KeyB => 5,
        KeyC => 6,
        KeyD => 7,
        KeyE => 8,
        KeyF => 9,
        KeyG => 10,
        KeyH => 11,
        KeyI => 12,
        KeyJ => 13,
        KeyK => 14,
        KeyL => 15,
        KeyM => 16,
        KeyN => 17,
        KeyO => 18,
        KeyP => 19,
        KeyQ => 20,
        KeyR => 21,
        KeyS => 22,
        KeyT => 23,
        KeyU => 24,
        KeyV => 25,
        KeyW => 26,
        KeyX => 27,
        KeyY => 28,
        KeyZ => 29,
        Digit1 => 30,
        Digit2 => 31,
        Digit3 => 32,
        Digit4 => 33,
        Digit5 => 34,
        Digit6 => 35,
        Digit7 => 36,
        Digit8 => 37,
        Digit9 => 38,
        Digit0 => 39,
        Enter => 40,
        Escape => 41,
        Backspace => 42,
        Tab => 43,
        Space => 44,
        Minus => 45,
        Equal => 46,
        BracketLeft => 47,
        BracketRight => 48,
        Backslash => 49,
        Semicolon => 51,
        Quote => 52,
        Backquote => 53,
        Comma => 54,
        Period => 55,
        Slash => 56,
        CapsLock => 57,
        F1 => 58,
        F2 => 59,
        F3 => 60,
        F4 => 61,
        F5 => 62,
        F6 => 63,
        F7 => 64,
        F8 => 65,
        F9 => 66,
        F10 => 67,
        F11 => 68,
        F12 => 69,
        PrintScreen => 70,
        ScrollLock => 71,
        Pause => 72,
        Insert => 73,
        Home => 74,
        PageUp => 75,
        Delete => 76,
        End => 77,
        PageDown => 78,
        ArrowRight => 79,
        ArrowLeft => 80,
        ArrowDown => 81,
        ArrowUp => 82,
        NumLock => 83,
        NumpadDivide => 84,
        NumpadMultiply => 85,
        NumpadSubtract => 86,
        NumpadAdd => 87,
        NumpadEnter => 88,
        Numpad1 => 89,
        Numpad2 => 90,
        Numpad3 => 91,
        Numpad4 => 92,
        Numpad5 => 93,
        Numpad6 => 94,
        Numpad7 => 95,
        Numpad8 => 96,
        Numpad9 => 97,
        Numpad0 => 98,
        NumpadDecimal => 99,
        ControlLeft => 224,
        ShiftLeft => 225,
        AltLeft => 226,
        SuperLeft => 227,
        ControlRight => 228,
        ShiftRight => 229,
        AltRight => 230,
        SuperRight => 231,
        _ => return None,
    })
}
pub fn run(
    backend: SharedBackend,
    demo: bool,
    rootless: bool,
    worker: Option<std::thread::JoinHandle<Result<i32>>>,
    selection_config: Option<(u16, std::path::PathBuf)>,
    frame_limit: Option<u32>,
) -> Result<()> {
    let event_loop = EventLoop::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let wake_pending = Arc::new(AtomicBool::new(false));
    let pending = wake_pending.clone();
    let ui_wake: crate::notify::UiWake = Arc::new(move || {
        if !pending.swap(true, Ordering::AcqRel) && proxy.send_event(()).is_err() {
            pending.store(false, Ordering::Release);
        }
    });
    backend
        .lock()
        .map_err(|_| anyhow!("backend mutex poisoned"))?
        .ui_wake = Some(ui_wake.clone());
    let mut app = App {
        wake_pending,
        ui_wake,
        selection_watch: None,
        backend,
        native: HashMap::new(),
        clipboard: None,
        preedit_painter: None,
        clipboard_revision: 0,
        last_clipboard: Instant::now(),
        error: None,
        demo,
        rootless,
        frame_limit,
        presented_frames: 0,
        selection: None,
        selection_config,
        worker,
    };
    let event_result = event_loop.run_app(&mut app);
    // Close the internal selection client, then request orderly server teardown.
    app.selection_watch.take();
    app.selection.take();
    if let Some(worker) = app.worker.take() {
        if !worker.is_finished() {
            let mut backend = app.backend.lock().unwrap();
            // Backpressure must not prevent shutdown after a UI/driver failure.
            while backend.poll_input().is_some() {}
            backend.push_input(InputEvent {
                kind: event::SHUTDOWN,
                ..Default::default()
            })?;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        anyhow::ensure!(
            worker.is_finished(),
            "XLibre worker did not stop within five seconds"
        );
        let code = worker
            .join()
            .map_err(|_| anyhow!("XLibre worker panicked"))??;
        anyhow::ensure!(code == 0, "XLibre worker exited with status {code}");
    }
    event_result?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
/// A real offscreen shader/texture/readback check; no native window is needed.
pub async fn gpu_smoke() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all()),
        ..Default::default()
    });
    let (gpu, _) = Gpu::new(&instance, None).await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut image = Image::new(&gpu, 2, 2, format);
    let mut fb = Framebuffer::new(2, 2)?;
    fb.upload(
        Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        },
        8,
        &[0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0],
    )?;
    image.upload(&gpu, &fb);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke target"),
        size: wgpu::Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    // Verify the IME overlay's BGRA color and edge clipping, then erase it
    // without changing the X11 framebuffer or relying on new remote damage.
    for overlay in [false, true, false] {
        image.revision = 0;
        image.upload(&gpu, &fb);
        if overlay {
            image.overlay(
                &gpu,
                &crate::preedit::PreeditImage {
                    width: 1,
                    height: 1,
                    pixels: vec![0, 255, 255, 255],
                },
                (i16::MAX, i16::MAX),
            );
        }
        image.draw(&gpu, &target.create_view(&Default::default()));
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 512,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(2),
                },
            },
            wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;
        let bytes = buffer.slice(..).get_mapped_range();
        anyhow::ensure!(
            bytes[..8] == [255, 0, 0, 255, 0, 255, 0, 255]
                && bytes[256..260] == [0, 0, 255, 255]
                && bytes[260..264]
                    == if overlay {
                        [255, 255, 0, 255]
                    } else {
                        [255, 255, 255, 255]
                    },
            "GPU framebuffer color/orientation mismatch"
        );
    }
    anyhow::ensure!(
        fb.pixels[12..16] == [255, 255, 255, 255],
        "IME modified remote framebuffer"
    );
    tracing::info!(
        "GPU texture upload, IME overlay/restore, shader composition and readback passed"
    );
    Ok(())
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    #[test]
    fn minimizing_should_report_state_without_resizing_the_x11_framebuffer() {
        let state = WindowState {
            flags: window_state::ROOTLESS,
            min_width: 80,
            min_height: 40,
            ..Default::default()
        };
        let minimized = resize_input(42, &state, 0, 0, window_state::MINIMIZED);
        assert_eq!(
            (
                minimized.kind,
                minimized.window,
                minimized.flags,
                minimized.x,
                minimized.y
            ),
            (event::STATE, 42, window_state::MINIMIZED, 0.0, 0.0)
        );
        let restored = resize_input(42, &state, 640, 480, 0);
        assert_eq!(
            (restored.kind, restored.x, restored.y),
            (event::RESIZE, 640.0, 480.0)
        );
    }
}

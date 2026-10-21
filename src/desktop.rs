use crate::{
    backend::{Framebuffer, InputEvent, Rect, event},
    bridge::SharedBackend,
};
use anyhow::{Context, Result, anyhow};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
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
}
impl Native {
    async fn new(
        event_loop: &ActiveEventLoop,
        id: u32,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title)
                    .with_inner_size(winit::dpi::PhysicalSize::new(width, height)),
            )?,
        );
        window.set_ime_allowed(true);
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
        })
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
struct App {
    backend: SharedBackend,
    native: HashMap<WindowId, Native>,
    clipboard: Option<arboard::Clipboard>,
    clipboard_revision: u64,
    last_clipboard: Instant,
    error: Option<anyhow::Error>,
    demo: bool,
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
    fn sync(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if let Some(worker) = self.worker.as_ref()
            && worker.is_finished()
        {
            let code = self
                .worker
                .take()
                .unwrap()
                .join()
                .map_err(|_| anyhow!("XLibre worker panicked"))??;
            if code != 0 {
                return Err(anyhow!("XLibre exited with status {code}"));
            }
            event_loop.exit();
            return Ok(());
        }
        if self.selection.is_none()
            && self.backend.lock().unwrap().windows.contains_key(&1)
            && let Some((display, auth)) = self.selection_config.take()
        {
            self.selection = Some(crate::selection::SelectionBridge::connect_local(
                display,
                &auth,
                self.backend.clone(),
            )?);
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
        for window in backend.windows.values() {
            if !window.mapped {
                continue;
            }
            if let Some(native) = self.native.values().find(|n| n.id == window.id) {
                native.window.set_title(&window.title);
            } else {
                let native = pollster::block_on(Native::new(
                    event_loop,
                    window.id,
                    &window.title,
                    window.framebuffer.width,
                    window.framebuffer.height,
                ))?;
                self.native.insert(native.window.id(), native);
            }
        }
        for native in self.native.values() {
            native.window.request_redraw();
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
        Ok(())
    }
}
impl ApplicationHandler for App {
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
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(16),
        ));
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
                native.resize_surface(size.width, size.height);
                input.kind = event::RESIZE;
                input.x = size.width as f64;
                input.y = size.height as f64;
            }
            WindowEvent::RedrawRequested => {
                let mut backend = self.backend.lock().unwrap();
                if let Some(window) = backend.windows.get_mut(&id) {
                    match native.render(&window.framebuffer) {
                        Err(error) => {
                            self.error = Some(error);
                            event_loop.exit();
                        }
                        Ok(true) => {
                            self.presented_frames += 1;
                            if self
                                .frame_limit
                                .is_some_and(|limit| self.presented_frames >= limit)
                            {
                                event_loop.exit();
                            }
                        }
                        Ok(false) => {}
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
                input.kind = event::FOCUS;
                input.flags = u32::from(focus);
            }
            WindowEvent::KeyboardInput {
                event: ke,
                is_synthetic: false,
                ..
            } => {
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
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                // Split on Unicode scalar boundaries; every text event is valid UTF-8.
                for ch in text.chars() {
                    let mut part = InputEvent {
                        kind: event::TEXT,
                        window: id,
                        ..Default::default()
                    };
                    let mut bytes = [0; 4];
                    let text = ch.encode_utf8(&mut bytes);
                    part.detail = text.len() as u32;
                    part.text[..text.len()].copy_from_slice(text.as_bytes());
                    self.input(part);
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
    worker: Option<std::thread::JoinHandle<Result<i32>>>,
    selection_config: Option<(u16, std::path::PathBuf)>,
    frame_limit: Option<u32>,
) -> Result<()> {
    let mut app = App {
        backend,
        native: HashMap::new(),
        clipboard: None,
        clipboard_revision: 0,
        last_clipboard: Instant::now(),
        error: None,
        demo,
        frame_limit,
        presented_frames: 0,
        selection: None,
        selection_config,
        worker,
    };
    let event_result = EventLoop::new().and_then(|event_loop| event_loop.run_app(&mut app));
    // Close the internal selection client, then request orderly server teardown.
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
            && bytes[256..264] == [0, 0, 255, 255, 255, 255, 255, 255],
        "GPU framebuffer color/orientation mismatch"
    );
    tracing::info!("GPU texture upload, shader composition and readback passed");
    Ok(())
}

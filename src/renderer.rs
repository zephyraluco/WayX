//! Shared GPU lifetime, bounded texture uploads and framebuffer composition.
use crate::backend::{Framebuffer, Rect};
use anyhow::{Context, Result, anyhow};
use std::{collections::HashMap, sync::Arc};

const SHADER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) index: u32) -> Output {
    var points=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));
    var out:Output; out.position=vec4<f32>(points[index],0.,1.);
    out.uv=vec2<f32>((points[index].x+1.)*0.5, (1.-points[index].y)*0.5); return out;
}
@fragment fn fs(in:Output)->@location(0) vec4<f32> { return vec4<f32>(textureSample(image,image_sampler,in.uv).rgb, 1.); }
"#;
pub(crate) struct Gpu {
    pipelines: std::sync::Mutex<HashMap<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>>,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    binding_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pub(crate) queue: wgpu::Queue,
    pub(crate) device: wgpu::Device,
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("WayX composition"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        Ok((
            Self {
                device,
                queue,
                binding_layout: layout,
                sampler,
                shader,
                pipeline_layout,
                pipelines: std::sync::Mutex::new(HashMap::new()),
            },
            adapter,
        ))
    }
    fn pipeline(&self, format: wgpu::TextureFormat) -> Result<Arc<wgpu::RenderPipeline>> {
        let mut pipelines = self
            .pipelines
            .lock()
            .map_err(|_| anyhow!("GPU pipeline cache poisoned"))?;
        if let Some(pipeline) = pipelines.get(&format) {
            return Ok(pipeline.clone());
        }
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("WayX framebuffer blit"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: "vs",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
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
        let pipeline = Arc::new(pipeline);
        pipelines.insert(format, pipeline.clone());
        Ok(pipeline)
    }
}
/// One instance, adapter and device for every native window in this serve.
pub(crate) struct SharedGpu {
    gpu: Gpu,
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) instance: wgpu::Instance,
    lost: Arc<std::sync::Mutex<Option<String>>>,
}
impl std::ops::Deref for SharedGpu {
    type Target = Gpu;
    fn deref(&self) -> &Gpu {
        &self.gpu
    }
}
impl SharedGpu {
    pub(crate) async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        wake: &crate::notify::UiWake,
    ) -> Result<(wgpu::Surface<'static>, Arc<Self>)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all()),
            ..Default::default()
        });
        let surface = instance.create_surface(target)?;
        let (gpu, adapter) = Gpu::new(&instance, Some(&surface)).await?;
        let lost = Arc::new(std::sync::Mutex::new(None));
        let failure = lost.clone();
        let notify = wake.clone();
        gpu.device.on_uncaptured_error(Box::new(move |error| {
            if let Ok(mut failure) = failure.lock()
                && failure.is_none()
            {
                *failure = Some(format!("uncaptured GPU error: {error}"));
            }
            notify();
        }));
        let failure = lost.clone();
        let wake = wake.clone();
        gpu.device.set_device_lost_callback(move |reason, message| {
            if let Ok(mut failure) = failure.lock()
                && failure.is_none()
            {
                *failure = Some(format!("device lost ({reason:?}): {message}"));
            }
            wake();
        });
        let gpu = Arc::new(SharedGpu {
            gpu,
            adapter,
            instance,
            lost,
        });
        tracing::info!("shared GPU device initialized");
        Ok((surface, gpu))
    }

    pub(crate) fn check(&self) -> Result<()> {
        let lost = self
            .lost
            .lock()
            .map_err(|_| anyhow!("GPU device-loss state poisoned"))?;
        if let Some(reason) = lost.as_ref() {
            anyhow::bail!("Shared GPU failure: {reason}");
        }
        Ok(())
    }
    pub(crate) fn report(&self, windows: usize, phase: &str) {
        if let Some(report) = self.instance.generate_report() {
            let hub = report.hub_report(self.adapter.get_info().backend);
            tracing::debug!(
                phase,
                windows,
                surfaces = report.surfaces().num_kept_from_user,
                devices = hub.devices.num_kept_from_user,
                adapters = hub.adapters.num_kept_from_user,
                textures = hub.textures.num_kept_from_user,
                bindings = hub.bind_groups.num_kept_from_user,
                pipelines = hub.render_pipelines.num_kept_from_user,
                "GPU resource state"
            );
        }
    }
}
pub(crate) struct Image {
    texture: wgpu::Texture,
    binding: wgpu::BindGroup,
    pipeline: Arc<wgpu::RenderPipeline>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) revision: u64,
    pub(crate) restore_overlay: bool,
    overlay_rect: Option<Rect>,
}
impl Image {
    pub(crate) fn new(
        gpu: &Gpu,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
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
        let binding = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &gpu.binding_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
            ],
        });
        let pipeline = gpu.pipeline(format)?;
        Ok(Self {
            texture,
            binding,
            pipeline,
            width,
            height,
            revision: 0,
            restore_overlay: false,
            overlay_rect: None,
        })
    }
    pub(crate) fn invalidate_overlay(&mut self) {
        self.restore_overlay = true;
    }
    pub(crate) fn upload(&mut self, gpu: &Gpu, fb: &Framebuffer) {
        if self.revision == fb.revision && !self.restore_overlay {
            return;
        }
        let full = [Rect {
            x: 0,
            y: 0,
            width: fb.width,
            height: fb.height,
        }];
        let changed = self.revision != fb.revision;
        let rects = if self.revision == 0 || (changed && fb.damage.rects().is_empty()) {
            &full[..]
        } else if changed {
            fb.damage.rects()
        } else {
            &[]
        };
        let restore = if self.restore_overlay {
            self.overlay_rect.take()
        } else {
            None
        };
        let mut bytes = 0u64;
        let mut rectangles = 0;
        for rect in rects.iter().copied().chain(restore) {
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
            bytes += u64::from(rect.width) * u64::from(rect.height) * 4;
            rectangles += 1;
        }
        tracing::trace!(bytes, rectangles, "framebuffer uploaded");
        self.revision = fb.revision;
        self.restore_overlay = false;
    }
    pub(crate) fn overlay(
        &mut self,
        gpu: &Gpu,
        preedit: &crate::preedit::PreeditImage,
        spot: (i16, i16),
    ) {
        let width = preedit.width.min(self.width);
        let height = preedit.height.min(self.height);
        let x = (spot.0.max(0) as u32).min(self.width - width);
        let y = (spot.1.max(0) as u32).min(self.height - height);
        self.overlay_rect = Some(Rect {
            x,
            y,
            width,
            height,
        });
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
    pub(crate) fn draw(&self, gpu: &Gpu, target: &wgpu::TextureView) {
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
/// A real offscreen shader/texture/readback check; no native window is needed.
pub async fn gpu_smoke() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::util::backend_bits_from_env().unwrap_or(wgpu::Backends::all()),
        ..Default::default()
    });
    let (gpu, _) = Gpu::new(&instance, None).await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut image = Image::new(&gpu, 2, 2, format)?;
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
        image.invalidate_overlay();
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
        fb.pixels[12..16] == [255, 255, 255, 0],
        "IME modified remote framebuffer"
    );
    tracing::info!(
        "GPU texture upload, IME overlay/restore, shader composition and readback passed"
    );
    Ok(())
}

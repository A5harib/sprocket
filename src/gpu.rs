use crate::scene::Ground;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::path::Path;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct QuadUniforms {
    dst_rect: [f32; 4],
    uv_rect: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BlurUniforms {
    vp_rect: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GradeUniforms {
    params: [f32; 4],
    dim_color: [f32; 4],
}

pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

pub struct GpuRenderer {
    pub adapter_name: String,
    pub backend_name: String,
    pub width: u32,
    pub height: u32,
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    quad_layout: wgpu::BindGroupLayout,
    quad_pipeline: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    blur_pipeline: wgpu::RenderPipeline,
    grade_layout: wgpu::BindGroupLayout,
    grade_pipeline: wgpu::RenderPipeline,
    white_tex: GpuTexture,
    /// Offscreen render target for the scrolling feed column (before motion blur)
    feed_tex: GpuTexture,
    /// Intermediate target after compositing ground + blurred feed + fixed chrome (before grade)
    scene_tex: GpuTexture,
    /// Final output render target (after desaturate + dim + headline)
    final_tex: GpuTexture,
    readback_buf: wgpu::Buffer,
    padded_bytes_per_row: u32,
    /// Reusable per-clip GPU textures so VRAM stays bounded
    clip_textures: HashMap<String, GpuTexture>,
    /// Cached per-tile UI overlay GPU textures (only kept while visible)
    overlay_textures: HashMap<i32, GpuTexture>,
}

impl GpuRenderer {
    pub fn probe_adapter() -> Result<(String, String)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context("failed to find a compatible DX12/Vulkan GPU adapter")?;
        let info = adapter.get_info();
        Ok((info.name, format!("{:?}", info.backend)))
    }

    pub fn new(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .context("failed to find a compatible DX12/Vulkan GPU adapter")?;

        let info = adapter.get_info();
        let adapter_name = info.name.clone();
        let backend_name = format!("{:?}", info.backend);

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("sprocket-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .context("failed to open wgpu device")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprocket-shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let format = wgpu::TextureFormat::Rgba8Unorm;

        let make_layout = |label: &str| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        };

        let quad_layout = make_layout("quad-layout");
        let blur_layout = make_layout("blur-layout");
        let grade_layout = make_layout("grade-layout");

        let make_pipeline = |label: &str,
                             layout: &wgpu::BindGroupLayout,
                             vs: &str,
                             fs: &str,
                             blend: Option<wgpu::BlendState>| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        let quad_pipeline = make_pipeline(
            "quad-pipeline",
            &quad_layout,
            "vs_quad",
            "fs_quad",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let blur_pipeline = make_pipeline(
            "blur-pipeline",
            &blur_layout,
            "vs_blur",
            "fs_blur",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let grade_pipeline = make_pipeline(
            "grade-pipeline",
            &grade_layout,
            "vs_full",
            "fs_grade",
            None,
        );

        let white_tex = Self::create_texture_from_rgba(&device, &queue, 1, 1, &[255, 255, 255, 255], "white");
        let feed_tex = Self::create_render_target(&device, width, height, "feed_tex");
        let scene_tex = Self::create_render_target(&device, width, height, "scene_tex");
        let final_tex = Self::create_render_target(&device, width, height, "final_tex");

        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded.div_ceil(align) * align;
        let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback-buf"),
            size: (padded_bytes_per_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Ok(Self {
            adapter_name,
            backend_name,
            width,
            height,
            device,
            queue,
            sampler,
            quad_layout,
            quad_pipeline,
            blur_layout,
            blur_pipeline,
            grade_layout,
            grade_pipeline,
            white_tex,
            feed_tex,
            scene_tex,
            final_tex,
            readback_buf,
            padded_bytes_per_row,
            clip_textures: HashMap::new(),
            overlay_textures: HashMap::new(),
        })
    }

    fn create_render_target(device: &wgpu::Device, width: u32, height: u32, label: &str) -> GpuTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        GpuTexture {
            texture,
            view,
            width,
            height,
        }
    }

    fn create_texture_from_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
        label: &str,
    ) -> GpuTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        GpuTexture {
            texture,
            view,
            width,
            height,
        }
    }

    pub fn upload_static_rgba(&self, width: u32, height: u32, rgba: &[u8], label: &str) -> GpuTexture {
        Self::create_texture_from_rgba(&self.device, &self.queue, width, height, rgba, label)
    }

    pub fn load_image_texture(&self, path: &Path, label: &str) -> Result<GpuTexture> {
        let img = image::open(path)
            .with_context(|| format!("failed to load image {}", path.display()))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        Ok(self.upload_static_rgba(w, h, &img, label))
    }

    /// Upload or update a video/poster frame texture keyed by clip id.
    pub fn update_clip_texture(&mut self, key: &str, width: u32, height: u32, rgba: &[u8]) {
        let recreate = self
            .clip_textures
            .get(key)
            .map(|t| t.width != width || t.height != height)
            .unwrap_or(true);
        if recreate {
            let tex = Self::create_texture_from_rgba(&self.device, &self.queue, width, height, rgba, key);
            self.clip_textures.insert(key.to_string(), tex);
        } else if let Some(tex) = self.clip_textures.get(key) {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    pub fn retain_clip_textures(&mut self, keep_keys: &[String]) {
        self.clip_textures.retain(|k, _| keep_keys.contains(k));
    }

    pub fn has_overlay(&self, slot_k: i32) -> bool {
        self.overlay_textures.contains_key(&slot_k)
    }

    pub fn insert_overlay(&mut self, slot_k: i32, width: u32, height: u32, rgba: &[u8]) {
        let tex = Self::create_texture_from_rgba(&self.device, &self.queue, width, height, rgba, "tile-ui");
        self.overlay_textures.insert(slot_k, tex);
    }

    pub fn retain_overlays(&mut self, keep_slots: &[i32]) {
        self.overlay_textures.retain(|k, _| keep_slots.contains(k));
    }

    pub fn begin_frame(&self) -> FrameBuilder<'_> {
        FrameBuilder {
            gpu: self,
            feed_draws: Vec::new(),
            pre_feed_draws: Vec::new(),
            post_feed_draws: Vec::new(),
            post_grade_draws: Vec::new(),
        }
    }

    fn make_quad_bind(&self, u: QuadUniforms, view: &wgpu::TextureView) -> wgpu::BindGroup {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<QuadUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, bytemuck::bytes_of(&u));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn make_blur_bind(&self, u: BlurUniforms) -> wgpu::BindGroup {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<BlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, bytemuck::bytes_of(&u));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.blur_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.feed_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn make_grade_bind(&self, u: GradeUniforms) -> wgpu::BindGroup {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<GradeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, bytemuck::bytes_of(&u));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.grade_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.scene_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

pub enum QuadSource<'a> {
    Solid,
    Texture(&'a GpuTexture),
    ClipKey(String),
    OverlaySlot(i32),
}

pub struct QuadCmd<'a> {
    pub dst: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub mode: f32,
    pub corner_radius: f32,
    pub tile_px: [f32; 2],
    pub source: QuadSource<'a>,
}

pub struct FrameBuilder<'a> {
    gpu: &'a GpuRenderer,
    pub pre_feed_draws: Vec<QuadCmd<'a>>,
    pub feed_draws: Vec<QuadCmd<'a>>,
    pub post_feed_draws: Vec<QuadCmd<'a>>,
    pub post_grade_draws: Vec<QuadCmd<'a>>,
}

impl<'a> FrameBuilder<'a> {
    pub fn push_ground(&mut self, ground: &Ground, paper_tex: Option<&'a GpuTexture>, to_post_feed: bool) {
        let cmd = match ground {
            Ground::Solid { color } => QuadCmd {
                dst: [0.0, 0.0, self.gpu.width as f32, self.gpu.height as f32],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: *color,
                mode: 0.0,
                corner_radius: 0.0,
                tile_px: [1.0, 1.0],
                source: QuadSource::Solid,
            },
            Ground::Paper { color, tile_px, .. } => {
                if let Some(tex) = paper_tex {
                    QuadCmd {
                        dst: [0.0, 0.0, self.gpu.width as f32, self.gpu.height as f32],
                        uv: [0.0, 0.0, 1.0, 1.0],
                        color: *color,
                        mode: 2.0,
                        corner_radius: 0.0,
                        tile_px: [*tile_px, *tile_px],
                        source: QuadSource::Texture(tex),
                    }
                } else {
                    QuadCmd {
                        dst: [0.0, 0.0, self.gpu.width as f32, self.gpu.height as f32],
                        uv: [0.0, 0.0, 1.0, 1.0],
                        color: *color,
                        mode: 0.0,
                        corner_radius: 0.0,
                        tile_px: [1.0, 1.0],
                        source: QuadSource::Solid,
                    }
                }
            }
        };
        if to_post_feed {
            self.post_feed_draws.push(cmd);
        } else {
            self.pre_feed_draws.push(cmd);
        }
    }

    pub fn push_feed_clip_cover(&mut self, clip_key: &str, src_w: u32, src_h: u32, dst: [f32; 4]) {
        let uv = cover_uv(src_w as f32, src_h as f32, dst[2], dst[3]);
        self.feed_draws.push(QuadCmd {
            dst,
            uv,
            color: [1.0, 1.0, 1.0, 1.0],
            mode: 1.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::ClipKey(clip_key.to_string()),
        });
    }

    pub fn push_feed_overlay(&mut self, slot_k: i32, dst: [f32; 4]) {
        self.feed_draws.push(QuadCmd {
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            mode: 1.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::OverlaySlot(slot_k),
        });
    }

    pub fn push_feed_solid(&mut self, dst: [f32; 4], color: [f32; 4]) {
        self.feed_draws.push(QuadCmd {
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color,
            mode: 0.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::Solid,
        });
    }

    pub fn push_pre_feed_tex(&mut self, tex: &'a GpuTexture, dst: [f32; 4], alpha: f32) {
        self.pre_feed_draws.push(QuadCmd {
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, alpha],
            mode: 1.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::Texture(tex),
        });
    }

    pub fn push_post_feed_tex(&mut self, tex: &'a GpuTexture, dst: [f32; 4], alpha: f32) {
        self.post_feed_draws.push(QuadCmd {
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, alpha],
            mode: 1.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::Texture(tex),
        });
    }

    pub fn push_post_grade_tex(&mut self, tex: &'a GpuTexture, dst: [f32; 4], alpha: f32) {
        self.post_grade_draws.push(QuadCmd {
            dst,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, alpha],
            mode: 1.0,
            corner_radius: 0.0,
            tile_px: [1.0, 1.0],
            source: QuadSource::Texture(tex),
        });
    }

    pub fn render_and_readback(
        self,
        viewport: [f32; 4],
        viewport_radius: f32,
        blur_dy_px: f32,
        blur_samples: u32,
        radial_blur: f32,
        desaturate: f32,
        dim_alpha: f32,
        dim_color: [f32; 4],
    ) -> Result<Vec<u8>> {
        let gpu = self.gpu;
        let cw = gpu.width as f32;
        let ch = gpu.height as f32;

        let build_binds = |cmds: &[QuadCmd<'a>]| -> Vec<wgpu::BindGroup> {
            cmds.iter()
                .filter_map(|c| {
                    let view = match &c.source {
                        QuadSource::Solid => &gpu.white_tex.view,
                        QuadSource::Texture(t) => &t.view,
                        QuadSource::ClipKey(k) => &gpu.clip_textures.get(k)?.view,
                        QuadSource::OverlaySlot(s) => &gpu.overlay_textures.get(s)?.view,
                    };
                    let u = QuadUniforms {
                        dst_rect: c.dst,
                        uv_rect: c.uv,
                        color: c.color,
                        params: [cw, ch, c.mode, c.corner_radius],
                        extra: [c.tile_px[0], c.tile_px[1], 0.0, 0.0],
                    };
                    Some(gpu.make_quad_bind(u, view))
                })
                .collect()
        };

        let feed_binds = build_binds(&self.feed_draws);
        let pre_binds = build_binds(&self.pre_feed_draws);
        let post_binds = build_binds(&self.post_feed_draws);
        let top_binds = build_binds(&self.post_grade_draws);

        let blur_bind = gpu.make_blur_bind(BlurUniforms {
            vp_rect: viewport,
            params: [cw, ch, viewport_radius, blur_dy_px / ch],
            extra: [blur_samples as f32, radial_blur, 0.0, 0.0],
        });

        let grade_bind = gpu.make_grade_bind(GradeUniforms {
            params: [desaturate, dim_alpha, 0.0, 0.0],
            dim_color,
        });

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        // Pass 1: Render scrolling feed tiles into feed_tex
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("feed-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gpu.feed_tex.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&gpu.quad_pipeline);
            for bg in &feed_binds {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
            }
        }

        // Pass 2: Ground + pre-feed + motion-blurred clipped feed + post-feed chrome -> scene_tex
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gpu.scene_tex.view,
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
            pass.set_pipeline(&gpu.quad_pipeline);
            for bg in &pre_binds {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
            }

            pass.set_pipeline(&gpu.blur_pipeline);
            pass.set_bind_group(0, &blur_bind, &[]);
            pass.draw(0..6, 0..1);

            pass.set_pipeline(&gpu.quad_pipeline);
            for bg in &post_binds {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
            }
        }

        // Pass 3: Grade (desaturate + dim) + headline -> final_tex
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("final-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gpu.final_tex.view,
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
            pass.set_pipeline(&gpu.grade_pipeline);
            pass.set_bind_group(0, &grade_bind, &[]);
            pass.draw(0..6, 0..1);

            pass.set_pipeline(&gpu.quad_pipeline);
            for bg in &top_binds {
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..6, 0..1);
            }
        }

        // Copy final_tex -> staging readback buffer
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu.final_tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &gpu.readback_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(gpu.padded_bytes_per_row),
                    rows_per_image: Some(gpu.height),
                },
            },
            wgpu::Extent3d {
                width: gpu.width,
                height: gpu.height,
                depth_or_array_layers: 1,
            },
        );

        gpu.queue.submit(Some(encoder.finish()));

        // Map and strip row padding into tightly packed RGBA frame
        let slice = gpu.readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv().context("GPU map channel dropped")??;

        let mapped = slice.get_mapped_range();
        let row_bytes = (gpu.width * 4) as usize;
        let stride = gpu.padded_bytes_per_row as usize;
        let mut out = vec![0u8; row_bytes * gpu.height as usize];
        if stride == row_bytes {
            out.copy_from_slice(&mapped);
        } else {
            for y in 0..gpu.height as usize {
                let s0 = y * stride;
                let d0 = y * row_bytes;
                out[d0..d0 + row_bytes].copy_from_slice(&mapped[s0..s0 + row_bytes]);
            }
        }
        drop(mapped);
        gpu.readback_buf.unmap();

        Ok(out)
    }
}

/// Compute CSS `object-fit: cover` UV coordinates `(u0, v0, u1, v1)`.
fn cover_uv(src_w: f32, src_h: f32, dst_w: f32, dst_h: f32) -> [f32; 4] {
    let src_aspect = src_w / src_h.max(1.0);
    let dst_aspect = dst_w / dst_h.max(1.0);
    if src_aspect > dst_aspect {
        let keep = dst_aspect / src_aspect;
        let margin = (1.0 - keep) * 0.5;
        [margin, 0.0, 1.0 - margin, 1.0]
    } else {
        let keep = src_aspect / dst_aspect;
        let margin = (1.0 - keep) * 0.5;
        [0.0, margin, 1.0, 1.0 - margin]
    }
}

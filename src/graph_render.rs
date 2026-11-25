//! Renders graph scenes (see `graph.rs`) on the GPU and pipes frames to the encoder.

use crate::decoder::{ClipPool, WindowSpec};
use crate::encoder::{EncoderKind, VideoEncoder};
use crate::fonts::FontStore;
use crate::gpu::GpuRenderer;
use crate::graph::{
    motion_field, Affine, DrawItem, DrawSource, Evaluator, Fit, FlatScene, GraphScene, Layer, LayerBlur, NodeKind,
    UvMode,
};
use crate::text::{rasterize_text, rounded_rect};
use crate::ui::UiRasterizer;
use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use cosmic_text::SwashCache;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Default)]
struct Inst {
    m: [f32; 4],
    t: [f32; 4],
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    cm: [f32; 4],
    ct: [f32; 4],
    cr: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CompU {
    l: [f32; 4],
    tv: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GradeU {
    params: [f32; 4],
    dim: [f32; 4],
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum TexKey {
    Node(usize),
    Image(String),
    Video(usize),
}

struct Tex {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    w: u32,
    h: u32,
    /// For text: where the texture sits in the node's local px.
    local: Option<[f32; 4]>,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct RenderOptions {
    pub encoder: EncoderKind,
    pub motion_blur: bool,
    pub max_decoders: usize,
    pub json: bool,
}

/// Per-node and per-image usage worked out before rendering.
#[derive(Default)]
struct Usage {
    first: HashMap<TexKey, u32>,
    last: HashMap<TexKey, u32>,
    max_scale: HashMap<usize, f32>,
}

impl Usage {
    fn mark(&mut self, key: TexKey, f: u32) {
        self.first.entry(key.clone()).or_insert(f);
        self.last.insert(key, f);
    }
}

enum Pass {
    /// Draw instance batches straight onto the canvas.
    Direct(Vec<(u32, u32, Option<TexKey>)>),
    /// Draw into the layer target, then composite with a velocity blur.
    Blurred(Vec<(u32, u32, Option<TexKey>)>, CompU),
    /// Average sub-frames, then composite.
    Accumulate(Vec<Vec<(u32, u32, Option<TexKey>)>>),
    Grade(GradeU),
}

macro_rules! rp {
    ($view:expr, $load:expr) => {
        wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: $view,
                resolve_target: None,
                ops: wgpu::Operations { load: $load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        }
    };
}

struct GraphGpu<'g> {
    gpu: &'g GpuRenderer,
    w: u32,
    h: u32,
    inst_pipeline: wgpu::RenderPipeline,
    comp_over: wgpu::RenderPipeline,
    comp_add: wgpu::RenderPipeline,
    grade_pipeline: wgpu::RenderPipeline,
    g0_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    full_layout: wgpu::BindGroupLayout,
    samp_clamp: wgpu::Sampler,
    samp_repeat: wgpu::Sampler,
    globals: wgpu::Buffer,
    inst_buf: wgpu::Buffer,
    inst_cap: u64,
    g0: wgpu::BindGroup,
    white: Tex,
    canvas: [Target; 2],
    layer: Target,
    accum: Target,
    readback: wgpu::Buffer,
    padded_row: u32,
}

impl<'g> GraphGpu<'g> {
    fn new(gpu: &'g GpuRenderer, w: u32, h: u32) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("graph-shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("graph.wgsl").into()),
        });
        let sampler = |mode: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let samp_clamp = sampler(wgpu::AddressMode::ClampToEdge);
        let samp_repeat = sampler(wgpu::AddressMode::Repeat);

        let g0_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("graph-g0"),
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
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("graph-tex"),
            entries: &[tex_entry(0)],
        });
        let full_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("graph-full"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let premul = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING;
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = |label: &str, layouts: &[&wgpu::BindGroupLayout], vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
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
                        format: wgpu::TextureFormat::Rgba8Unorm,
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
        let inst_pipeline = pipeline("graph-inst", &[&g0_layout, &tex_layout], "vs_inst", "fs_inst", Some(premul));
        let comp_over = pipeline("graph-comp-over", &[&full_layout], "vs_full", "fs_composite", Some(premul));
        let comp_add = pipeline("graph-comp-add", &[&full_layout], "vs_full", "fs_composite", Some(additive));
        let grade_pipeline = pipeline("graph-grade", &[&full_layout], "vs_full", "fs_grade", None);

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("graph-globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&globals, 0, bytemuck::cast_slice(&[w as f32, h as f32, 0.0, 0.0]));
        let inst_cap = 4096u64;
        let inst_buf = Self::make_inst_buf(device, inst_cap);
        let g0 = Self::make_g0(device, &g0_layout, &globals, &inst_buf, &samp_clamp, &samp_repeat);

        let target = |label: &str| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            Target { texture, view }
        };
        let canvas = [target("canvas-a"), target("canvas-b")];
        let layer = target("layer");
        let accum = target("accum");

        let padded_row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("graph-readback"),
            size: padded_row as u64 * h as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let white = upload_tex(gpu, &tex_layout, 1, 1, &[255, 255, 255, 255], None);
        Self {
            gpu,
            w,
            h,
            inst_pipeline,
            comp_over,
            comp_add,
            grade_pipeline,
            g0_layout,
            tex_layout,
            full_layout,
            samp_clamp,
            samp_repeat,
            globals,
            inst_buf,
            inst_cap,
            g0,
            white,
            canvas,
            layer,
            accum,
            readback,
            padded_row,
        }
    }

    fn make_inst_buf(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("graph-instances"),
            size: cap * std::mem::size_of::<Inst>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn make_g0(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        inst: &wgpu::Buffer,
        clamp: &wgpu::Sampler,
        repeat: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("graph-g0"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: inst.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(clamp) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(repeat) },
            ],
        })
    }

    fn upload(&self, w: u32, h: u32, rgba: &[u8], local: Option<[f32; 4]>) -> Tex {
        upload_tex(self.gpu, &self.tex_layout, w, h, rgba, local)
    }

    fn write(&self, texture: &wgpu::Texture, w: u32, h: u32, rgba: &[u8]) {
        write_tex(self.gpu, texture, w, h, rgba)
    }

    fn full_bind(&self, uniform: &[u8], view: &wgpu::TextureView) -> wgpu::BindGroup {
        let device = &self.gpu.device;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: uniform.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&buf, 0, uniform);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.full_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.samp_clamp) },
            ],
        })
    }

    fn ensure_capacity(&mut self, n: u64) {
        if n <= self.inst_cap {
            return;
        }
        let mut cap = self.inst_cap;
        while cap < n {
            cap *= 2;
        }
        self.inst_cap = cap;
        self.inst_buf = Self::make_inst_buf(&self.gpu.device, cap);
        self.g0 = Self::make_g0(
            &self.gpu.device,
            &self.g0_layout,
            &self.globals,
            &self.inst_buf,
            &self.samp_clamp,
            &self.samp_repeat,
        );
    }

    /// Encode all passes for one frame and read the result back as tight RGBA.
    fn render(
        &mut self,
        insts: &[Inst],
        passes: &[Pass],
        background: [f32; 4],
        textures: &HashMap<TexKey, Tex>,
    ) -> Result<Vec<u8>> {
        self.ensure_capacity(insts.len().max(1) as u64);
        if !insts.is_empty() {
            self.gpu.queue.write_buffer(&self.inst_buf, 0, bytemuck::cast_slice(insts));
        }
        let device = &self.gpu.device;
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("graph-frame") });
        let mut cur = 0usize;

        let clear = |c: [f32; 4]| wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 });
        {
            let _p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.canvas[0].view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: clear(background), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        let draw_batches = |pass: &mut wgpu::RenderPass, batches: &[(u32, u32, Option<TexKey>)]| {
            pass.set_pipeline(&self.inst_pipeline);
            pass.set_bind_group(0, &self.g0, &[]);
            for (a, b, key) in batches {
                let bind = match key {
                    None => &self.white.bind,
                    Some(k) => match textures.get(k) {
                        Some(t) => &t.bind,
                        None => continue,
                    },
                };
                pass.set_bind_group(1, bind, &[]);
                pass.draw(0..6, *a..*b);
            }
        };

        for p in passes {
            match p {
                Pass::Direct(batches) => {
                    let mut pass = enc.begin_render_pass(&rp!(&self.canvas[cur].view, wgpu::LoadOp::Load));
                    draw_batches(&mut pass, batches);
                }
                Pass::Blurred(batches, cu) => {
                    {
                        let mut pass = enc.begin_render_pass(&rp!(&self.layer.view, clear([0.0; 4])));
                        draw_batches(&mut pass, batches);
                    }
                    let bind = self.full_bind(bytemuck::bytes_of(cu), &self.layer.view);
                    let mut pass = enc.begin_render_pass(&rp!(&self.canvas[cur].view, wgpu::LoadOp::Load));
                    pass.set_pipeline(&self.comp_over);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(0..6, 0..1);
                }
                Pass::Accumulate(subs) => {
                    {
                        let _p = enc.begin_render_pass(&rp!(&self.accum.view, clear([0.0; 4])));
                    }
                    let n = subs.len().max(1) as f32;
                    let add = still_comp(self.w, self.h, 1.0 / n);
                    let add_bind = self.full_bind(bytemuck::bytes_of(&add), &self.layer.view);
                    for batches in subs {
                        {
                            let mut pass = enc.begin_render_pass(&rp!(&self.layer.view, clear([0.0; 4])));
                            draw_batches(&mut pass, batches);
                        }
                        let mut pass = enc.begin_render_pass(&rp!(&self.accum.view, wgpu::LoadOp::Load));
                        pass.set_pipeline(&self.comp_add);
                        pass.set_bind_group(0, &add_bind, &[]);
                        pass.draw(0..6, 0..1);
                    }
                    let over = still_comp(self.w, self.h, 1.0);
                    let bind = self.full_bind(bytemuck::bytes_of(&over), &self.accum.view);
                    let mut pass = enc.begin_render_pass(&rp!(&self.canvas[cur].view, wgpu::LoadOp::Load));
                    pass.set_pipeline(&self.comp_over);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(0..6, 0..1);
                }
                Pass::Grade(gu) => {
                    let bind = self.full_bind(bytemuck::bytes_of(gu), &self.canvas[cur].view);
                    let next = 1 - cur;
                    {
                        let mut pass = enc.begin_render_pass(&rp!(&self.canvas[next].view, clear([0.0; 4])));
                        pass.set_pipeline(&self.grade_pipeline);
                        pass.set_bind_group(0, &bind, &[]);
                        pass.draw(0..6, 0..1);
                    }
                    cur = next;
                }
            }
        }

        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.canvas[cur].texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.h),
                },
            },
            wgpu::Extent3d { width: self.w, height: self.h, depth_or_array_layers: 1 },
        );
        self.gpu.queue.submit(Some(enc.finish()));

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().context("GPU map channel dropped")??;
        let mapped = slice.get_mapped_range();
        let row = (self.w * 4) as usize;
        let stride = self.padded_row as usize;
        let mut out = vec![0u8; row * self.h as usize];
        for y in 0..self.h as usize {
            out[y * row..(y + 1) * row].copy_from_slice(&mapped[y * stride..y * stride + row]);
        }
        drop(mapped);
        self.readback.unmap();
        Ok(out)
    }
}

/// Upload premultiplied RGBA as a texture with its bind group.
fn upload_tex(gpu: &GpuRenderer, layout: &wgpu::BindGroupLayout, w: u32, h: u32, rgba: &[u8], local: Option<[f32; 4]>) -> Tex {
    let device = &gpu.device;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_tex(gpu, &texture, w, h, rgba);
    let view = texture.create_view(&Default::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
    });
    Tex { texture, bind, w, h, local }
}

fn write_tex(gpu: &GpuRenderer, texture: &wgpu::Texture, w: u32, h: u32, rgba: &[u8]) {
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn still_comp(w: u32, h: u32, weight: f32) -> CompU {
    CompU { l: [0.0; 4], tv: [0.0; 4], params: [w as f32, h as f32, 0.0, 1.0], extra: [weight, 0.0, 0.0, 0.0] }
}

pub fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
}

fn cover_uv(sw: f32, sh: f32, dw: f32, dh: f32) -> [f32; 4] {
    let sa = sw / sh.max(1.0);
    let da = dw / dh.max(1e-3);
    if sa > da {
        let keep = da / sa;
        let m = (1.0 - keep) * 0.5;
        [m, 0.0, 1.0 - m, 1.0]
    } else {
        let keep = sa / da;
        let m = (1.0 - keep) * 0.5;
        [0.0, m, 1.0, 1.0 - m]
    }
}

// ---------------------------------------------------------------------------------------------
// Rasterising node textures
// ---------------------------------------------------------------------------------------------

struct Rasterizer<'a> {
    assets: &'a Path,
    fonts: FontStore,
    swash: SwashCache,
    ui: UiRasterizer,
    usvg_opts: Option<resvg::usvg::Options<'static>>,
    canvas_w: u32,
    canvas_h: u32,
}

impl<'a> Rasterizer<'a> {
    fn svg_opts(&mut self) -> &resvg::usvg::Options<'static> {
        if self.usvg_opts.is_none() {
            let mut o = resvg::usvg::Options::default();
            o.resources_dir = Some(self.assets.to_path_buf());
            o.fontdb_mut().load_system_fonts();
            let fonts_dir = self.assets.join("fonts");
            if fonts_dir.is_dir() {
                o.fontdb_mut().load_fonts_dir(&fonts_dir);
            }
            self.usvg_opts = Some(o);
        }
        self.usvg_opts.as_ref().unwrap()
    }

    /// Premultiplied RGBA for a node, plus text placement.
    fn node_texture(&mut self, node: &crate::graph::Node, scale: f32) -> Result<Option<(u32, u32, Vec<u8>, Option<[f32; 4]>)>> {
        let s = scale.clamp(0.05, 4.0);
        let (bw, bh) = (node.width, node.height);
        let px = |v: f32| ((v * s).ceil() as u32).clamp(1, 8192);
        Ok(match &node.kind {
            NodeKind::Text(spec) => {
                let r = rasterize_text(&mut self.fonts, &mut self.swash, spec, bw, s);
                Some((r.width, r.height, r.rgba, Some(r.local)))
            }
            NodeKind::Rect { color, gradient, stroke } => {
                let (w, h) = (px(bw), px(bh));
                let mut pix = tiny_skia::Pixmap::new(w, h).context("rect pixmap")?;
                let path = rounded_rect(0.0, 0.0, w as f32, h as f32, node.corner_radius * s);
                fill_and_stroke(&mut pix, path, *color, gradient.as_ref(), stroke.as_ref(), s);
                Some((w, h, pix.take(), None))
            }
            NodeKind::Ellipse { color, gradient, stroke } => {
                let (w, h) = (px(bw), px(bh));
                let mut pix = tiny_skia::Pixmap::new(w, h).context("ellipse pixmap")?;
                let path = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32)
                    .and_then(|r| tiny_skia::PathBuilder::from_oval(r));
                fill_and_stroke(&mut pix, path, *color, gradient.as_ref(), stroke.as_ref(), s);
                Some((w, h, pix.take(), None))
            }
            NodeKind::Polygon { points, color } => {
                let (w, h) = (px(bw), px(bh));
                let mut pix = tiny_skia::Pixmap::new(w, h).context("polygon pixmap")?;
                let mut pb = tiny_skia::PathBuilder::new();
                for (i, p) in points.iter().enumerate() {
                    if i == 0 {
                        pb.move_to(p[0] * s, p[1] * s);
                    } else {
                        pb.line_to(p[0] * s, p[1] * s);
                    }
                }
                pb.close();
                fill_and_stroke(&mut pix, pb.finish(), *color, None, None, s);
                Some((w, h, pix.take(), None))
            }
            NodeKind::Svg { src, data } => {
                let bytes = match (src, data) {
                    (Some(p), _) => std::fs::read(self.assets.join(p)).with_context(|| format!("failed to read svg {p}"))?,
                    (None, Some(d)) => d.as_bytes().to_vec(),
                    _ => return Ok(None),
                };
                let tree = resvg::usvg::Tree::from_data(&bytes, self.svg_opts()).context("failed to parse svg")?;
                let size = tree.size();
                let (w, h) = (
                    px(if bw > 0.0 { bw } else { size.width() }),
                    px(if bh > 0.0 { bh } else { size.height() }),
                );
                let mut pix = tiny_skia::Pixmap::new(w, h).context("svg pixmap")?;
                let xf = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
                resvg::render(&tree, xf, &mut pix.as_mut());
                Some((w, h, pix.take(), None))
            }
            NodeKind::IgReel { tile, chrome } => {
                let (w, h) = (bw.round().max(1.0) as u32, bh.round().max(1.0) as u32);
                let mut rgba = self.ui.render_reel_overlay(&mut self.fonts, tile, *chrome, w, h, self.assets);
                premultiply(&mut rgba);
                Some((w, h, rgba, None))
            }
            NodeKind::IgFixed { nav_top, profile_poster } => {
                let mut rgba = self.ui.render_ig_fixed(
                    &mut self.fonts,
                    self.canvas_w,
                    self.canvas_h,
                    *nav_top,
                    &self.assets.join(profile_poster),
                );
                premultiply(&mut rgba);
                Some((self.canvas_w, self.canvas_h, rgba, None))
            }
            _ => None,
        })
    }

    fn image(&self, rel: &str) -> Result<(u32, u32, Vec<u8>)> {
        let path = self.assets.join(rel);
        let img = image::open(&path)
            .with_context(|| format!("failed to load image {}", path.display()))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let mut raw = img.into_raw();
        premultiply(&mut raw);
        Ok((w, h, raw))
    }
}

fn fill_and_stroke(
    pix: &mut tiny_skia::Pixmap,
    path: Option<tiny_skia::Path>,
    color: crate::graph::Rgba,
    gradient: Option<&crate::graph::Gradient>,
    stroke: Option<&crate::graph::Stroke>,
    s: f32,
) {
    let Some(path) = path else {
        return;
    };
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    let mut paint = tiny_skia::Paint { anti_alias: true, ..Default::default() };
    let tc = |c: crate::graph::Rgba| tiny_skia::Color::from_rgba(c.0[0], c.0[1], c.0[2], c.0[3]).unwrap_or(tiny_skia::Color::BLACK);
    match gradient {
        Some(g) if g.stops.len() >= 2 => {
            let stops: Vec<tiny_skia::GradientStop> =
                g.stops.iter().map(|(o, c)| tiny_skia::GradientStop::new(*o, tc(*c))).collect();
            let shader = if g.radial {
                let c = tiny_skia::Point::from_xy(w * 0.5, h * 0.5);
                tiny_skia::RadialGradient::new(c, c, (w * w + h * h).sqrt() * 0.5, stops, tiny_skia::SpreadMode::Pad, tiny_skia::Transform::identity())
            } else {
                let a = g.angle.to_radians();
                let (dx, dy) = (a.sin(), -a.cos());
                let len = (w * dx).abs() + (h * dy).abs();
                let (cx, cy) = (w * 0.5, h * 0.5);
                tiny_skia::LinearGradient::new(
                    tiny_skia::Point::from_xy(cx - dx * len * 0.5, cy - dy * len * 0.5),
                    tiny_skia::Point::from_xy(cx + dx * len * 0.5, cy + dy * len * 0.5),
                    stops,
                    tiny_skia::SpreadMode::Pad,
                    tiny_skia::Transform::identity(),
                )
            };
            if let Some(sh) = shader {
                paint.shader = sh;
            }
        }
        _ => paint.set_color(tc(color)),
    }
    pix.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    if let Some(st) = stroke {
        let mut sp = tiny_skia::Paint { anti_alias: true, ..Default::default() };
        sp.set_color(tc(st.color));
        let stroke = tiny_skia::Stroke { width: st.width * s, ..Default::default() };
        pix.stroke_path(&path, &sp, &stroke, tiny_skia::Transform::identity(), None);
    }
}

// ---------------------------------------------------------------------------------------------
// The render loop
// ---------------------------------------------------------------------------------------------

fn tex_key_for(item: &DrawItem, flat: &FlatScene) -> Option<TexKey> {
    match item.source {
        DrawSource::Solid => None,
        DrawSource::Video(i) => Some(TexKey::Video(i)),
        DrawSource::Static(i) => match &flat.nodes[i].node.kind {
            NodeKind::Image { src, .. } => Some(TexKey::Image(src.clone())),
            _ => Some(TexKey::Node(i)),
        },
    }
}

fn is_cullable(item: &DrawItem, flat: &FlatScene) -> bool {
    !matches!(flat.nodes[item.node].node.kind, NodeKind::Text(_) | NodeKind::IgFixed { .. })
}

fn on_screen(item: &DrawItem, rect: [f32; 4], w: f32, h: f32) -> bool {
    let bb = item.xf.bbox(rect);
    let m = item.blur * 3.0 + 2.0;
    let vis = bb[2] > -m && bb[0] < w + m && bb[3] > -m && bb[1] < h + m;
    if !vis {
        return false;
    }
    if let Some(c) = &item.clip {
        return bb[2] > c.bbox[0] - m && bb[0] < c.bbox[2] + m && bb[3] > c.bbox[1] - m && bb[1] < c.bbox[3] + m;
    }
    true
}

fn build_inst(item: &DrawItem, tex: Option<&Tex>) -> Option<Inst> {
    let mut rect = item.rect;
    let (mut mode, mut uv) = (0.0f32, [0.0, 0.0, 1.0, 1.0]);
    if let Some(t) = tex {
        if let Some(l) = t.local {
            rect = l;
        }
        mode = 1.0;
        match item.uv {
            UvMode::Stretch(u) => uv = u,
            UvMode::Tile { tile, offset } => {
                mode = 2.0;
                uv = [tile[0], tile[1], offset[0], offset[1]];
            }
            UvMode::Fit(Fit::Tile) => {
                mode = 2.0;
                uv = [t.w as f32, t.h as f32, 0.0, 0.0];
            }
            UvMode::Fit(Fit::Fill) => {}
            UvMode::Fit(Fit::Cover) => uv = cover_uv(t.w as f32, t.h as f32, rect[2], rect[3]),
            UvMode::Fit(Fit::Contain) => {
                let k = (rect[2] / t.w as f32).min(rect[3] / t.h as f32);
                let (cw, ch) = (t.w as f32 * k, t.h as f32 * k);
                rect = [rect[0] + (rect[2] - cw) * 0.5, rect[1] + (rect[3] - ch) * 0.5, cw, ch];
            }
        }
    }
    if rect[2] <= 0.0 || rect[3] <= 0.0 {
        return None;
    }
    let (sx, sy) = item.xf.scale_xy();
    let (sx, sy) = (sx.max(1e-6), sy.max(1e-6));
    let mut pad = [1.0 / sx, 1.0 / sy];
    let mut sigma = [0.0f32, 0.0];
    if item.blur > 0.05 && mode == 1.0 {
        mode = 3.0;
        sigma = [
            item.blur / (rect[2] * sx) * (uv[2] - uv[0]),
            item.blur / (rect[3] * sy) * (uv[3] - uv[1]),
        ];
        pad = [pad[0] + item.blur * 2.5 / sx, pad[1] + item.blur * 2.5 / sy];
    }
    let xf: Affine = item.xf;
    let (cm, ct, cr) = match &item.clip {
        Some(c) => (
            [c.inv.a, c.inv.b, c.inv.c, c.inv.d],
            [c.inv.e, c.inv.f, c.radius, 1.0],
            c.rect,
        ),
        None => ([1.0, 0.0, 0.0, 1.0], [0.0; 4], [0.0; 4]),
    };
    Some(Inst {
        m: [xf.a, xf.b, xf.c, xf.d],
        t: [xf.e, xf.f, pad[0], pad[1]],
        rect,
        uv,
        color: item.color,
        params: [mode, item.radius, sigma[0], sigma[1]],
        cm,
        ct,
        cr,
    })
}

pub fn render_graph(scene: &GraphScene, assets_dir: &Path, out_path: &Path, opts: &RenderOptions) -> Result<()> {
    let t_start = Instant::now();
    let total = scene.total_frames();
    let fps = scene.fps as f64;
    let (cw, ch) = (scene.width as f32, scene.height as f32);
    let gpu = GpuRenderer::new(scene.width, scene.height)?;
    if !opts.json {
        println!(
            "[sprocket] GPU: {} ({}) | graph scene {}x{} @ {}fps ({} frames, {} nodes)",
            gpu.adapter_name,
            gpu.backend_name,
            scene.width,
            scene.height,
            scene.fps,
            total,
            scene.node_count()
        );
    }
    let mut gg = GraphGpu::new(&gpu, scene.width, scene.height);

    let mut fonts = FontStore::load(assets_dir)?;
    for f in &scene.fonts {
        let p = assets_dir.join(f);
        let bytes = std::fs::read(&p).with_context(|| format!("failed to read font {}", p.display()))?;
        let data = match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("woff2") => wuff::decompress_woff2(&bytes).ok().unwrap_or_default(),
            Some("woff") => wuff::decompress_woff1(&bytes).ok().unwrap_or_default(),
            _ => bytes,
        };
        fonts.font_system.db_mut().load_font_data(data);
    }
    let mut raster = Rasterizer {
        assets: assets_dir,
        fonts,
        swash: SwashCache::new(),
        ui: UiRasterizer::new(),
        usvg_opts: None,
        canvas_w: scene.width,
        canvas_h: scene.height,
    };

    let flat = FlatScene::new(scene);
    let ev = Evaluator::new(&flat);
    let shutter = (scene.motion_blur.shutter_angle / 360.0) as f64 / fps;
    let blur_on = opts.motion_blur && scene.motion_blur.enabled;

    // Track nodes for velocity blur
    let mut track_nodes: HashMap<usize, usize> = HashMap::new();
    for (li, l) in scene.layers.iter().enumerate() {
        if let Layer::Nodes { motion_blur: LayerBlur::Track { node }, .. } = l {
            let idx = *flat
                .by_id
                .get(node)
                .with_context(|| format!("motion_blur track node {node:?} not found"))?;
            track_nodes.insert(li, idx);
        }
    }

    // 1. Usage pass: when each texture is first and last needed, and how large it gets.
    let mut usage = Usage::default();
    let mut items: Vec<DrawItem> = Vec::new();
    for f in 0..total {
        let t = f as f64 / fps;
        for (li, l) in scene.layers.iter().enumerate() {
            if !matches!(l, Layer::Nodes { .. }) {
                continue;
            }
            items.clear();
            ev.layer_items(li, t, &mut items);
            for it in &items {
                if is_cullable(it, &flat) && !on_screen(it, it.rect, cw, ch) {
                    continue;
                }
                if let Some(k) = tex_key_for(it, &flat) {
                    usage.mark(k, f);
                }
                if let DrawSource::Static(i) = it.source {
                    let (sx, sy) = it.xf.scale_xy();
                    let e = usage.max_scale.entry(i).or_insert(0.0);
                    *e = e.max(sx.max(sy));
                }
            }
        }
    }

    // 2. Video windows
    let mut specs = Vec::new();
    for (k, first) in &usage.first {
        if let TexKey::Video(i) = k {
            let last = usage.last[k];
            if let NodeKind::Video { src, start, media_start, .. } = &flat.nodes[*i].node.kind {
                let first = first.saturating_sub(1);
                specs.push(WindowSpec {
                    key: format!("n{i}"),
                    path: assets_dir.join(src),
                    first_frame: first,
                    last_frame: (last + 1).min(total.saturating_sub(1)),
                    start_sec: first as f64 / fps - start + media_start,
                });
            }
        }
    }
    let video_count = specs.len();
    let mut clips = ClipPool::from_windows(scene.fps, opts.max_decoders.max(1), specs);

    let audio_path: Option<PathBuf> = scene.audio.as_ref().map(|a| assets_dir.join(a));
    let encoder = VideoEncoder::start(scene.width, scene.height, scene.fps, audio_path.as_deref(), out_path, opts.encoder)?;
    if !opts.json {
        println!(
            "[sprocket] Encoder: {} | Audio: {} | {} video windows",
            encoder.codec_name,
            audio_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()),
            video_count
        );
    }
    let init_ms = t_start.elapsed().as_millis();
    let t_loop = Instant::now();

    let mut textures: HashMap<TexKey, Tex> = HashMap::new();
    let mut peak_textures = 0usize;
    let mut peak_decoders = 0usize;
    let bg = scene.background.premul(1.0);
    let (mut t_build, mut t_gpu, mut t_send, mut t_raster) = (0f64, 0f64, 0f64, 0f64);

    for f in 0..total {
        let t = f as f64 / fps;
        let tb = Instant::now();
        clips.prepare_frame(f)?;
        peak_decoders = peak_decoders.max(clips.active_count());

        // Release textures that are done for good.
        textures.retain(|k, _| match usage.last.get(k) {
            Some(last) => *last + 1 >= f,
            None => matches!(k, TexKey::Image(_)),
        });

        let mut insts: Vec<Inst> = Vec::new();
        let mut passes: Vec<Pass> = Vec::new();
        let mut uploaded_video: Vec<usize> = Vec::new();

        for (li, layer) in scene.layers.iter().enumerate() {
            match layer {
                Layer::Grade { desaturate, dim, dim_color } => {
                    let d = desaturate.as_ref().map(|v| v.eval(t)).unwrap_or(0.0);
                    let m = dim.as_ref().map(|v| v.eval(t)).unwrap_or(0.0);
                    if d > 1e-4 || m > 1e-4 {
                        passes.push(Pass::Grade(GradeU { params: [d, m, 0.0, 0.0], dim: dim_color.0 }));
                    }
                }
                Layer::Nodes { motion_blur, .. } => {
                    let times: Vec<f64> = match (blur_on, motion_blur) {
                        (true, LayerBlur::Accumulate { samples }) => {
                            let n = samples.unwrap_or(scene.motion_blur.samples).clamp(1, 64) as usize;
                            (0..n).map(|s| (t + ((s as f64 + 0.5) / n as f64 - 0.5) * shutter).max(0.0)).collect()
                        }
                        _ => vec![t],
                    };
                    let mut sub_batches = Vec::new();
                    for &ts in &times {
                        items.clear();
                        ev.layer_items(li, ts, &mut items);
                        let mut batches: Vec<(u32, u32, Option<TexKey>)> = Vec::new();
                        for it in &items {
                            // Skip off-screen items before touching textures.
                            if is_cullable(it, &flat) && !on_screen(it, it.rect, cw, ch) {
                                continue;
                            }
                            let key = tex_key_for(it, &flat);
                            // Make sure the texture exists.
                            let key = match key {
                                Some(TexKey::Video(i)) => {
                                    let vkey = TexKey::Video(i);
                                    if !uploaded_video.contains(&i) {
                                        if let Some((w, h, rgba)) = clips.video_frame(&format!("n{i}"), f) {
                                            match textures.get(&vkey) {
                                                Some(tx) if tx.w == w && tx.h == h => gg.write(&tx.texture, w, h, rgba),
                                                _ => {
                                                    let tx = gg.upload(w, h, rgba, None);
                                                    textures.insert(vkey.clone(), tx);
                                                }
                                            }
                                            uploaded_video.push(i);
                                        }
                                    }
                                    if textures.contains_key(&vkey) {
                                        Some(vkey)
                                    } else if let NodeKind::Video { poster: Some(p), .. } = &flat.nodes[i].node.kind {
                                        let pk = TexKey::Image(p.clone());
                                        if !textures.contains_key(&pk) {
                                            let (w, h, rgba) = raster.image(p)?;
                                            textures.insert(pk.clone(), gg.upload(w, h, &rgba, None));
                                        }
                                        Some(pk)
                                    } else {
                                        continue;
                                    }
                                }
                                Some(TexKey::Image(src)) => {
                                    let k = TexKey::Image(src.clone());
                                    if !textures.contains_key(&k) {
                                        let (w, h, rgba) = raster.image(&src)?;
                                        textures.insert(k.clone(), gg.upload(w, h, &rgba, None));
                                    }
                                    Some(k)
                                }
                                Some(TexKey::Node(i)) => {
                                    let k = TexKey::Node(i);
                                    if !textures.contains_key(&k) {
                                        let node = flat.nodes[i].node;
                                        let scale = node
                                            .raster_scale
                                            .unwrap_or_else(|| usage.max_scale.get(&i).copied().unwrap_or(1.0).clamp(0.1, 2.0));
                                        let tr = Instant::now();
                                        let made = raster.node_texture(node, scale)?;
                                        t_raster += tr.elapsed().as_secs_f64();
                                        if std::env::var_os("SPROCKET_DEBUG").is_some() {
                                            eprintln!(
                                                "[raster] f{} node {} {:?} scale {:.2} {:.1}ms",
                                                f,
                                                i,
                                                node.id,
                                                scale,
                                                tr.elapsed().as_secs_f64() * 1000.0
                                            );
                                        }
                                        match made {
                                            Some((w, h, rgba, local)) => {
                                                textures.insert(k.clone(), gg.upload(w, h, &rgba, local));
                                            }
                                            None => continue,
                                        }
                                    }
                                    Some(k)
                                }
                                None => None,
                            };
                            let tex = key.as_ref().and_then(|k| textures.get(k));
                            let Some(inst) = build_inst(it, tex) else {
                                continue;
                            };
                            if !on_screen(it, inst.rect, cw, ch) {
                                continue;
                            }
                            let idx = insts.len() as u32;
                            insts.push(inst);
                            match batches.last_mut() {
                                Some(b) if b.2 == key && b.1 == idx => b.1 = idx + 1,
                                _ => batches.push((idx, idx + 1, key)),
                            }
                        }
                        sub_batches.push(batches);
                    }
                    if times.len() > 1 {
                        passes.push(Pass::Accumulate(sub_batches));
                    } else {
                        let batches = sub_batches.pop().unwrap_or_default();
                        match (blur_on, track_nodes.get(&li)) {
                            (true, Some(&node)) => {
                                let mf = motion_field(&ev, node, t);
                                let cu = CompU {
                                    l: mf.l,
                                    tv: [mf.t0[0], mf.t0[1], mf.v[0], mf.v[1]],
                                    params: [cw, ch, shutter as f32, scene.motion_blur.samples as f32],
                                    extra: [1.0, 0.0, 0.0, 0.0],
                                };
                                passes.push(Pass::Blurred(batches, cu));
                            }
                            _ => passes.push(Pass::Direct(batches)),
                        }
                    }
                }
            }
        }
        peak_textures = peak_textures.max(textures.len());

        t_build += tb.elapsed().as_secs_f64();
        let tg = Instant::now();
        let rgba = gg.render(&insts, &passes, bg, &textures)?;
        t_gpu += tg.elapsed().as_secs_f64();
        let ts = Instant::now();
        encoder.send_frame(rgba)?;
        t_send += ts.elapsed().as_secs_f64();
        if !opts.json && f % 60 == 0 {
            println!("[sprocket] frame {}/{} ({} quads, {} textures)", f, total, insts.len(), textures.len());
        }
    }

    let codec = encoder.codec_name;
    encoder.finish()?;
    let render_s = t_loop.elapsed().as_secs_f64();
    let total_s = t_start.elapsed().as_secs_f64();
    let fps_eff = total as f64 / render_s.max(0.001);
    let bytes = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "format": "graph",
                "id": scene.id,
                "out": out_path.display().to_string(),
                "bytes": bytes,
                "width": scene.width,
                "height": scene.height,
                "fps": scene.fps,
                "frames": total,
                "duration": scene.duration,
                "nodes": scene.node_count(),
                "gpu": gpu.adapter_name,
                "backend": gpu.backend_name,
                "encoder": codec,
                "init_ms": init_ms,
                "render_s": (render_s * 100.0).round() / 100.0,
                "total_s": (total_s * 100.0).round() / 100.0,
                "render_fps": (fps_eff * 10.0).round() / 10.0,
                "peak_decoders": peak_decoders,
                "peak_textures": peak_textures,
                "timings_s": {
                    "build": (t_build * 100.0).round() / 100.0,
                    "raster": (t_raster * 100.0).round() / 100.0,
                    "gpu_readback": (t_gpu * 100.0).round() / 100.0,
                    "encode_send": (t_send * 100.0).round() / 100.0,
                },
            }))?
        );
    } else {
        println!(
            "[sprocket] Done: {} | total {:.2}s (init {}ms, render {:.2}s, {:.1} fps) | peak decoders {} | peak textures {}",
            out_path.display(),
            total_s,
            init_ms,
            render_s,
            fps_eff,
            peak_decoders,
            peak_textures
        );
    }
    Ok(())
}

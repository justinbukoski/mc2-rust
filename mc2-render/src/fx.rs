// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! CROSS-CRATE (mc2-fx lane X1): the drawing hook for gosFX effects.
//!
//! `mc2-fx` simulates effects and produces plain draw data; this module draws it. The data is a
//! list of [`FxBatch`]es (textured, vertex-coloured triangles in game world coordinates with
//! one blend and depth state each), drawn inside the scene's main render pass after the
//! terrain, shapes and water, depth-tested against them. The blend modes are the `MLRState`
//! alpha modes the effects use (`mclib/mlr/mlrstate.hpp`, set up in `MLRState::SetRendererState`
//! as `gos_Alpha_*`): the target keeps its alpha untouched, like the other scene blends.

use crate::error::{RenderError, Result};
use crate::gpu::{Gpu, OffscreenTarget, COLOR_FORMAT, DEPTH_FORMAT};
use crate::math::Mat4;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SHADER: &str = include_str!("shaders/fx.wgsl");

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FxVertex {
    /// Game world coordinates (not the `Stuff` frame effects are simulated in).
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// Straight RGBA multiplied into the texture.
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct FxGpuVertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
    ground_clearance: f32,
    fade_rgb: f32,
}

impl FxGpuVertex {
    fn new(vertex: FxVertex, ground_clearance: f32, fade_rgb: bool) -> Self {
        Self {
            pos: vertex.pos,
            uv: vertex.uv,
            color: vertex.color,
            ground_clearance,
            fade_rgb: if fade_rgb { 1.0 } else { 0.0 },
        }
    }
}

/// `MLRState::AlphaMode`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FxBlend {
    /// `OneZeroMode`: replace.
    Opaque,
    /// `OneOneMode`: `src + dst`.
    Additive,
    /// `AlphaOneMode`: `src * alpha + dst`.
    AlphaAdditive,
    /// `OneAlphaMode`: `src + dst * alpha`.
    OneAlpha,
    /// `AlphaInvAlphaMode`: the usual alpha blend.
    Alpha,
    /// `OneInvAlphaMode`: premultiplied alpha.
    Premultiplied,
}

impl FxBlend {
    const ALL: [FxBlend; 6] = [
        FxBlend::Opaque,
        FxBlend::Additive,
        FxBlend::AlphaAdditive,
        FxBlend::OneAlpha,
        FxBlend::Alpha,
        FxBlend::Premultiplied,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|b| *b == self).unwrap_or(0)
    }

    fn state(self) -> Option<wgpu::BlendState> {
        use wgpu::BlendFactor as F;
        let color = |src, dst| wgpu::BlendComponent {
            src_factor: src,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        };
        // The target stays opaque: blending must not write coverage into alpha.
        let alpha = wgpu::BlendComponent {
            src_factor: F::Zero,
            dst_factor: F::One,
            operation: wgpu::BlendOperation::Add,
        };
        let (c, a) = match self {
            FxBlend::Opaque => return None,
            FxBlend::Additive => (color(F::One, F::One), alpha),
            FxBlend::AlphaAdditive => (color(F::SrcAlpha, F::One), alpha),
            FxBlend::OneAlpha => (color(F::One, F::SrcAlpha), alpha),
            FxBlend::Alpha => (color(F::SrcAlpha, F::OneMinusSrcAlpha), alpha),
            FxBlend::Premultiplied => (color(F::One, F::OneMinusSrcAlpha), alpha),
        };
        Some(wgpu::BlendState { color: c, alpha: a })
    }
}

/// A texture registered with [`FxRenderer::add_texture`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxTextureId(pub usize);

/// Triangles that share one state.
#[derive(Clone, Debug)]
pub struct FxBatch {
    pub texture: Option<FxTextureId>,
    pub blend: FxBlend,
    pub z_write: bool,
    pub z_compare: bool,
    /// Texture clamp instead of wrap.
    pub clamp: bool,
    /// Fade this effect into terrain over a short vertical range. Used for gosFX particles and
    /// shapes so billboards at ground level do not end in a hard depth-test cut.
    pub ground_fade: bool,
    /// Lower priorities draw first (`MLRState` priority).
    pub priority: u32,
    pub vertices: Vec<FxVertex>,
    /// Triangle list into `vertices`.
    pub indices: Vec<u32>,
}

struct FxTexture {
    wrap: wgpu::BindGroup,
    clamp: wgpu::BindGroup,
}

/// Effect pipelines per (colour format, sample count).
type PipelineCache = HashMap<(wgpu::TextureFormat, u32), Arc<Vec<wgpu::RenderPipeline>>>;

pub struct FxRenderer {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// Pipelines (one per blend/depth slot) per (colour format, sample count) of the target.
    pipelines: Mutex<PipelineCache>,
    bgl_globals: wgpu::BindGroupLayout,
    textures: Vec<FxTexture>,
    white: FxTexture,
    sampler_wrap: wgpu::Sampler,
    sampler_clamp: wgpu::Sampler,
    bgl_tex: wgpu::BindGroupLayout,
}

/// Pipeline slot for a blend and depth state.
fn slot(blend: FxBlend, z_write: bool, z_compare: bool) -> usize {
    blend.index() * 4 + (z_write as usize) * 2 + z_compare as usize
}

impl FxRenderer {
    /// Builds the pipelines for the scene's colour/depth formats and MSAA count.
    pub fn new(gpu: &Gpu) -> Result<FxRenderer> {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl_globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let bgl_tex = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx texture"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx"),
            bind_group_layouts: &[Some(&bgl_globals), Some(&bgl_tex)],
            immediate_size: 0,
        });
        let first = Arc::new(build_pipelines(
            device,
            &module,
            &layout,
            gpu.target_format,
            gpu.samples,
        )?);
        let mut cache = HashMap::new();
        cache.insert((gpu.target_format, gpu.samples), first);
        let sampler = |address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("fx sampler"),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let sampler_wrap = sampler(wgpu::AddressMode::Repeat);
        let sampler_clamp = sampler(wgpu::AddressMode::ClampToEdge);
        let mut r = FxRenderer {
            module,
            layout,
            pipelines: Mutex::new(cache),
            bgl_globals,
            textures: Vec::new(),
            white: FxTexture {
                wrap: dummy_bg(device, &bgl_tex, &sampler_wrap),
                clamp: dummy_bg(device, &bgl_tex, &sampler_clamp),
            },
            sampler_wrap,
            sampler_clamp,
            bgl_tex,
        };
        // 1x1 white for untextured batches
        let white = r.make_texture(gpu, "fx white", 1, 1, &[255, 255, 255, 255]);
        r.white = white;
        Ok(r)
    }

    fn make_texture(&self, gpu: &Gpu, name: &str, w: u32, h: u32, rgba: &[u8]) -> FxTexture {
        let tex = gpu.texture_2d(name, w, h, rgba, true);
        let view = tex.create_view(&Default::default());
        let bg = |sampler: &wgpu::Sampler| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx texture"),
                layout: &self.bgl_tex,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        };
        FxTexture {
            wrap: bg(&self.sampler_wrap),
            clamp: bg(&self.sampler_clamp),
        }
    }

    /// Registers an RGBA8 texture (through the same hi-res replacement hook as the scene
    /// textures: `name` is the replacement key base).
    pub fn add_texture(
        &mut self,
        gpu: &Gpu,
        name: &str,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> FxTextureId {
        let t = self.make_texture(gpu, name, w, h, rgba);
        self.textures.push(t);
        FxTextureId(self.textures.len() - 1)
    }

    /// Draws the batches alone on a cleared offscreen target and returns tightly packed RGBA8
    /// (tools and tests; the game draws through [`draw`](Self::draw) in the scene pass).
    pub fn render_offscreen(
        &self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        clear: [f64; 3],
        view_proj: &Mat4,
        batches: &[FxBatch],
    ) -> Result<Vec<u8>> {
        self.render_offscreen_with_ground(gpu, width, height, clear, view_proj, batches, None)
    }

    /// Offscreen version of [`Self::draw_into`] with a terrain map for explosion softening.
    #[allow(clippy::too_many_arguments)]
    pub fn render_offscreen_with_ground(
        &self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        clear: [f64; 3],
        view_proj: &Mat4,
        batches: &[FxBatch],
        ground: Option<&crate::terrain::TerrainMap>,
    ) -> Result<Vec<u8>> {
        let offscreen = OffscreenTarget::new(gpu, width, height);
        let target = offscreen.target();
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fx offscreen"),
            });
        {
            let (view, resolve) = target.color_attachment();
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fx offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::gpu::clear_color(
                            [clear[0] as f32, clear[1] as f32, clear[2] as f32],
                            1.0,
                            target.display_format,
                        )),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_into(
                gpu,
                &mut pass,
                target.display_format,
                target.samples,
                view_proj,
                batches,
                ground,
            )?;
        }
        target.read_rgba(gpu, enc)
    }

    /// Draws the batches into a render pass that has the scene's colour and depth attachments,
    /// for a target in the device's default configuration ([`Gpu::target_format`],
    /// [`Gpu::samples`]); see [`Self::draw_into`].
    pub fn draw(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        view_proj: &Mat4,
        batches: &[FxBatch],
    ) {
        if let Err(e) = self.draw_into(
            gpu,
            pass,
            gpu.target_format,
            gpu.samples,
            view_proj,
            batches,
            None,
        ) {
            log::error!("fx draw: {e}");
        }
    }

    /// The pipelines for targets of `format` with `samples` MSAA samples (built on first use).
    fn pipelines_for(
        &self,
        gpu: &Gpu,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> Result<Arc<Vec<wgpu::RenderPipeline>>> {
        let mut cache = self
            .pipelines
            .lock()
            .map_err(|_| RenderError::Other("fx pipeline cache poisoned".into()))?;
        if let Some(p) = cache.get(&(format, samples)) {
            return Ok(p.clone());
        }
        let p = Arc::new(build_pipelines(
            &gpu.device,
            &self.module,
            &self.layout,
            format,
            samples,
        )?);
        cache.insert((format, samples), p.clone());
        Ok(p)
    }

    /// Draws the batches into a render pass whose colour attachment is `format` with `samples`
    /// samples (a [`crate::gpu::RenderTarget`]'s `format`/`samples`) and that has the scene's
    /// depth.  `view_proj` is [`GameCamera::gpu_view_proj`](crate::camera::GameCamera::gpu_view_proj).
    /// Batches are drawn in ascending priority, stable within a priority.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_into(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        format: wgpu::TextureFormat,
        samples: u32,
        view_proj: &Mat4,
        batches: &[FxBatch],
        ground: Option<&crate::terrain::TerrainMap>,
    ) -> Result<()> {
        let pipelines = self.pipelines_for(gpu, format, samples)?;
        let order: Vec<&FxBatch> = {
            let mut v: Vec<&FxBatch> = batches
                .iter()
                .filter(|b| !b.vertices.is_empty() && !b.indices.is_empty())
                .collect();
            v.sort_by_key(|b| b.priority);
            v
        };
        if order.is_empty() {
            return Ok(());
        }
        // One vertex buffer and one index buffer for the whole frame.
        let mut verts: Vec<FxGpuVertex> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        let mut ranges = Vec::with_capacity(order.len());
        for b in &order {
            let base = verts.len() as u32;
            verts.extend(b.vertices.iter().copied().map(|v| {
                let clearance = if b.ground_fade {
                    ground.map_or(GROUND_FADE_HEIGHT, |map| {
                        v.pos[2] - map.elevation_at(v.pos[0], v.pos[1])
                    })
                } else {
                    GROUND_FADE_HEIGHT
                };
                FxGpuVertex::new(
                    v,
                    clearance,
                    b.ground_fade
                        && matches!(
                            b.blend,
                            FxBlend::Additive | FxBlend::OneAlpha | FxBlend::Premultiplied
                        ),
                )
            }));
            let first = idx.len() as u32;
            idx.extend(b.indices.iter().map(|i| base + i));
            ranges.push(first..idx.len() as u32);
        }
        let vb = gpu.buffer(
            "fx vertices",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&verts),
        );
        let ib = gpu.buffer(
            "fx indices",
            wgpu::BufferUsages::INDEX,
            bytemuck::cast_slice(&idx),
        );
        let globals = gpu.buffer(
            "fx globals",
            wgpu::BufferUsages::UNIFORM,
            bytemuck::cast_slice(&view_proj.0),
        );
        let bg_globals = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx globals"),
            layout: &self.bgl_globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        pass.set_bind_group(0, &bg_globals, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
        for (b, range) in order.iter().zip(ranges) {
            pass.set_pipeline(&pipelines[slot(b.blend, b.z_write, b.z_compare)]);
            let tex = b
                .texture
                .and_then(|t| self.textures.get(t.0))
                .unwrap_or(&self.white);
            pass.set_bind_group(1, if b.clamp { &tex.clamp } else { &tex.wrap }, &[]);
            pass.draw_indexed(range, 0, 0..1);
        }
        Ok(())
    }
}

/// Smoothly fade explosion geometry from the terrain surface to full opacity over 32 world units.
const GROUND_FADE_HEIGHT: f32 = 32.0;

/// The effect pipelines (one per blend/depth slot) for one colour format and sample count.
fn build_pipelines(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    samples: u32,
) -> Result<Vec<wgpu::RenderPipeline>> {
    let attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 20,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 36,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 40,
            shader_location: 4,
        },
    ];
    let vb = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<FxGpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attrs,
    };
    // display values into an sRGB target: decode in the shader (`to_target`)
    let srgb = [("SRGB_TARGET", if format.is_srgb() { 1.0 } else { 0.0 })];
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut pipelines = Vec::new();
    for blend in FxBlend::ALL {
        for z_write in [false, true] {
            for z_compare in [false, true] {
                let targets = [Some(wgpu::ColorTargetState {
                    format,
                    blend: blend.state(),
                    write_mask: wgpu::ColorWrites::COLOR,
                })];
                pipelines.push(
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("fx"),
                        layout: Some(layout),
                        vertex: wgpu::VertexState {
                            module,
                            entry_point: Some("vs_fx"),
                            compilation_options: Default::default(),
                            buffers: &[Some(vb.clone())],
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            // Effects are two-sided (backface culling is off in their state).
                            cull_mode: None,
                            ..Default::default()
                        },
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: DEPTH_FORMAT,
                            depth_write_enabled: Some(z_write),
                            depth_compare: Some(if z_compare {
                                wgpu::CompareFunction::LessEqual
                            } else {
                                wgpu::CompareFunction::Always
                            }),
                            stencil: wgpu::StencilState::default(),
                            bias: Default::default(),
                        }),
                        multisample: wgpu::MultisampleState {
                            count: samples,
                            mask: !0,
                            alpha_to_coverage_enabled: false,
                        },
                        fragment: Some(wgpu::FragmentState {
                            module,
                            entry_point: Some("fs_fx"),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &srgb,
                                zero_initialize_workgroup_memory: false,
                            },
                            targets: &targets,
                        }),
                        multiview_mask: None,
                        cache: None,
                    }),
                );
            }
        }
    }
    if let Some(err) = pollster::block_on(scope.pop()) {
        return Err(RenderError::Gpu(format!(
            "fx pipeline creation failed: {err}"
        )));
    }
    Ok(pipelines)
}

/// Placeholder bind group used only while the renderer is being constructed.
fn dummy_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fx placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fx placeholder"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod ground_fade_tests {
    use super::*;
    use crate::gpu::Gpu;
    use mc2_assets::map::TerrainVertex;
    use mc2_gfx::config::RenderConfig;

    #[test]
    fn terrain_crossing_billboard_fades_at_the_fragment_intersection() {
        let gpu = match Gpu::headless(&RenderConfig::default()) {
            Ok(gpu) => gpu,
            Err(error)
                if std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
            {
                eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({error})");
                return;
            }
            Err(error) => panic!("terrain crossing FX render needs a GPU adapter: {error}"),
        };
        let renderer = FxRenderer::new(&gpu).unwrap();
        let map = crate::terrain::TerrainMap::new(
            2,
            (0..4)
                .map(|_| TerrainVertex {
                    normal: [0.0, 0.0, 1.0],
                    elevation: 0.0,
                    texture_data: 0,
                    local_rgb_light: 0,
                    terrain_type: 0,
                    selected: 0,
                    water: 0,
                    shadow: 0,
                    highlighted: 0,
                })
                .collect(),
        );
        let v = |x, z| FxVertex {
            pos: [x, 0.0, z],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        };
        let mut billboard = FxBatch {
            texture: None,
            blend: FxBlend::Additive,
            z_write: false,
            z_compare: false,
            clamp: false,
            ground_fade: true,
            priority: 0,
            vertices: vec![v(-0.8, -16.0), v(0.8, -16.0), v(0.8, 16.0), v(-0.8, 16.0)],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        // Map world Z to clip Y so the card crosses flat terrain at the image center.
        let view_proj = Mat4::from_rows([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0 / 16.0, 0.0],
            [0.0, 1.0, 0.0, 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let faded = renderer
            .render_offscreen_with_ground(
                &gpu,
                65,
                65,
                [0.0; 3],
                &view_proj,
                std::slice::from_ref(&billboard),
                Some(&map),
            )
            .unwrap();
        let center = &faded[((32 * 65 + 32) * 4) as usize..][..3];
        assert!(
            center.iter().all(|channel| *channel <= 1),
            "intersection pixel was {center:?}"
        );
        let above = &faded[((24 * 65 + 32) * 4) as usize..][..3];
        assert!(
            above.iter().any(|channel| *channel > 10),
            "billboard did not fade in above the ground: {above:?}"
        );

        billboard.ground_fade = false;
        let sharp = renderer
            .render_offscreen_with_ground(
                &gpu,
                65,
                65,
                [0.0; 3],
                &view_proj,
                &[billboard],
                Some(&map),
            )
            .unwrap();
        let sharp_center = &sharp[((32 * 65 + 32) * 4) as usize..][..3];
        assert!(
            sharp_center.iter().all(|channel| *channel > 200),
            "control billboard did not cover the intersection: {sharp_center:?}"
        );
    }
}

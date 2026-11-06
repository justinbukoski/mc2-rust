// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The GUI's 2D wgpu pipeline (written fresh; the original drew through GameOS'
//! `gos_DrawQuads` with `gos_State_AlphaMode`, `gos_State_Texture`, `gos_FilterNone`).
//!
//! [`UiRenderer::draw`] records one render pass that draws a [`DrawList`] into a caller-provided
//! [`RenderTarget`] with `LoadOp::Load`, on the caller's encoder. It never creates a device and
//! never submits: textures and buffers are written through the shared queue, and the app submits
//! the frame.
//!
//! **Resolution independence.** Widgets are laid out in the logical UI space the original used
//! (800x600 for the menus, see [`UiLayout`]); the renderer maps that space onto the target with
//! one uniform scale, centred (letterbox or pillarbox), with the offset rounded to whole pixels.
//! Art is drawn sharp-bilinear (see `shaders/ui.wgsl`), text at whole UI pixels, so 1080p, 4K and
//! 6K targets all stay crisp. Every texture goes through `Gpu::texture_2d` and therefore the
//! hi-res replacement hook (`MC2_HIRES_DIR`, key from the texture's data path), except streamed
//! movie frames (`TextureStore::insert_streamed`), which upload raw; UVs are normalised,
//! so a replacement of any size lines up.

use crate::ctx::UiAssets;
use crate::draw::{Blend, DrawList, GuiRect};
use crate::textures::TexId;
use mc2_render::gpu::{Gpu, OffscreenTarget};
use std::collections::HashMap;

const SHADER: &str = include_str!("shaders/ui.wgsl");

/// How the logical UI space is fitted into the target.
/// Drawing uses the target's non-sRGB `display_view` to preserve original gamma-space blending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// Largest uniform scale that fits, centred.
    #[default]
    Fit,
    /// Largest whole-number scale that fits (at least 1), centred: exact pixel multiples.
    Integer,
}

/// Mapping from UI coordinates to target pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiLayout {
    pub ui_size: (u32, u32),
    pub target_size: (u32, u32),
    pub scale: f32,
    pub offset: (f32, f32),
}

impl UiLayout {
    pub fn new(ui_size: (u32, u32), target_size: (u32, u32), mode: ScaleMode) -> UiLayout {
        let (uw, uh) = (ui_size.0.max(1) as f32, ui_size.1.max(1) as f32);
        let (tw, th) = (target_size.0.max(1) as f32, target_size.1.max(1) as f32);
        let mut scale = (tw / uw).min(th / uh);
        if mode == ScaleMode::Integer {
            scale = scale.floor().max(1.0);
        }
        let offset = (
            ((tw - uw * scale) / 2.0).round(),
            ((th - uh * scale) / 2.0).round(),
        );
        UiLayout {
            ui_size,
            target_size,
            scale,
            offset,
        }
    }

    /// UI point -> target pixel.
    pub fn to_target(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.offset.0 + x * self.scale,
            self.offset.1 + y * self.scale,
        )
    }

    /// Target pixel -> UI point.
    pub fn to_ui(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.offset.0) / self.scale,
            (y - self.offset.1) / self.scale,
        )
    }

    /// A UI rectangle as a scissor rectangle in target pixels, clamped to the target.
    pub fn scissor(&self, r: GuiRect) -> (u32, u32, u32, u32) {
        let (x0, y0) = self.to_target(r.left as f32, r.top as f32);
        let (x1, y1) = self.to_target(r.right as f32, r.bottom as f32);
        let (tw, th) = (self.target_size.0 as f32, self.target_size.1 as f32);
        let x0 = x0.floor().clamp(0.0, tw);
        let y0 = y0.floor().clamp(0.0, th);
        let x1 = x1.ceil().clamp(x0, tw);
        let y1 = y1.ceil().clamp(y0, th);
        (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    target_size: [f32; 2],
    srgb_target: f32,
    _pad: f32,
}

struct GpuTexture {
    version: u32,
    bind_group: wgpu::BindGroup,
    _texture: wgpu::Texture,
}

pub struct UiRenderer {
    bgl_globals: wgpu::BindGroupLayout,
    bgl_texture: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pipelines: HashMap<(wgpu::TextureFormat, u32, Blend), wgpu::RenderPipeline>,
    textures: HashMap<TexId, GpuTexture>,
    white: wgpu::BindGroup,
    _white_tex: wgpu::Texture,
}

fn blend_state(b: Blend) -> Option<wgpu::BlendState> {
    let keep_alpha = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    match b {
        Blend::Alpha => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: keep_alpha,
        }),
        Blend::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: keep_alpha,
        }),
        Blend::Opaque => None,
    }
}

/// A `gos_VERTEX` `argb` (`0xAARRGGBB`, `GameOS/include/gameos.hpp:2157`) as the vertex bytes.
pub(crate) fn argb_to_rgba(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

impl UiRenderer {
    pub fn new(gpu: &Gpu) -> UiRenderer {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bgl_globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let bgl_texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui texture"),
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
            label: Some("ui"),
            bind_group_layouts: &[Some(&bgl_globals), Some(&bgl_texture)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let white_tex = gpu.texture_2d("mc2-ui white", 1, 1, &[255, 255, 255, 255], false);
        let white = Self::bind_texture(device, &bgl_texture, &sampler, &white_tex);
        UiRenderer {
            bgl_globals,
            bgl_texture,
            layout,
            module,
            sampler,
            pipelines: HashMap::new(),
            textures: HashMap::new(),
            white,
            _white_tex: white_tex,
        }
    }

    fn bind_texture(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        tex: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = tex.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui texture"),
            layout: bgl,
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

    fn pipeline(&mut self, gpu: &Gpu, format: wgpu::TextureFormat, samples: u32, blend: Blend) {
        let key = (format, samples, blend);
        if self.pipelines.contains_key(&key) {
            return;
        }
        let attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Unorm8x4,
                offset: 16,
                shader_location: 2,
            },
        ];
        let p = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ui"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GpuVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples.max(1),
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: blend_state(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        self.pipelines.insert(key, p);
    }

    /// Uploads new or changed textures (through the hi-res replacement hook).
    fn sync_textures(&mut self, gpu: &Gpu, assets: &UiAssets, list: &DrawList) {
        for b in &list.batches {
            let Some(id) = b.texture else { continue };
            let Some(img) = assets.textures.get(id) else {
                continue;
            };
            if self
                .textures
                .get(&id)
                .is_some_and(|t| t.version == img.version)
            {
                continue;
            }
            // A streamed frame skips the hi-res lookup: the pack never holds a per-frame entry.
            let tex = if img.streamed {
                gpu.texture_2d_raw(&img.name, img.width, img.height, &img.rgba, false)
            } else {
                gpu.texture_2d(&img.name, img.width, img.height, &img.rgba, false)
            };
            let bind_group =
                Self::bind_texture(&gpu.device, &self.bgl_texture, &self.sampler, &tex);
            self.textures.insert(
                id,
                GpuTexture {
                    version: img.version,
                    bind_group,
                    _texture: tex,
                },
            );
        }
    }

    /// Records the draw list into `target` (loading, not clearing, what is there).
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &mc2_render::gpu::RenderTarget,
        layout: &UiLayout,
        list: &DrawList,
        assets: &UiAssets,
    ) {
        if list.vertices.is_empty() {
            return;
        }
        self.sync_textures(gpu, assets, list);
        let view = &target.display_view;
        let mut used = Vec::new();
        for b in &list.batches {
            if !used.contains(&b.blend) {
                used.push(b.blend);
            }
        }
        for &b in &used {
            self.pipeline(gpu, target.display_format, 1, b);
        }
        let mut verts: Vec<GpuVertex> = list
            .vertices
            .iter()
            .map(|v| {
                let (x, y) = layout.to_target(v.x, v.y);
                GpuVertex {
                    pos: [x, y],
                    uv: [v.u, v.v],
                    color: argb_to_rgba(v.argb),
                }
            })
            .collect();
        for b in &list.batches {
            let Some(img) = b.texture.and_then(|t| assets.textures.get(t)) else {
                continue;
            };
            let dims = (img.width as f32, img.height as f32);
            let first = b.first_quad as usize * 4;
            let end = first + b.quad_count as usize * 4;
            for q in verts[first..end].chunks_exact_mut(4) {
                inset_edges(q, dims);
            }
        }
        let quads = list.quad_count() as u32;
        let mut indices = Vec::with_capacity(quads as usize * 6);
        for q in 0..quads {
            let b = q * 4;
            indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        let vb = gpu.buffer(
            "ui vertices",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&verts),
        );
        let ib = gpu.buffer(
            "ui indices",
            wgpu::BufferUsages::INDEX,
            bytemuck::cast_slice(&indices),
        );
        let globals = Globals {
            target_size: [layout.target_size.0 as f32, layout.target_size.1 as f32],
            srgb_target: if target.display_format.is_srgb() {
                1.0
            } else {
                0.0
            },
            _pad: 0.0,
        };
        let ub = gpu.buffer(
            "ui globals",
            wgpu::BufferUsages::UNIFORM,
            bytemuck::bytes_of(&globals),
        );
        let bg_globals = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui globals"),
            layout: &self.bgl_globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &bg_globals, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
        // everything stays inside the fitted UI area (art placed off-screen in a layout must not
        // spill into the letterbox bars); batch clips narrow it further
        let ui_rect = GuiRect::new(0, 0, layout.ui_size.0 as i32, layout.ui_size.1 as i32);
        for b in &list.batches {
            let clip = b.clip.map_or(ui_rect, |c| c.intersect(&ui_rect));
            let (sx, sy, sw, sh) = layout.scissor(clip);
            if sw == 0 || sh == 0 {
                continue;
            }
            pass.set_scissor_rect(sx, sy, sw, sh);
            pass.set_pipeline(&self.pipelines[&(target.display_format, 1, b.blend)]);
            let bg = match b.texture.and_then(|t| self.textures.get(&t)) {
                Some(t) => &t.bind_group,
                None => &self.white,
            };
            pass.set_bind_group(1, bg, &[]);
            pass.draw_indexed(b.first_quad * 6..(b.first_quad + b.quad_count) * 6, 0, 0..1);
        }
    }
}

/// Texels a textured quad's UV edges are kept inside its own texel range, per axis.
///
/// The original adds `0.1 / fileWidth` to every UV, so a quad samples 0.1 texel past its far edge
/// and only 0.1 texel into its near one. At 1:1 the shader snaps to texel centres and never notices.
/// Once the UI is upscaled, the screen pixels along the edges sit at texel fractions of 0.1 / 0.9,
/// where the bilinear tap still mixes in 40 % of the neighbouring atlas texel: a 1 px stray line
/// (the next cell's border art) along the quad's edges. Keeping the edge samples at a fraction of
/// 0.4 / 0.6 makes the tap a pure texel centre; the interior shifts by under half a texel.
const EDGE_INSET: f32 = 0.4;
/// The original's `0.1 / fileWidth` bias, already in the near edge.
const ORIGINAL_BIAS: f32 = 0.1;

fn inset_edges(quad: &mut [GpuVertex], dims: (f32, f32)) {
    for (axis, dim) in [(0usize, dims.0), (1, dims.1)] {
        let lo = quad.iter().map(|v| v.uv[axis]).fold(f32::MAX, f32::min);
        let hi = quad.iter().map(|v| v.uv[axis]).fold(f32::MIN, f32::max);
        // a sliver or a degenerate quad (a flat colour, a 1-texel stripe) has nothing to trim
        if (hi - lo) * dim < 2.0 {
            continue;
        }
        // only quads that carry the original's bias; glyph quads use exact texel UVs and must
        // keep sampling them untouched
        let frac = lo * dim - (lo * dim).floor();
        if (frac - ORIGINAL_BIAS).abs() > 0.02 {
            continue;
        }
        for v in quad.iter_mut() {
            if v.uv[axis] == hi {
                // the far edge ends `0.1` past the texel boundary: land at `boundary - 0.4`
                v.uv[axis] -= (ORIGINAL_BIAS + EDGE_INSET) / dim;
            } else if v.uv[axis] == lo {
                v.uv[axis] += (EDGE_INSET - ORIGINAL_BIAS) / dim;
            }
        }
    }
}

/// Convenience API for headless rendering, backed by `mc2-render`'s real target.
pub struct Offscreen {
    inner: OffscreenTarget,
}

impl Offscreen {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Offscreen {
        Offscreen {
            inner: OffscreenTarget::new(gpu, width, height),
        }
    }

    pub fn target(&self) -> mc2_render::gpu::RenderTarget {
        self.inner.target()
    }

    /// Clears to `rgba` (a pass on the encoder).
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder, rgba: [f64; 4]) {
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.inner.target().display_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: rgba[0],
                        g: rgba[1],
                        b: rgba[2],
                        a: rgba[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    /// Submits `encoder` plus a copy and returns tightly packed RGBA8 pixels.
    pub fn read_rgba(&self, gpu: &Gpu, encoder: wgpu::CommandEncoder) -> Result<Vec<u8>, String> {
        self.inner
            .target()
            .read_rgba(gpu, encoder)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gv(u: f32, v: f32) -> GpuVertex {
        GpuVertex {
            pos: [0.0, 0.0],
            uv: [u, v],
            color: [255; 4],
        }
    }

    #[test]
    fn edges_are_kept_inside_the_quads_own_texels() {
        // a 128x64 button cell of a 256x256 atlas with the original's +0.1 texel bias
        let b = 0.1 / 256.0;
        let (l, t, r, bt) = (b, 128.0 / 256.0 + b, 128.0 / 256.0 + b, 192.0 / 256.0 + b);
        let mut q = [gv(l, t), gv(l, bt), gv(r, bt), gv(r, t)];
        inset_edges(&mut q, (256.0, 256.0));
        let texel = |u: f32| u * 256.0;
        // texel fractions 0.4 (near) and 0.6 (far): the bilinear tap is a pure texel centre
        assert!((texel(q[0].uv[0]) - 0.4).abs() < 1e-3);
        assert!((texel(q[0].uv[1]) - 128.4).abs() < 1e-3);
        assert!((texel(q[2].uv[0]) - 127.6).abs() < 1e-3);
        assert!((texel(q[2].uv[1]) - 191.6).abs() < 1e-3);
    }

    #[test]
    fn unbiased_glyph_uvs_are_left_alone() {
        // AgencyFB-style glyph cell: exact texel boundaries, no +0.1 bias
        let (l, t, r, b) = (13.0 / 256.0, 28.0 / 256.0, 26.0 / 256.0, 42.0 / 256.0);
        let mut q = [gv(l, t), gv(l, b), gv(r, b), gv(r, t)];
        let before = q.map(|v| v.uv);
        inset_edges(&mut q, (256.0, 256.0));
        assert_eq!(q.map(|v| v.uv), before);
    }

    #[test]
    fn sub_texel_and_flat_quads_are_left_alone() {
        let mut q = [gv(0.0, 0.0), gv(0.0, 0.0), gv(0.0, 0.0), gv(0.0, 0.0)];
        inset_edges(&mut q, (256.0, 256.0));
        assert!(q.iter().all(|v| v.uv == [0.0, 0.0]));
    }

    #[test]
    fn layouts_fit_and_centre() {
        let l = UiLayout::new((800, 600), (1024, 768), ScaleMode::Fit);
        assert_eq!((l.scale, l.offset), (1.28, (0.0, 0.0)));
        let l = UiLayout::new((800, 600), (3840, 2160), ScaleMode::Fit);
        assert_eq!(l.scale, 3.6);
        assert_eq!(l.offset, (480.0, 0.0));
        assert_eq!(l.to_target(800.0, 600.0), (3360.0, 2160.0));
        assert_eq!(l.to_ui(480.0, 0.0), (0.0, 0.0));
        let l = UiLayout::new((800, 600), (3840, 2160), ScaleMode::Integer);
        assert_eq!((l.scale, l.offset), (3.0, (720.0, 180.0)));
        let l = UiLayout::new((800, 600), (6016, 3384), ScaleMode::Fit);
        assert_eq!(l.scale, 5.64);
        assert_eq!(
            l.scissor(GuiRect::new(0, 0, 800, 600)),
            (752, 0, 4512, 3384)
        );
    }
}

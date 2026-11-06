//! A minimal 2D painter for the app's own overlays (the stand-in cursor, loading and results
//! placeholders): solid-colour triangles in pixel coordinates, alpha blended over the frame.
//! Platform code, written fresh against the wgpu API.

use bytemuck::{Pod, Zeroable};
use mc2_render::gpu::Gpu;

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.color = color;
    return o;
}

// Colours are display values; an sRGB target stores them re-encoded, so decode first.
override SRGB_TARGET: bool = false;

@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    if (SRGB_TARGET) {
        let c = i.color.rgb;
        let lin = select(pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
        return vec4<f32>(lin, i.color.a);
    }
    return i.color;
}
"#;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
}

/// Triangles collected for one pass, in pixel coordinates (origin top-left).
#[derive(Default)]
pub struct OverlayBatch {
    tris: Vec<OverlayTri>,
}

/// Three corners and a colour.
pub type OverlayTri = ([f32; 2], [f32; 2], [f32; 2], [f32; 4]);

impl OverlayBatch {
    pub fn triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [f32; 4]) {
        self.tris.push((a, b, c, color));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.triangle([x, y], [x + w, y], [x + w, y + h], color);
        self.triangle([x, y], [x + w, y + h], [x, y + h], color);
    }

    /// A one-pixel-thick axis-aligned line in the same pixel space as [`Self::rect`].
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, thickness: f32, color: [f32; 4]) {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        let t = thickness.max(1.0);
        self.rect(x0, y, (x1 - x0).max(t), t, color);
    }

    /// A one-pixel-thick axis-aligned line in the same pixel space as [`Self::rect`].
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, thickness: f32, color: [f32; 4]) {
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        let t = thickness.max(1.0);
        self.rect(x, y0, t, (y1 - y0).max(t), color);
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    /// The three corners of every triangle with its colour (the geometry the batch draws).
    pub fn triangles(&self) -> &[OverlayTri] {
        &self.tris
    }

    /// Display colours of each triangle, in the order they were submitted.
    pub fn colors(&self) -> Vec<[f32; 4]> {
        self.tris.iter().map(|t| t.3).collect()
    }
}

pub struct OverlayPainter {
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
}

impl OverlayPainter {
    /// A painter for targets in `format` (a [`mc2_render::gpu::RenderTarget::format`]: colours
    /// are given in display terms and blended in the target's space).
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 8,
                shader_location: 1,
            },
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[("SRGB_TARGET", if format.is_srgb() { 1.0 } else { 0.0 })],
                    zero_initialize_workgroup_memory: false,
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, format }
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Draws `batch` over `view` (`size` in pixels).  `clear` clears the view first.
    pub fn draw(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: (u32, u32),
        batch: &OverlayBatch,
        clear: Option<[f32; 4]>,
    ) {
        if batch.is_empty() && clear.is_none() {
            return;
        }
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let ndc = |p: [f32; 2]| [p[0] / w * 2.0 - 1.0, 1.0 - p[1] / h * 2.0];
        let mut verts = Vec::with_capacity(batch.tris.len() * 3);
        for (a, b, c, color) in &batch.tris {
            for p in [a, b, c] {
                verts.push(Vertex {
                    pos: ndc(*p),
                    color: *color,
                });
            }
        }
        let vb = gpu.buffer(
            "overlay vertices",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&verts),
        );
        let load = match clear {
            Some(c) => wgpu::LoadOp::Clear(mc2_render::gpu::clear_color(
                [c[0], c[1], c[2]],
                c[3] as f64,
                self.format,
            )),
            None => wgpu::LoadOp::Load,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !verts.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.draw(0..verts.len() as u32, 0..1);
        }
    }
}

/// The stand-in pointer (until `mc2-ui` draws the game's cursor sprites): an arrow with a dark
/// outline, or a diamond while the camera button rotates the view.  `scale` is native pixels per
/// game pixel.
pub fn cursor_shape(batch: &mut OverlayBatch, x: f32, y: f32, scale: f32, rotate: bool) {
    let s = scale.max(0.5);
    let dark = [0.0, 0.0, 0.0, 0.85];
    let light = [0.95, 0.95, 0.85, 1.0];
    if rotate {
        for (r, c) in [(9.0, dark), (7.0, light)] {
            let r = r * s;
            batch.triangle([x, y - r], [x + r, y], [x, y + r], c);
            batch.triangle([x, y - r], [x, y + r], [x - r, y], c);
        }
        return;
    }
    let arrow = |b: &mut OverlayBatch, k: f32, o: f32, c: [f32; 4]| {
        let p = |dx: f32, dy: f32| [x + (dx * k - o) * s, y + (dy * k - o) * s];
        b.triangle(p(0.0, 0.0), p(0.0, 16.0), p(11.0, 11.0), c);
        b.triangle(p(3.0, 12.0), p(6.0, 11.0), p(9.0, 19.0), c);
    };
    arrow(batch, 1.15, 1.0, dark);
    arrow(batch, 1.0, 0.0, light);
}

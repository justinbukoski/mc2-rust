//! Presenting a frame rendered at one size on a target of another: the scaled fullscreen of a Resolution the
//! monitor has no display mode for.  The game renders into an offscreen frame of the chosen size (its
//! effective rendering dimensions) and [`ScaledPresenter::present`] stretches it over the surface.

use crate::gpu::{Gpu, OffscreenTarget, RenderTarget};

const SHADER: &str = "
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
";

/// The chosen-size frame and the pass that stretches it over a larger or smaller target.
pub struct ScaledPresenter {
    frame: Option<OffscreenTarget>,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
}

impl Default for ScaledPresenter {
    fn default() -> Self {
        Self::new()
    }
}

impl ScaledPresenter {
    pub fn new() -> Self {
        Self {
            frame: None,
            pipeline: None,
            layout: None,
            sampler: None,
        }
    }

    /// The frame to render the game into, `width` x `height` (the chosen resolution), in the device's display format.
    pub fn frame(&mut self, gpu: &Gpu, width: u32, height: u32) -> RenderTarget {
        let want = (width.max(1), height.max(1));
        match &mut self.frame {
            Some(f) if f.size() == want && f.format() == gpu.target_format => {}
            slot => {
                *slot = Some(OffscreenTarget::with_format(
                    gpu,
                    want.0,
                    want.1,
                    gpu.target_format,
                ))
            }
        }
        self.frame
            .as_ref()
            .map(OffscreenTarget::target)
            .expect("frame was just made")
    }

    /// The size of the chosen-size frame, once one was rendered.
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        self.frame.as_ref().map(OffscreenTarget::size)
    }

    /// Stretches the chosen-size frame over all of `target` (single-sample draw into its display view).
    pub fn present(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
    ) {
        let Some(frame) = &self.frame else { return };
        let device = &gpu.device;
        let layout = self.layout.get_or_insert_with(|| {
            let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty,
                count: None,
            };
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("scaled present"),
                entries: &[
                    entry(
                        0,
                        wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    ),
                    entry(
                        1,
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    ),
                ],
            })
        });
        let sampler = self.sampler.get_or_insert_with(|| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scaled present"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        });
        let format = target.display_format;
        if self.pipeline.as_ref().map(|p| p.0) != Some(format) {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("scaled present"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("scaled present"),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scaled present"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.pipeline = Some((format, pipeline));
        }
        let view = frame.sampled_view();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scaled present"),
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
        });
        let Some((_, pipeline)) = &self.pipeline else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scaled present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.display_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

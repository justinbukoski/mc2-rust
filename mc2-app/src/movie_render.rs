//! Full-screen movie quad for the production UI host.

use crate::layers::DrawContext;
use mc2_platform::movie::VideoFrame;
use mc2_render::gpu::Gpu;
use mc2_ui::render::{ScaleMode, UiLayout};

#[derive(Clone, Copy, Debug)]
pub enum MoviePlacement {
    FullScreen,
    LogisticsCutscene,
    /// A rectangle in the 800x600 mission selection UI coordinate space.
    MissionSelectVidCom {
        rect: (f32, f32, f32, f32),
    },
}

const SHADER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let points = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = points[index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, in.uv);
}
"#;

struct Image {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// Uploads decoded RGBA movie frames and covers the current screen with them.
pub struct MovieRenderer {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    image: Option<Image>,
}

impl MovieRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("movie image layout"),
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
            label: Some("movie sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            sampler,
            pipeline: None,
            image: None,
        }
    }

    pub fn draw(
        &mut self,
        ctx: &mut DrawContext<'_>,
        frame: &VideoFrame,
        placement: MoviePlacement,
    ) {
        let rgba = frame.to_rgba();
        if self
            .image
            .as_ref()
            .is_none_or(|image| image.size != (frame.width, frame.height))
        {
            let size = wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            };
            let texture = ctx.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("movie frame"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = ctx
                .gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("movie frame bindings"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            self.image = Some(Image {
                _texture: texture,
                view,
                bind_group,
                size: (frame.width, frame.height),
            });
        }
        let image = self.image.as_ref().expect("movie frame texture created");
        ctx.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &image._texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
        let format = ctx.target.display_format;
        if self.pipeline.as_ref().is_none_or(|(old, _)| *old != format) {
            let device = &ctx.gpu.device;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("movie quad shader"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("movie quad pipeline layout"),
                bind_group_layouts: &[Some(&self.layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("movie quad pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
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
        let (_, pipeline) = self.pipeline.as_ref().expect("movie pipeline created");
        let _keep_view_alive = &image.view;
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("movie frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ctx.target.display_view,
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &image.bind_group, &[]);
        let viewport = match placement {
            MoviePlacement::FullScreen => (0.0, 0.0, ctx.size.0 as f32, ctx.size.1 as f32),
            MoviePlacement::LogisticsCutscene => {
                let scale_y = ctx.size.1 as f32 / ctx.game_size.1.max(1.0);
                (0.0, 100.0 * scale_y, ctx.size.0 as f32, 400.0 * scale_y)
            }
            MoviePlacement::MissionSelectVidCom { rect } => {
                let layout = UiLayout::new((800, 600), ctx.size, ScaleMode::Fit);
                let (left, top) = layout.to_target(rect.0, rect.1);
                let (right, bottom) = layout.to_target(rect.0 + rect.2, rect.1 + rect.3);
                let bounds = (left, top, right - left, bottom - top);
                let scale = (bounds.2 / frame.width.max(1) as f32)
                    .min(bounds.3 / frame.height.max(1) as f32);
                let width = frame.width as f32 * scale;
                let height = frame.height as f32 * scale;
                (
                    bounds.0 + (bounds.2 - width) * 0.5,
                    bounds.1 + (bounds.3 - height) * 0.5,
                    width,
                    height,
                )
            }
        };
        pass.set_viewport(viewport.0, viewport.1, viewport.2, viewport.3, 0.0, 1.0);
        pass.draw(0..3, 0..1);
    }
}

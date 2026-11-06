// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The GPU scene: terrain, water and shape pipelines and the frame renderer.
//!
//! Draw order of the original (`GameCamera::render`): terrain solid, objects, then alpha
//! (water, windows).  Terrain tiles use the colour map plus detail bitmap with the cement tile
//! layer on top; shapes are instanced by model with one matrix per node; water follows.  An
//! optional sun shadow map (cast by the shapes) is a modern addition: the original projected
//! flattened shadow shapes onto the terrain.

use crate::appearance::{AppearanceState, ShapeChoice, ShapeSet};
use crate::camera::GameCamera;
use crate::error::{RenderError, Result};
use crate::fx::{FxBatch, FxRenderer, FxTextureId};
use crate::game::GameData;
use crate::gpu::{Gpu, OffscreenTarget, RenderTarget, DEPTH_FORMAT};
use crate::light::{LightModel, LightUniform};
use crate::math::{Mat4, Vec3};
use crate::mission::{Mission, TerrainParams, WaterParams};
use crate::objects::GlowColors;
use crate::shape::{
    apply_paint_scheme, instance_node_matrices, texture_path, AnimationClip, NodeKind,
    PaintRoutine, ShapeModel, ShapeVertex,
};
use crate::terrain::{pack_tile_word, TerrainGpuVertex, TerrainMap, TerrainMesh, TileKind};
use crate::terrain_lights::{self, LightMap, TerrainLight};
use crate::terrain_tex::{TerrainTextures, TILE_SIZE};
use crate::visual::{self, VisualOptions};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SHADER: &str = include_str!("shaders/scene.wgsl");

/// The shader with every `@enhanced:begin` .. `@enhanced:end` block removed: what the renderer
/// compiled before the visual upgrades existed (test support, see [`SceneRenderer::new_baseline`]).
fn baseline_shader() -> String {
    let mut out = String::with_capacity(SHADER.len());
    let mut skipping = false;
    for line in SHADER.lines() {
        let t = line.trim();
        if t == "// @enhanced:begin" {
            assert!(!skipping, "nested @enhanced block");
            skipping = true;
        } else if t == "// @enhanced:end" {
            assert!(skipping, "@enhanced:end without begin");
            skipping = false;
        } else if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(!skipping, "unterminated @enhanced block");
    out
}

/// (colour format, sample count, visual options, variant) of a pipeline set.
type PipelineKey = (wgpu::TextureFormat, u32, VisualOptions, Variant);

/// Which pipeline set a cache entry is: the normal scene, the baseline (no enhanced code), or
/// the material-ID render of the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Variant {
    Scene,
    Baseline,
    Ids,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Globals {
    view_proj: [f32; 16],
    light_view_proj: [f32; 16],
    camera_pos: [f32; 4],
    terrain_dir: [f32; 4],
    shape_dir: [f32; 4],
    sun: [f32; 4],
    ambient: [f32; 4],
    fog: [f32; 4],
    fog_height: [f32; 4],
    map: [f32; 4],
    tiling: [f32; 4],
    water: [f32; 4],
    water_alpha: [f32; 4],
    haze: [f32; 4],
    shadow_info: [f32; 4],
    /// Enhanced water: cloud angle, spray offset, frame blend, glint strength.
    water_anim: [f32; 4],
}

/// Per-model uniform (`ModelParams` in the shader): the marker glow colours.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct ModelParams {
    pink: [f32; 4],
    yellow: [f32; 4],
    green: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct InstanceData {
    node_base: u32,
    flags: u32,
    /// `SetARGBHighLight` colour of the instance (0 = none).
    highlight: u32,
    _pad: u32,
    infinite: [f32; 4],
    ambient_override: [f32; 4],
}

/// Handle to a model uploaded with [`SceneRenderer::load_shape`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelId(pub usize);

/// How a model's texture is prepared (part of the cache key).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ShapeStyle {
    /// Colours of the marker vertices (night windows and lights).
    pub glow: GlowColors,
    /// Paint scheme `(red, green, blue)` for mechs and vehicles (`apply_paint_scheme`).
    pub paint: Option<(u32, u32, u32)>,
    /// The C++ routine the paint goes through: a vehicle keeps its mask's alpha.
    pub paint_routine: PaintRoutine,
}

/// Whether shape skins (mechs, vehicles, buildings) get a mip chain.  The C++ loads every shape texture with
/// `gosHint_DisableMipmap` (mech3d.cpp:1111-1372, 1799, 2923-2947; bdactor.cpp:618-3535; gvactor.cpp; genactor.cpp) and its GL
/// upload never generates a chain (`glGenerateMipmap` is commented out, gl_utils.cpp:170, 263).
pub const SHAPE_SKIN_MIPS: bool = false;

/// The sampler of a shape skin: bilinear min/mag (`gos_FilterBiLinear`, tgl.cpp:2562 -> `TFM_LINEAR`, gameos_graphics.cpp:1558;
/// gl_utils.cpp:283-284 sets MIN and MAG alike), repeat wrap, no mip filtering and no anisotropy (GL has none set).
pub fn shape_sampler_descriptor() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("shape sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        lod_min_clamp: 0.0,
        lod_max_clamp: 0.0,
        anisotropy_clamp: 1,
        ..Default::default()
    }
}

/// One placed shape.
#[derive(Clone, Debug)]
pub struct ShapeInstance {
    /// The model drawn this frame: the LOD or damage shape [`ShapeInstance::select_shape`] chose.
    pub model: ModelId,
    /// Every shape the object can show (its LODs and damage shapes).
    pub shapes: ShapeSet,
    /// Appearance state the shape is chosen from.
    pub state: AppearanceState,
    /// The shape choice carried between frames: what is shown, `currentLOD` and the status it was set for
    /// ([`crate::appearance::advance`]).
    pub choice: ShapeChoice,
    /// Node matrices of the pose (see [`ShapeModel::pose`]), Stuff frame, relative to the origin.
    pub pose: Vec<Mat4>,
    /// The model and state `pose` was computed for; [`crate::populate::select_shapes`] recomputes the pose
    /// when either changes.
    pub pose_key: Option<(ModelId, AppearanceState)>,
    /// A turret rotation `(rotational node id, rotation)` applied to whichever model is selected, so a
    /// LOD change keeps the turret pose ([`crate::populate::select_shapes`]).
    pub turret: Option<(String, crate::math::Quat)>,
    pub position: Vec3,
    /// Object rotation in degrees.
    pub yaw_degrees: f32,
    /// Colour of the infinite light for this object (`ramp(terrainLight)`).
    pub infinite_light: [f32; 3],
    /// Replaces the ambient light (trees feed their terrain ramp to the ambient slot).
    pub ambient_override: Option<[f32; 3]>,
    pub casts_shadow: bool,
    /// The appearance's `inView` for the frame ([`crate::populate::select_shapes`]).  An object out of view is not
    /// drawn, as the C++ `render` returns early (`bdactor.cpp:1508`).  Starts `true`: an instance that no camera has
    /// tested is drawn.
    pub in_view: bool,
    /// Retained `baseRootNodeDifference` for a mech selection box. C++ keeps this across
    /// out-of-view frames (`mech3d.cpp:2153-2160`); it is presentation state only.
    pub root_height_difference: f32,
    /// `SetARGBHighLight` colour added to every lit vertex (`0x00RRGGBB`; 0 = none): the building flash
    /// ([`crate::flash::BuildingFlash`]).
    pub highlight: u32,
}

/// The pixel work of a texture upload (`SceneRenderer::add_model`), in its order: the paint scheme
/// on the mask (`paint`, `None` for every other texture), then the alpha rule, which makes a texture
/// without an alpha flag opaque.  `routine` decides the alpha of the painted pixels.
pub fn prepare_texture_pixels(
    px: &mut [u8],
    paint: Option<(u32, u32, u32)>,
    routine: PaintRoutine,
    texture_alpha: bool,
) {
    if let Some((r, g, b)) = paint {
        apply_paint_scheme(px, r, g, b, routine);
    }
    if !texture_alpha {
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
}

struct BatchGpu {
    first: u32,
    count: u32,
    kind: NodeKind,
    /// Alpha-channel texture (`a_*`): drawn as a cut-out.
    alpha: bool,
    bind_group: wgpu::BindGroup,
}

struct ModelGpu {
    vb: wgpu::Buffer,
    batches: Vec<BatchGpu>,
    node_count: usize,
}

struct TerrainGpu {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    index_count: u32,
    water_ib: wgpu::Buffer,
    water_index_count: u32,
    bg_terrain: wgpu::BindGroup,
    /// Kept to rebuild `bg_terrain` when the night light map changes.
    views: [wgpu::TextureView; 3],
    /// Smooth per-pixel terrain normals for the enhanced lighting path.
    normal_view: wgpu::TextureView,
    bg_water: Vec<wgpu::BindGroup>,
    /// Enhanced water: `(from, to)` detail frame pairs for the cross-fade, adjacent frames only.
    bg_water_pairs: Vec<((usize, usize), wgpu::BindGroup)>,
    world_side: f32,
    top_left: (f32, f32),
    z_range: (f32, f32),
    detail_frame_rate: f32,
}

struct Pipelines {
    terrain: wgpu::RenderPipeline,
    water_base: wgpu::RenderPipeline,
    water_detail: wgpu::RenderPipeline,
    shape_opaque: wgpu::RenderPipeline,
    shape_cutout: wgpu::RenderPipeline,
    shape_window: wgpu::RenderPipeline,
    shape_spot: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    shadow_cutout: wgpu::RenderPipeline,
}

/// Everything that changes between frames.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub camera: &'a GameCamera,
    pub light: &'a LightModel,
    pub water: &'a WaterParams,
    pub terrain_params: &'a TerrainParams,
    /// Seconds since mission start; drives the water animation.
    pub time: f32,
    /// `time` before its rounding to f32, when the caller has it.  Only the enhanced water reads
    /// it (phases are reduced in f64); `None` means `f64::from(time)`.
    pub precise_time: Option<f64>,
    pub instances: &'a [ShapeInstance],
    /// Sun shadow map from the shapes.
    pub shadows: bool,
}

pub struct SceneRenderer {
    gpu: Arc<Gpu>,
    bgl_nodes: wgpu::BindGroupLayout,
    bgl_shape_tex: wgpu::BindGroupLayout,
    bgl_terrain: wgpu::BindGroupLayout,
    bgl_water: wgpu::BindGroupLayout,
    layouts: PipelineLayouts,
    /// Pipelines per (colour format, sample count) of the targets drawn into.
    pipelines: Mutex<HashMap<PipelineKey, Arc<Pipelines>>>,
    visual: VisualOptions,
    /// Built by [`SceneRenderer::new_baseline`]: shader without enhanced code, switches fixed off.
    baseline: bool,
    /// The night lights of the mission (`set_terrain_lights`): the point lights that can cast Local Shadows.
    local_lights: Vec<TerrainLight>,
    /// `useLocalShadows`: point lights cast shape shadows (`mclib/tgl.cpp:3049`).
    local_shadows: bool,
    globals: wgpu::Buffer,
    sampler_clamp: wgpu::Sampler,
    sampler_wrap: wgpu::Sampler,
    /// Shape skins: bilinear, level 0 only (see [`shape_sampler_descriptor`]).
    sampler_shape: wgpu::Sampler,
    shadow_size: u32,
    shadow_view: wgpu::TextureView,
    bgl_globals: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
    dummy_shadow_view: wgpu::TextureView,
    bg_main: wgpu::BindGroup,
    bg_shadow: wgpu::BindGroup,
    terrain: Option<TerrainGpu>,
    terrain_map: Option<TerrainMap>,
    models: Vec<ModelGpu>,
    model_cpu: Vec<ShapeModel>,
    /// Keyed by the lowercase name, the style, and the clip owner (see [`SceneRenderer::load_shape_owned`]).
    model_by_name: HashMap<(String, ShapeStyle, Option<String>), ModelId>,
    grey: wgpu::TextureView,
    fx: FxRenderer,
}

fn vertex_attr(format: wgpu::VertexFormat, offset: u64, location: u32) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        format,
        offset,
        shader_location: location,
    }
}

impl SceneRenderer {
    /// Builds the pipelines on a shared device (`Gpu` or `Arc<Gpu>`).
    pub fn new(gpu: impl Into<Arc<Gpu>>) -> Result<Self> {
        Self::build(gpu.into(), false)
    }

    /// Test support: a renderer that never constructs the enhanced pipelines.  Its shader is the
    /// scene shader with all enhanced code removed (not merely switched off by constants) and
    /// [`set_visual`](Self::set_visual) is ignored, so its output is what the switches-off
    /// production renderer must reproduce byte for byte on the same adapter.
    pub fn new_baseline(gpu: impl Into<Arc<Gpu>>) -> Result<Self> {
        Self::build(gpu.into(), true)
    }

    fn build(gpu: Arc<Gpu>, baseline: bool) -> Result<Self> {
        let device = &gpu.device;
        let samples = gpu.samples;
        let target_format = gpu.target_format;
        let source = if baseline {
            baseline_shader()
        } else {
            SHADER.to_owned()
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let tex_entry = |binding: u32, array: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: if array {
                    wgpu::TextureViewDimension::D2Array
                } else {
                    wgpu::TextureViewDimension::D2
                },
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let bgl_globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Globals>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // the night light map the shapes sample (`terrain_lights`)
                tex_entry(3, false),
                sampler_entry(4),
            ],
        });
        let bgl_terrain = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                tex_entry(0, false),
                tex_entry(1, false),
                tex_entry(2, true),
                sampler_entry(3),
                sampler_entry(4),
                tex_entry(5, false),
                tex_entry(6, false),
            ],
        });
        let bgl_water = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                tex_entry(0, false),
                tex_entry(1, false),
                sampler_entry(2),
                tex_entry(3, false),
            ],
        });
        let bgl_nodes = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nodes"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let bgl_shape_tex = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shape texture"),
            entries: &[
                tex_entry(0, false),
                sampler_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<ModelParams>() as u64
                        ),
                    },
                    count: None,
                },
            ],
        });

        let layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
            let g: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|b| Some(*b)).collect();
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &g,
                immediate_size: 0,
            })
        };
        let layout_terrain = layout("terrain", &[&bgl_globals, &bgl_terrain]);
        let layout_water = layout("water", &[&bgl_globals, &bgl_water]);
        let layout_shape = layout("shape", &[&bgl_globals, &bgl_nodes, &bgl_shape_tex]);
        let layout_shadow = layout("shadow", &[&bgl_globals, &bgl_nodes]);
        let layout_shadow_cutout =
            layout("shadow cutout", &[&bgl_globals, &bgl_nodes, &bgl_shape_tex]);

        let layouts = PipelineLayouts {
            module,
            terrain: layout_terrain,
            water: layout_water,
            shape: layout_shape,
            shadow: layout_shadow,
            shadow_cutout: layout_shadow_cutout,
        };
        // The device's default target configuration is built up front; other (format, samples)
        // pairs are built on first use.
        let first = Arc::new(build_pipelines(
            device,
            &layouts,
            target_format,
            samples,
            VisualOptions::default(),
            if baseline {
                Variant::Baseline
            } else {
                Variant::Scene
            },
        )?);
        let mut cache = HashMap::new();
        cache.insert(
            (
                target_format,
                samples,
                VisualOptions::default(),
                if baseline {
                    Variant::Baseline
                } else {
                    Variant::Scene
                },
            ),
            first,
        );

        let aniso = gpu.cfg.aniso.max(1);
        let make_sampler = |address: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene sampler"),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                lod_min_clamp: 0.0,
                lod_max_clamp: 32.0,
                anisotropy_clamp: aniso,
                ..Default::default()
            })
        };
        let sampler_clamp = make_sampler(wgpu::AddressMode::ClampToEdge);
        let sampler_wrap = make_sampler(wgpu::AddressMode::Repeat);
        let sampler_shape = device.create_sampler(&shape_sampler_descriptor());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let shadow_size = 4096u32.min(device.limits().max_texture_dimension_2d);
        let depth_tex = |size: u32, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let shadow_view = depth_tex(shadow_size, "shadow map").create_view(&Default::default());
        let dummy_shadow_view = depth_tex(1, "dummy shadow").create_view(&Default::default());

        let globals = gpu.buffer(
            "globals",
            wgpu::BufferUsages::UNIFORM,
            &vec![0u8; std::mem::size_of::<Globals>()],
        );
        let sampler_for_lights = &sampler_clamp;
        let no_lights = gpu
            .texture_2d("no night lights", 1, 1, &[0, 0, 0, 255], false)
            .create_view(&Default::default());
        let mk_bg = |shadow: &wgpu::TextureView, lights: &wgpu::TextureView, label: &str| {
            globals_bind_group(
                device,
                &bgl_globals,
                &globals,
                shadow,
                &shadow_sampler,
                lights,
                sampler_for_lights,
                label,
            )
        };
        let bg_main = mk_bg(&shadow_view, &no_lights, "globals main");
        let bg_shadow = mk_bg(&dummy_shadow_view, &no_lights, "globals shadow pass");
        let grey = gpu
            .texture_2d("grey", 2, 2, &[160u8; 16].map(|v| v), false)
            .create_view(&Default::default());

        let fx = FxRenderer::new(&gpu)?;
        Ok(SceneRenderer {
            gpu,
            bgl_nodes,
            bgl_shape_tex,
            bgl_terrain,
            bgl_water,
            layouts,
            pipelines: Mutex::new(cache),
            visual: VisualOptions::default(),
            baseline,
            local_lights: Vec::new(),
            local_shadows: true,
            globals,
            sampler_clamp,
            sampler_wrap,
            sampler_shape,
            shadow_size,
            shadow_view,
            bgl_globals,
            shadow_sampler,
            dummy_shadow_view,
            bg_main,
            bg_shadow,
            terrain: None,
            terrain_map: None,
            models: Vec::new(),
            model_cpu: Vec::new(),
            model_by_name: HashMap::new(),
            grey,
            fx,
        })
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The shared device handle.
    pub fn gpu_arc(&self) -> &Arc<Gpu> {
        &self.gpu
    }

    /// The pipelines for targets of `format` with `samples` MSAA samples (built on first use).
    fn pipelines_for(
        &self,
        format: wgpu::TextureFormat,
        samples: u32,
        ids: bool,
    ) -> Result<Arc<Pipelines>> {
        let variant = if ids {
            Variant::Ids
        } else if self.baseline {
            Variant::Baseline
        } else {
            Variant::Scene
        };
        let key = (format, samples, self.visual, variant);
        let mut cache = self
            .pipelines
            .lock()
            .map_err(|_| RenderError::Other("pipeline cache poisoned".into()))?;
        if let Some(p) = cache.get(&key) {
            return Ok(p.clone());
        }
        let p = Arc::new(build_pipelines(
            &self.gpu.device,
            &self.layouts,
            format,
            samples,
            self.visual,
            variant,
        )?);
        cache.insert(key, p.clone());
        Ok(p)
    }

    /// Switches the visual upgrades (see [`crate::visual`]); the default is everything off, which
    /// renders exactly as before they existed.  Purely presentational: nothing in the simulation
    /// reads it.
    pub fn set_visual(&mut self, options: VisualOptions) {
        if !self.baseline {
            self.visual = options;
        }
    }

    /// Test support: the visual options of every scene pipeline set built so far (the baseline
    /// renderer and a renderer that never enabled a switch list only the default).
    pub fn built_visual_variants(&self) -> Vec<VisualOptions> {
        let cache = self.pipelines.lock().expect("pipeline cache");
        let mut v: Vec<VisualOptions> = cache
            .keys()
            .filter(|k| k.3 != Variant::Ids)
            .map(|k| k.2)
            .collect();
        v.sort_by_key(|o| (o.enhanced_lighting, o.enhanced_water));
        v.dedup();
        v
    }

    /// The visual upgrades currently on.
    pub fn visual(&self) -> VisualOptions {
        self.visual
    }

    pub fn model(&self, id: ModelId) -> &ShapeModel {
        &self.model_cpu[id.0]
    }

    /// Binds the animation of each gesture to a model (`ShapeModel::gestures`).  The clips are CPU data: the
    /// GPU copy of the model does not change.
    pub fn set_gesture_clips(&mut self, id: ModelId, clips: Vec<Option<AnimationClip>>) {
        self.model_cpu[id.0].gestures = clips;
    }

    /// Uploads the terrain mesh and its textures.
    pub fn set_terrain(&mut self, mission: &Mission, tex: &TerrainTextures) {
        self.clear_shapes();
        self.terrain_map = Some(mission.terrain.clone());
        // Bitmaps beyond the device's array layer limit fall back to the colour map.
        let max_layers = self.gpu.device.limits().max_texture_array_layers;
        let layers = &tex.tile_layers[..tex.tile_layers.len().min(max_layers as usize)];
        let words: Vec<u32> = tex
            .tile_words
            .iter()
            .map(|&w| {
                if (w & 0xffff) != 0xffff && (w & 0xffff) >= layers.len() as u32 {
                    pack_tile_word(0xffff, TileKind::ColorMap)
                } else {
                    w
                }
            })
            .collect();
        let mesh: TerrainMesh = mission.terrain.build_mesh(Some(&words));
        let gpu = &self.gpu;
        let vb = gpu.buffer(
            "terrain vb",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&mesh.vertices),
        );
        let ib = gpu.buffer(
            "terrain ib",
            wgpu::BufferUsages::INDEX,
            bytemuck::cast_slice(&mesh.indices),
        );
        let water_ib = gpu.buffer(
            "water ib",
            wgpu::BufferUsages::INDEX,
            bytemuck::cast_slice(&mesh.water_indices),
        );
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let cm = &tex.color_map;
        let color_map = gpu.texture_2d(
            &tex.color_map_name,
            cm.width as u32,
            cm.height as u32,
            &cm.pixels,
            true,
        );
        let det = &tex.detail;
        let detail = gpu.texture_2d(
            &tex.detail_name,
            det.width as u32,
            det.height as u32,
            &det.pixels,
            true,
        );
        let tiles = gpu.texture_array_named(
            "terrain tiles",
            TILE_SIZE,
            layers,
            &tex.tile_names[..layers.len().min(tex.tile_names.len())],
        );
        let tile_view = tiles.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let views = [view(&color_map), view(&detail), tile_view];
        let empty_lights = self.light_map_view(&LightMap::empty());
        let nm = visual::build_normal_map(&mission.terrain);
        let normal_view = gpu
            .texture_2d_raw(
                "terrain normals",
                nm.size as u32,
                nm.size as u32,
                &nm.rgba,
                false,
            )
            .create_view(&Default::default());
        let bg_terrain = self.terrain_bind_group(&views, &empty_lights, &normal_view);

        let wt = &tex.water;
        let water = gpu.texture_2d(
            &tex.water_name,
            wt.width as u32,
            wt.height as u32,
            &wt.pixels,
            true,
        );
        let water_view = view(&water);
        let transparent = gpu.texture_2d("no water detail", 1, 1, &[0, 0, 0, 0], false);
        let detail_views: Vec<wgpu::TextureView> = if tex.water_detail.is_empty() {
            vec![view(&transparent)]
        } else {
            tex.water_detail
                .iter()
                .enumerate()
                .map(|(i, im)| {
                    view(
                        &gpu.texture_2d(
                            tex.water_detail_names
                                .get(i)
                                .map_or("water detail", String::as_str),
                            im.width as u32,
                            im.height as u32,
                            &im.pixels,
                            true,
                        ),
                    )
                })
                .collect()
        };
        let water_group = |dv: &wgpu::TextureView, next: &wgpu::TextureView| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water textures"),
                layout: &self.bgl_water,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&water_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(dv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler_wrap),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(next),
                    },
                ],
            })
        };
        // the default path steps through the frames; its next-frame slot is unused
        let bg_water: Vec<wgpu::BindGroup> =
            detail_views.iter().map(|dv| water_group(dv, dv)).collect();
        // the enhanced path cross-fades neighbours of the ping-pong order, both directions
        let mut bg_water_pairs = Vec::new();
        for i in 0..detail_views.len().saturating_sub(1) {
            bg_water_pairs.push((
                (i, i + 1),
                water_group(&detail_views[i], &detail_views[i + 1]),
            ));
            bg_water_pairs.push((
                (i + 1, i),
                water_group(&detail_views[i + 1], &detail_views[i]),
            ));
        }

        let (lo, hi) = mission.terrain.elevation_range();
        self.terrain = Some(TerrainGpu {
            vb,
            ib,
            index_count: mesh.indices.len() as u32,
            water_ib,
            water_index_count: mesh.water_indices.len() as u32,
            bg_terrain,
            views,
            normal_view,
            bg_water,
            bg_water_pairs,
            world_side: mission.terrain.world_side(),
            top_left: mission.terrain.top_left(),
            z_range: (lo, hi),
            detail_frame_rate: 8.0,
        });
    }

    fn light_map_view(&self, lm: &LightMap) -> wgpu::TextureView {
        // not mipmapped: the map is a smooth additive term
        self.gpu
            .texture_2d(
                "terrain light map",
                lm.size as u32,
                lm.size as u32,
                &lm.rgba,
                false,
            )
            .create_view(&Default::default())
    }

    fn terrain_bind_group(
        &self,
        views: &[wgpu::TextureView; 3],
        lights: &wgpu::TextureView,
        normals: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain textures"),
                layout: &self.bgl_terrain,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler_clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.sampler_wrap),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(lights),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(normals),
                    },
                ],
            })
    }

    /// Bakes and uploads the building night lights (see `terrain_lights`).  Call after the
    /// terrain is set; daylight (`night_factor <= 0`) or no lights clears them.
    pub fn set_terrain_lights(
        &mut self,
        mission: &Mission,
        lights: &[TerrainLight],
        night_factor: f32,
    ) {
        self.local_lights = if night_factor > 0.0 {
            lights.to_vec()
        } else {
            Vec::new()
        };
        let lm = terrain_lights::bake(&mission.terrain, lights, night_factor);
        let view = self.light_map_view(&lm);
        // shapes sample the same map (the `TG_LIGHT_TERRAIN` branch of `MultiTransformShape`)
        let shape_lights = self.light_map_view(&lm);
        self.bg_main = globals_bind_group(
            &self.gpu.device,
            &self.bgl_globals,
            &self.globals,
            &self.shadow_view,
            &self.shadow_sampler,
            &shape_lights,
            &self.sampler_clamp,
            "globals main",
        );
        self.bg_shadow = globals_bind_group(
            &self.gpu.device,
            &self.bgl_globals,
            &self.globals,
            &self.dummy_shadow_view,
            &self.shadow_sampler,
            &shape_lights,
            &self.sampler_clamp,
            "globals shadow pass",
        );
        if let Some(t) = &self.terrain {
            let bg = self.terrain_bind_group(&t.views, &view, &t.normal_view);
            if let Some(t) = &mut self.terrain {
                t.bg_terrain = bg;
            }
        }
    }

    /// Local Shadows (`useLocalShadows`) on or off.  On, at night the building light nearest the view casts the
    /// shadows of the shapes inside its radius (the sun casts none at night); off, nothing but the sun does.
    pub fn set_local_shadows(&mut self, on: bool) {
        self.local_shadows = on;
    }

    /// The point light whose shape shadows are drawn this frame, if any: Local Shadows on, night, no sun shadows,
    /// and a light within reach of the view.
    fn local_shadow_light(&self, frame: &Frame<'_>) -> Option<&TerrainLight> {
        if !self.local_shadows || frame.shadows || !frame.light.is_night {
            return None;
        }
        let t = frame.camera.target;
        let dist = |l: &TerrainLight| {
            let (dx, dy) = (l.position.x - t.x, l.position.y - t.y);
            (dx * dx + dy * dy).sqrt()
        };
        self.local_lights
            .iter()
            .filter(|l| dist(l) < l.outer_radius + 600.0)
            .min_by(|a, b| dist(a).total_cmp(&dist(b)))
    }

    /// Number of shape models currently uploaded.
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Drops every uploaded shape model (and its textures).  `set_terrain` calls this: a new
    /// mission starts with an empty model set, so a renderer reused through a campaign does not
    /// keep the previous missions' meshes, textures and paint variants alive.
    pub fn clear_shapes(&mut self) {
        self.models.clear();
        self.model_cpu.clear();
        self.model_by_name.clear();
    }

    /// Loads `data/tgl/<name>.tgl` and its textures once per `(name, style)`.
    ///
    /// Textures named `a_*` keep their alpha channel and are drawn as cut-outs (the game draws
    /// them blended with alpha test for trees); every other texture ignores alpha like
    /// `gos_Texture_Solid`.
    pub fn load_shape(
        &mut self,
        game: &mut GameData,
        name: &str,
        style: ShapeStyle,
    ) -> Result<ModelId> {
        self.load_shape_keyed(game, name, style, None)
    }

    /// As [`SceneRenderer::load_shape`], for a model that carries its own gesture clips
    /// ([`SceneRenderer::set_gesture_clips`]).  The clips are bound to the model, so each `owner` (the appearance
    /// whose animation files they are) gets its own copy: two appearances that share a mesh file keep their own
    /// animations, whatever the load order.
    pub fn load_shape_owned(
        &mut self,
        game: &mut GameData,
        name: &str,
        style: ShapeStyle,
        owner: &str,
    ) -> Result<ModelId> {
        self.load_shape_keyed(game, name, style, Some(owner.to_ascii_lowercase()))
    }

    fn load_shape_keyed(
        &mut self,
        game: &mut GameData,
        name: &str,
        style: ShapeStyle,
        owner: Option<String>,
    ) -> Result<ModelId> {
        let key = (name.to_ascii_lowercase(), style, owner);
        if let Some(id) = self.model_by_name.get(&key) {
            return Ok(*id);
        }
        let model = ShapeModel::load(game, name)?;
        // Original art pack (off unless `art_dir` is set): a `<name>.glb` there replaces the geometry
        // and paint mask; the node list, bounds and extents stay the original's.
        let (model, embedded) = match &self.gpu.cfg.art_dir {
            Some(dir) => match crate::artpack::load_pack_model(dir, name, &model)? {
                Some((m, t)) => (m, t),
                None => (model, Vec::new()),
            },
            None => (model, Vec::new()),
        };
        let id = self.add_model_with(game, model, style, &embedded);
        self.model_by_name.insert(key, id);
        Ok(id)
    }

    pub fn add_model(
        &mut self,
        game: &mut GameData,
        model: ShapeModel,
        style: ShapeStyle,
    ) -> ModelId {
        self.add_model_with(game, model, style, &[])
    }

    /// As [`SceneRenderer::add_model`]; `embedded[i]` (when present) is texture `i` of the model's list
    /// instead of a game-data TGA (art pack models carry their own paint mask).
    pub fn add_model_with(
        &mut self,
        game: &mut GameData,
        model: ShapeModel,
        style: ShapeStyle,
        embedded: &[crate::artpack::PackTexture],
    ) -> ModelId {
        let gpu = &self.gpu;
        // One texture per entry of the shape's texture list.
        let views: Vec<wgpu::TextureView> = model
            .texture_names
            .iter()
            .enumerate()
            .map(|(index, n)| {
                if let Some(t) = embedded.get(index) {
                    let mut px = t.rgba.clone();
                    let paint = if index == 0 { style.paint } else { None };
                    prepare_texture_pixels(&mut px, paint, style.paint_routine, false);
                    return gpu
                        .texture_2d_raw(n, t.width, t.height, &px, SHAPE_SKIN_MIPS)
                        .create_view(&Default::default());
                }
                let path = texture_path(n);
                match game.tga(&path) {
                    Ok(img) => {
                        // The hi-res replacement is looked up by the plain texture path, so one
                        // pack texture serves every paint scheme; paint and the alpha rule then
                        // act on the pixels that will be drawn (replacement or original).
                        let (mut px, w, h) = gpu.replacement(
                            &path,
                            img.width as u32,
                            img.height as u32,
                            &img.pixels,
                        );
                        // the paint is the first texture's (the mask); the alpha rule is per texture
                        let paint = if index == 0 { style.paint } else { None };
                        let texture_alpha =
                            model.texture_alpha.get(index).copied().unwrap_or(false);
                        prepare_texture_pixels(&mut px, paint, style.paint_routine, texture_alpha);
                        gpu.texture_2d_raw(&path, w, h, &px, SHAPE_SKIN_MIPS)
                            .create_view(&Default::default())
                    }
                    Err(_) => self.grey.clone(),
                }
            })
            .collect();
        let vb = gpu.buffer(
            "shape vb",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&model.vertices),
        );

        let rgb = |c: u32| {
            let v = crate::light::argb_to_rgb(c);
            [v[0], v[1], v[2], 1.0]
        };
        let params = ModelParams {
            pink: rgb(style.glow.pink),
            yellow: rgb(style.glow.yellow),
            green: rgb(style.glow.green),
        };
        let params_buf = gpu.buffer(
            "model params",
            wgpu::BufferUsages::UNIFORM,
            bytemuck::bytes_of(&params),
        );
        let batches = model
            .batches
            .iter()
            .map(|b| {
                let view = views.get(b.texture).unwrap_or(&self.grey);
                BatchGpu {
                    first: b.first_vertex,
                    count: b.vertex_count,
                    kind: b.kind,
                    alpha: b.alpha,
                    bind_group: gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("shape texture"),
                        layout: &self.bgl_shape_tex,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.sampler_shape),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: params_buf.as_entire_binding(),
                            },
                        ],
                    }),
                }
            })
            .collect();
        let id = ModelId(self.models.len());
        self.models.push(ModelGpu {
            vb,
            batches,
            node_count: model.nodes.len(),
        });
        self.model_cpu.push(model);
        id
    }

    /// Orthographic light matrix covering the visible ground and everything standing on it.
    fn light_matrix(
        &self,
        frame: &Frame<'_>,
        terrain: &TerrainGpu,
        local: Option<&TerrainLight>,
    ) -> Mat4 {
        let to_sun = match local {
            // a point light: the direction from the view's ground point to it, no lower than 20 degrees up
            Some(l) => {
                let d = l.position - frame.camera.target;
                let flat = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
                Vec3::new(d.x, d.y, d.z.max(flat * 0.364)).normalized()
            }
            None => frame.light.shape_light_direction().normalized(),
        };
        let f = -to_sun;
        let up_ref = Vec3::new(0.0, 0.0, 1.0);
        let right = f.cross(up_ref).normalized();
        let up = right.cross(f).normalized();
        // Visible ground: rays through the viewport corners and edge midpoints.
        let cam = frame.camera;
        let (w, h) = (cam.viewport.0, cam.viewport.1);
        let mut pts: Vec<Vec3> = Vec::new();
        let ground_z = cam.target.z;
        for &(u, v) in &[
            (0.0, 0.0),
            (0.5, 0.0),
            (1.0, 0.0),
            (0.0, 0.5),
            (1.0, 0.5),
            (0.0, 1.0),
            (0.5, 1.0),
            (1.0, 1.0),
        ] {
            let (o, d) = cam.pixel_ray(u * w, v * h);
            let t = if d.z < -1e-4 {
                ((ground_z - o.z) / d.z).clamp(0.0, 4500.0)
            } else {
                4500.0
            };
            pts.push(o + d * t);
        }
        let (zlo, zhi) = (terrain.z_range.0 - 50.0, terrain.z_range.1 + 450.0);
        let mut mn = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut mx = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
        for p in &pts {
            for z in [zlo, zhi] {
                let q = Vec3::new(p.x, p.y, z);
                let l = Vec3::new(q.dot(right), q.dot(up), q.dot(f));
                mn = Vec3::new(mn.x.min(l.x), mn.y.min(l.y), mn.z.min(l.z));
                mx = Vec3::new(mx.x.max(l.x), mx.y.max(l.y), mx.z.max(l.z));
            }
        }
        // margins
        let pad = 60.0;
        mn = Vec3::new(mn.x - pad, mn.y - pad, mn.z - pad);
        mx = Vec3::new(mx.x + pad, mx.y + pad, mx.z + pad);
        let sx = 2.0 / (mx.x - mn.x);
        let sy = 2.0 / (mx.y - mn.y);
        let sz = 1.0 / (mx.z - mn.z);
        Mat4::from_rows([
            [right.x * sx, right.y * sx, right.z * sx, -(mn.x * sx) - 1.0],
            [up.x * sy, up.y * sy, up.z * sy, -(mn.y * sy) - 1.0],
            [f.x * sz, f.y * sz, f.z * sz, -(mn.z * sz)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn globals(
        &self,
        frame: &Frame<'_>,
        terrain: &TerrainGpu,
        light_vp: &Mat4,
        shadows: bool,
    ) -> Globals {
        let lu: LightUniform = frame.light.uniform();
        let cam = frame.camera;
        let cp = cam.position_world();
        let w = frame.water;
        // Terrain::frameAngle / frameCos
        let angle = (w.frequency * frame.time) % 360.0;
        let wave = angle.to_radians().cos() * w.amplitude;
        let cloud = (frame.time * 0.002).fract();
        let a = |c: u32| ((c >> 24) & 0xff) as f32 / 255.0;
        Globals {
            view_proj: cam.gpu_view_proj().0,
            light_view_proj: light_vp.0,
            camera_pos: [cp.x, cp.y, cp.z, 1.0],
            terrain_dir: lu.terrain_dir,
            shape_dir: lu.shape_dir,
            sun: lu.sun,
            ambient: lu.ambient,
            fog: lu.fog,
            fog_height: lu.fog_height,
            map: [
                terrain.world_side,
                terrain.top_left.0,
                terrain.top_left.1,
                frame.terrain_params.light_scale,
            ],
            tiling: [
                frame.terrain_params.detail_tiling,
                frame.terrain_params.water_tiling,
                frame.terrain_params.water_detail_tiling,
                frame.time,
            ],
            water: [w.elevation, wave, w.alpha_depth, cloud],
            water_alpha: [a(w.alpha_shallow), a(w.alpha_middle), a(w.alpha_deep), 0.0],
            haze: [
                cam.settings.min_haze_distance,
                cam.settings.max_clip_distance,
                1.0,
                if shadows { 1.0 } else { 0.0 },
            ],
            shadow_info: [
                1.0 / self.shadow_size as f32,
                0.0015,
                frame.terrain_params.lightning,
                if frame.terrain_params.detail_texture {
                    1.0
                } else {
                    0.0
                },
            ],
            water_anim: if self.visual.enhanced_water {
                let t = frame.precise_time.unwrap_or_else(|| f64::from(frame.time));
                let blend =
                    visual::water_blend(t, terrain.detail_frame_rate, terrain.bg_water.len());
                [
                    visual::cloud_angle(t),
                    visual::spray_offset(t),
                    blend.t,
                    visual::GLINT_STRENGTH,
                ]
            } else {
                [0.0; 4]
            },
        }
    }

    /// Registers a texture for effect batches (CROSS-CRATE: gosFX drawing hook).
    pub fn add_fx_texture(&mut self, name: &str, w: u32, h: u32, rgba: &[u8]) -> FxTextureId {
        self.fx.add_texture(&self.gpu, name, w, h, rgba)
    }

    /// Renders one frame offscreen and returns tightly packed RGBA8: [`Self::draw`] into an
    /// [`OffscreenTarget`] and a readback, the same path as a windowed frame.
    pub fn render(&self, frame: &Frame<'_>) -> Result<Vec<u8>> {
        self.render_with_fx(frame, &[])
    }

    /// Like [`render`](Self::render), drawing the gosFX effect batches in the same pass after
    /// the water and windows, depth-tested against the scene (CROSS-CRATE: gosFX drawing hook).
    pub fn render_with_fx(&self, frame: &Frame<'_>, fx: &[FxBatch]) -> Result<Vec<u8>> {
        let gpu = self.gpu.clone();
        let offscreen = OffscreenTarget::new(&gpu, frame.width, frame.height);
        let target = offscreen.target();
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene"),
            });
        self.encode(&gpu, &mut enc, &target, frame, fx)?;
        target.read_rgba(&gpu, enc)
    }

    /// Test support: the same view as [`render`](Self::render) with every pixel's material class
    /// in the red byte (`class * 16`) and coverage in green, no lighting, shadows or blending:
    /// 0 nothing drawn, 1 plain terrain, 2 terrain tile blended by its alpha, 3 terrain tile
    /// replacing the colour map, 4/5 water (base/detail, green = alpha), 6 shape, 7 window,
    /// 8 night spotlight, 9 fully fogged terrain.  `translucent` also draws water, windows and
    /// spotlights; without it only the opaque surfaces (terrain, shapes) are drawn.
    pub fn render_ids(&self, frame: &Frame<'_>, translucent: bool) -> Result<Vec<u8>> {
        let gpu = self.gpu.clone();
        let offscreen = OffscreenTarget::new(&gpu, frame.width, frame.height);
        let target = offscreen.target();
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene ids"),
            });
        let mode = if translucent {
            PassMode::IdsAll
        } else {
            PassMode::IdsOpaque
        };
        self.encode_mode(&gpu, &mut enc, &target, frame, &[], mode)?;
        target.read_rgba(&gpu, enc)
    }

    /// Records one frame of the scene into `encoder`, drawing into `target` (`frame.width` x
    /// `frame.height`).  Clears the target to the fog colour; HUD and cursor passes load it
    /// afterwards.  `gpu` must be the device the renderer was built on.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        frame: &Frame<'_>,
    ) -> Result<()> {
        self.encode(gpu, encoder, target, frame, &[])
    }

    /// [`Self::draw`] plus the gosFX effect batches, drawn in the same pass after the water and
    /// windows, depth-tested against the scene.
    pub fn draw_with_fx(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        frame: &Frame<'_>,
        fx: &[FxBatch],
    ) -> Result<()> {
        self.encode(gpu, encoder, target, frame, fx)
    }

    /// [`Self::draw_with_fx`] through a shared reference.
    pub fn encode(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        frame: &Frame<'_>,
        fx: &[FxBatch],
    ) -> Result<()> {
        self.encode_mode(gpu, encoder, target, frame, fx, PassMode::Colour)
    }

    fn encode_mode(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        frame: &Frame<'_>,
        fx: &[FxBatch],
        mode: PassMode,
    ) -> Result<()> {
        let ids = mode != PassMode::Colour;
        let translucent = mode != PassMode::IdsOpaque;
        if !std::ptr::eq(gpu, &*self.gpu) {
            return Err(RenderError::Other(
                "draw called with a different Gpu than the renderer was built on".into(),
            ));
        }
        let terrain = self
            .terrain
            .as_ref()
            .ok_or_else(|| RenderError::Other("no terrain uploaded".into()))?;
        let gpu = &*self.gpu;
        if target.size() != (frame.width.max(1), frame.height.max(1)) {
            return Err(RenderError::Other(format!(
                "frame is {}x{} but the target is {:?}",
                frame.width,
                frame.height,
                target.size()
            )));
        }
        let pipelines = self.pipelines_for(target.display_format, target.samples, ids)?;

        // --- instances grouped by model
        let mut per_model: Vec<Vec<usize>> = vec![Vec::new(); self.models.len()];
        for (i, inst) in frame.instances.iter().enumerate() {
            // An object out of view is not drawn at all (`BldgAppearance::render`, `bdactor.cpp:1508`).
            if inst.in_view && inst.model.0 < per_model.len() {
                per_model[inst.model.0].push(i);
            }
        }
        let mut node_mats: Vec<[f32; 16]> = Vec::new();
        let mut inst_data: Vec<InstanceData> = Vec::new();
        let mut inst_casts: Vec<bool> = Vec::new();
        // (model, first instance, count)
        let mut draws: Vec<(usize, u32, u32)> = Vec::new();
        for (m, list) in per_model.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let first = inst_data.len() as u32;
            for &i in list {
                let inst = &frame.instances[i];
                let base = node_mats.len() as u32;
                let mats = instance_node_matrices(&inst.pose, inst.position, inst.yaw_degrees);
                let want = self.models[m].node_count;
                for k in 0..want {
                    node_mats.push(mats.get(k).copied().unwrap_or(Mat4::IDENTITY).0);
                }
                let amb = match inst.ambient_override {
                    Some(a) => [a[0], a[1], a[2], 1.0],
                    None => [0.0; 4],
                };
                inst_data.push(InstanceData {
                    node_base: base,
                    flags: 0,
                    highlight: inst.highlight,
                    _pad: 0,
                    infinite: [
                        inst.infinite_light[0],
                        inst.infinite_light[1],
                        inst.infinite_light[2],
                        0.0,
                    ],
                    ambient_override: amb,
                });
                inst_casts.push(inst.casts_shadow);
            }
            draws.push((m, first, list.len() as u32));
        }
        // A fully culled frame still binds the storage buffer for the shared scene pipeline.
        // wgpu requires that binding to cover one `mat4x4<f32>` (64 bytes), even though no draw
        // reads it. This is renderer plumbing, not a gameplay or visibility decision.
        if node_mats.is_empty() {
            node_mats.push(Mat4::IDENTITY.0);
        }
        let nodes_buf = gpu.buffer(
            "node matrices",
            wgpu::BufferUsages::STORAGE,
            bytemuck::cast_slice(&node_mats),
        );
        let inst_buf = gpu.buffer(
            "instances",
            wgpu::BufferUsages::VERTEX,
            bytemuck::cast_slice(&inst_data),
        );
        let bg_nodes = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("node matrices"),
            layout: &self.bgl_nodes,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: nodes_buf.as_entire_binding(),
            }],
        });

        let local = self.local_shadow_light(frame).cloned();
        if let Some(l) = &local {
            // only shapes inside the light's reach are shadowed by it
            for (k, &i) in per_model.iter().flatten().enumerate() {
                let p = frame.instances[i].position;
                let (dx, dy) = (p.x - l.position.x, p.y - l.position.y);
                if dx * dx + dy * dy > l.outer_radius * l.outer_radius {
                    inst_casts[k] = false;
                }
            }
        }
        let shadows_on = frame.shadows || local.is_some();
        let light_vp = self.light_matrix(frame, terrain, local.as_ref());
        let globals = self.globals(frame, terrain, &light_vp, shadows_on);
        gpu.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let enc = encoder;

        // --- shadow pass
        if shadows_on && !ids && !draws.is_empty() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bg_shadow, &[]);
            pass.set_bind_group(1, &bg_nodes, &[]);
            for cutout in [false, true] {
                pass.set_pipeline(if cutout {
                    &pipelines.shadow_cutout
                } else {
                    &pipelines.shadow
                });
                for &(m, first, count) in &draws {
                    let model = &self.models[m];
                    pass.set_vertex_buffer(0, model.vb.slice(..));
                    pass.set_vertex_buffer(1, inst_buf.slice(..));
                    for b in &model.batches {
                        if b.kind != NodeKind::Normal || b.alpha != cutout {
                            continue;
                        }
                        if cutout {
                            pass.set_bind_group(2, &b.bind_group, &[]);
                        }
                        // Casters only: draw runs of consecutive casting instances.
                        let mut run_start: Option<u32> = None;
                        for k in 0..=count {
                            let casts = k < count && inst_casts[(first + k) as usize];
                            match (casts, run_start) {
                                (true, None) => run_start = Some(first + k),
                                (false, Some(s)) => {
                                    pass.draw(b.first..b.first + b.count, s..first + k);
                                    run_start = None;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // --- main pass
        {
            let (view, resolve) = target.color_attachment();
            let clear = if ids { [0.0; 3] } else { frame.light.fog_color };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::gpu::clear_color(
                            [clear[0], clear[1], clear[2]],
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
            pass.set_viewport(0.0, 0.0, frame.width as f32, frame.height as f32, 0.0, 1.0);

            // terrain
            pass.set_pipeline(&pipelines.terrain);
            pass.set_bind_group(0, &self.bg_main, &[]);
            pass.set_bind_group(1, &terrain.bg_terrain, &[]);
            pass.set_vertex_buffer(0, terrain.vb.slice(..));
            pass.set_index_buffer(terrain.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..terrain.index_count, 0, 0..1);

            // shapes: opaque models first, cut-out models next
            for cutout in [false, true] {
                pass.set_pipeline(if cutout {
                    &pipelines.shape_cutout
                } else {
                    &pipelines.shape_opaque
                });
                pass.set_bind_group(0, &self.bg_main, &[]);
                pass.set_bind_group(1, &bg_nodes, &[]);
                for &(m, first, count) in &draws {
                    let model = &self.models[m];
                    pass.set_vertex_buffer(0, model.vb.slice(..));
                    pass.set_vertex_buffer(1, inst_buf.slice(..));
                    for b in &model.batches {
                        if b.kind != NodeKind::Normal || b.alpha != cutout {
                            continue;
                        }
                        pass.set_bind_group(2, &b.bind_group, &[]);
                        pass.draw(b.first..b.first + b.count, first..first + count);
                    }
                }
            }

            // water
            if translucent && terrain.water_index_count > 0 {
                let frames = terrain.bg_water.len();
                let frame_idx = detail_frame(frame.time, terrain.detail_frame_rate, frames);
                let water_bg = if self.visual.enhanced_water {
                    let b = visual::water_blend(
                        frame.precise_time.unwrap_or_else(|| f64::from(frame.time)),
                        terrain.detail_frame_rate,
                        frames,
                    );
                    terrain
                        .bg_water_pairs
                        .iter()
                        .find(|(k, _)| *k == (b.from, b.to))
                        .map_or(&terrain.bg_water[b.from], |(_, g)| g)
                } else {
                    &terrain.bg_water[frame_idx]
                };
                pass.set_vertex_buffer(0, terrain.vb.slice(..));
                pass.set_index_buffer(terrain.water_ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(0, &self.bg_main, &[]);
                pass.set_bind_group(1, water_bg, &[]);
                pass.set_pipeline(&pipelines.water_base);
                pass.draw_indexed(0..terrain.water_index_count, 0, 0..1);
                if frame.terrain_params.detail_texture {
                    pass.set_pipeline(&pipelines.water_detail);
                    pass.draw_indexed(0..terrain.water_index_count, 0, 0..1);
                }
            }

            // night spotlight cones (additive); `TG_Shape::Render` skips them by day
            if translucent && frame.light.is_night {
                pass.set_pipeline(&pipelines.shape_spot);
                pass.set_bind_group(0, &self.bg_main, &[]);
                pass.set_bind_group(1, &bg_nodes, &[]);
                for &(m, first, count) in &draws {
                    let model = &self.models[m];
                    pass.set_vertex_buffer(0, model.vb.slice(..));
                    pass.set_vertex_buffer(1, inst_buf.slice(..));
                    for b in &model.batches {
                        if b.kind != NodeKind::Spotlight {
                            continue;
                        }
                        pass.set_bind_group(2, &b.bind_group, &[]);
                        pass.draw(b.first..b.first + b.count, first..first + count);
                    }
                }
            }

            // windows (blended)
            let window_draws: &[(usize, u32, u32)] = if translucent { &draws } else { &[] };
            pass.set_pipeline(&pipelines.shape_window);
            pass.set_bind_group(0, &self.bg_main, &[]);
            pass.set_bind_group(1, &bg_nodes, &[]);
            for &(m, first, count) in window_draws {
                let model = &self.models[m];
                pass.set_vertex_buffer(0, model.vb.slice(..));
                pass.set_vertex_buffer(1, inst_buf.slice(..));
                for b in &model.batches {
                    if b.kind != NodeKind::Window {
                        continue;
                    }
                    pass.set_bind_group(2, &b.bind_group, &[]);
                    pass.draw(b.first..b.first + b.count, first..first + count);
                }
            }

            // effects (gosFX): after everything opaque and the windows
            if !ids && !fx.is_empty() {
                self.fx.draw_into(
                    gpu,
                    &mut pass,
                    target.display_format,
                    target.samples,
                    &frame.camera.gpu_view_proj(),
                    fx,
                    self.terrain_map.as_ref(),
                )?;
            }
        }
        Ok(())
    }
}

/// What one `encode_mode` call records.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PassMode {
    Colour,
    IdsOpaque,
    IdsAll,
}

/// Pipeline layouts and the shader module: everything the per-format pipelines are built from.
struct PipelineLayouts {
    module: wgpu::ShaderModule,
    terrain: wgpu::PipelineLayout,
    water: wgpu::PipelineLayout,
    shape: wgpu::PipelineLayout,
    shadow: wgpu::PipelineLayout,
    shadow_cutout: wgpu::PipelineLayout,
}

/// Builds the scene pipelines for one colour format and MSAA sample count.
fn build_pipelines(
    device: &wgpu::Device,
    l: &PipelineLayouts,
    target_format: wgpu::TextureFormat,
    samples: u32,
    visual: VisualOptions,
    variant: Variant,
) -> Result<Pipelines> {
    let terrain_attrs = [
        vertex_attr(wgpu::VertexFormat::Float32x3, 0, 0),
        vertex_attr(wgpu::VertexFormat::Uint32, 12, 1),
        vertex_attr(wgpu::VertexFormat::Float32x3, 16, 2),
        vertex_attr(wgpu::VertexFormat::Uint32, 28, 3),
        vertex_attr(wgpu::VertexFormat::Float32x2, 32, 4),
    ];
    let shape_attrs = [
        vertex_attr(wgpu::VertexFormat::Float32x3, 0, 0),
        vertex_attr(wgpu::VertexFormat::Uint32, 12, 1),
        vertex_attr(wgpu::VertexFormat::Float32x3, 16, 2),
        vertex_attr(wgpu::VertexFormat::Uint32, 28, 3),
        vertex_attr(wgpu::VertexFormat::Float32x2, 32, 4),
    ];
    let instance_attrs = [
        vertex_attr(wgpu::VertexFormat::Uint32, 0, 5),
        vertex_attr(wgpu::VertexFormat::Uint32, 4, 6),
        vertex_attr(wgpu::VertexFormat::Float32x4, 16, 7),
        vertex_attr(wgpu::VertexFormat::Float32x4, 32, 8),
        vertex_attr(wgpu::VertexFormat::Uint32, 8, 9),
    ];
    let terrain_vb = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TerrainGpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &terrain_attrs,
    };
    let shape_vb = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ShapeVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &shape_attrs,
    };
    let instance_vb = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<InstanceData>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &instance_attrs,
    };

    let alpha_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        // The target stays opaque: blending must not write coverage into alpha.
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };

    // `gos_Alpha_OneOne` of the spotlight pass; alpha stays untouched like the others.
    let additive_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };

    struct Spec<'a> {
        label: &'a str,
        layout: &'a wgpu::PipelineLayout,
        vs: &'a str,
        fs: Option<&'a str>,
        buffers: Vec<wgpu::VertexBufferLayout<'a>>,
        cull: Option<wgpu::Face>,
        blend: Option<wgpu::BlendState>,
        depth_write: bool,
        a2c: bool,
        constants: Vec<(&'a str, f64)>,
        depth_only: bool,
        bias: wgpu::DepthBiasState,
        samples: u32,
    }
    let build = |s: Spec| -> wgpu::RenderPipeline {
        let mut constants = s.constants.clone();
        if s.fs.is_some() && !s.depth_only {
            if variant != Variant::Baseline {
                constants.push(("ENHANCED_LIGHT", f64::from(visual.enhanced_lighting)));
                constants.push(("ENHANCED_WATER", f64::from(visual.enhanced_water)));
            }
            if variant == Variant::Ids {
                constants.push(("ID_MODE", 1.0));
            }
            // display values into an sRGB target: decode in the shader (`to_target`)
            constants.push((
                "SRGB_TARGET",
                if target_format.is_srgb() { 1.0 } else { 0.0 },
            ));
        }
        let opts = wgpu::PipelineCompilationOptions {
            constants: &constants,
            zero_initialize_workgroup_memory: false,
        };
        let buffers: Vec<Option<wgpu::VertexBufferLayout>> =
            s.buffers.iter().cloned().map(Some).collect();
        let targets = [Some(wgpu::ColorTargetState {
            format: target_format,
            blend: if variant == Variant::Ids {
                None
            } else {
                s.blend
            },
            write_mask: wgpu::ColorWrites::COLOR,
        })];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(s.label),
            layout: Some(s.layout),
            vertex: wgpu::VertexState {
                module: &l.module,
                entry_point: Some(s.vs),
                compilation_options: opts.clone(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: s.cull,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(s.depth_write),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: s.bias,
            }),
            multisample: wgpu::MultisampleState {
                count: s.samples,
                mask: !0,
                alpha_to_coverage_enabled: s.a2c && variant != Variant::Ids,
            },
            fragment: if s.fs.is_none() {
                None
            } else {
                Some(wgpu::FragmentState {
                    module: &l.module,
                    entry_point: s.fs,
                    compilation_options: opts,
                    targets: if s.depth_only { &[] } else { &targets },
                })
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let shape_buffers = || vec![shape_vb.clone(), instance_vb.clone()];
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipelines = Pipelines {
        terrain: build(Spec {
            label: "terrain",
            layout: &l.terrain,
            vs: "vs_terrain",
            fs: Some("fs_terrain"),
            buffers: vec![terrain_vb.clone()],
            cull: Some(wgpu::Face::Back),
            blend: None,
            depth_write: true,
            a2c: false,
            constants: vec![],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        water_base: build(Spec {
            label: "water base",
            layout: &l.water,
            vs: "vs_water",
            fs: Some("fs_water"),
            buffers: vec![terrain_vb.clone()],
            cull: None,
            blend: Some(alpha_blend),
            depth_write: false,
            a2c: false,
            constants: vec![],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        water_detail: build(Spec {
            label: "water detail",
            layout: &l.water,
            vs: "vs_water",
            fs: Some("fs_water_detail"),
            buffers: vec![terrain_vb.clone()],
            cull: None,
            blend: Some(alpha_blend),
            depth_write: false,
            a2c: false,
            constants: vec![],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        shape_opaque: build(Spec {
            label: "shape opaque",
            layout: &l.shape,
            vs: "vs_shape",
            fs: Some("fs_shape"),
            buffers: shape_buffers(),
            cull: None,
            blend: None,
            depth_write: true,
            a2c: false,
            constants: vec![("CUTOUT", 0.0)],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        shape_cutout: build(Spec {
            label: "shape cutout",
            layout: &l.shape,
            vs: "vs_shape",
            fs: Some("fs_shape"),
            buffers: shape_buffers(),
            cull: None,
            blend: None,
            depth_write: true,
            a2c: samples > 1,
            constants: vec![
                ("CUTOUT", 1.0),
                ("ALPHA_CUT", if samples > 1 { 0.02 } else { 0.5 }),
            ],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        shape_window: build(Spec {
            label: "shape window",
            layout: &l.shape,
            vs: "vs_shape",
            fs: Some("fs_window"),
            buffers: shape_buffers(),
            cull: None,
            blend: Some(alpha_blend),
            depth_write: false,
            a2c: false,
            constants: vec![],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        shape_spot: build(Spec {
            label: "shape spotlight",
            layout: &l.shape,
            vs: "vs_shape",
            fs: Some("fs_spot"),
            buffers: shape_buffers(),
            cull: None,
            blend: Some(additive_blend),
            depth_write: false,
            a2c: false,
            constants: vec![],
            depth_only: false,
            bias: Default::default(),
            samples,
        }),
        shadow_cutout: build(Spec {
            label: "shadow cutout",
            layout: &l.shadow_cutout,
            vs: "vs_shadow_cutout",
            fs: Some("fs_shadow_cutout"),
            buffers: shape_buffers(),
            cull: None,
            blend: None,
            depth_write: true,
            a2c: false,
            constants: vec![],
            depth_only: true,
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
            samples: 1,
        }),
        shadow: build(Spec {
            label: "shadow",
            layout: &l.shadow,
            vs: "vs_shadow",
            fs: None,
            buffers: shape_buffers(),
            cull: None,
            blend: None,
            depth_write: true,
            a2c: false,
            constants: vec![],
            depth_only: true,
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
                clamp: 0.0,
            },
            samples: 1,
        }),
    };
    if let Some(err) = pollster::block_on(scope.pop()) {
        return Err(RenderError::Gpu(format!("pipeline creation failed: {err}")));
    }
    Ok(pipelines)
}

/// Water detail frame at `time`: ping-pong through the frames at `rate` frames per second
/// (`MapData::update` bounces `sprayFrame` between the first and last frame).
pub fn detail_frame(time: f32, rate: f32, frames: usize) -> usize {
    if frames <= 1 {
        return 0;
    }
    let period = 2 * (frames - 1);
    let step = (time * rate).max(0.0) as usize % period;
    if step < frames {
        step
    } else {
        period - step
    }
}

#[allow(clippy::too_many_arguments)]
fn globals_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    shadow: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    lights: &wgpu::TextureView,
    lights_sampler: &wgpu::Sampler,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(shadow),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(lights),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(lights_sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_skins_sample_level_zero_bilinear_like_the_cpp() {
        assert!(
            !std::hint::black_box(SHAPE_SKIN_MIPS),
            "the C++ disables mipmaps on every shape texture"
        );
        let d = shape_sampler_descriptor();
        assert_eq!(d.mag_filter, wgpu::FilterMode::Linear);
        assert_eq!(d.min_filter, wgpu::FilterMode::Linear);
        assert_eq!(d.mipmap_filter, wgpu::MipmapFilterMode::Nearest);
        assert_eq!(d.lod_max_clamp, 0.0);
        assert_eq!(d.anisotropy_clamp, 1);
    }

    #[test]
    fn baseline_shader_has_no_enhanced_code() {
        let b = baseline_shader();
        assert!(SHADER.contains("ENHANCED_LIGHT") && SHADER.contains("water_glint"));
        for word in [
            "ENHANCED",
            "water_glint",
            "detail_blended",
            "water_anim",
            "@enhanced",
        ] {
            let kept: Vec<&str> = b.lines().filter(|l| l.contains(word)).collect();
            // `water_anim` stays in the shared Globals struct (layout), nowhere else
            if word == "water_anim" {
                assert_eq!(kept.len(), 1, "{kept:?}");
            } else {
                assert!(kept.is_empty(), "{word}: {kept:?}");
            }
        }
    }

    #[test]
    fn water_detail_frames_ping_pong() {
        assert_eq!(detail_frame(0.0, 8.0, 1), 0);
        let seq: Vec<usize> = (0..10)
            .map(|i| detail_frame(i as f32 / 8.0 + 0.01, 8.0, 4))
            .collect();
        assert_eq!(seq, vec![0, 1, 2, 3, 2, 1, 0, 1, 2, 3]);
    }

    #[test]
    fn gpu_structs_match_the_wgsl_layouts() {
        assert_eq!(std::mem::size_of::<Globals>(), 2 * 64 + 14 * 16);
        assert_eq!(std::mem::size_of::<InstanceData>(), 48);
        assert_eq!(std::mem::size_of::<TerrainGpuVertex>(), 40);
        assert_eq!(std::mem::size_of::<ShapeVertex>(), 40);
    }
}

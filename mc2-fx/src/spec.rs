// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Effect specifications: the data half of every gosFX class, loaded field for field from
//! `mc2.fx` in the order of each `Load` constructor in `mclib/gosfx/*.cpp`.
//!
//! The class hierarchy of the C++ (`Effect` -> `ParticleCloud` -> `SpinningCloud` -> ...) is a
//! set of nested structs here: [`ParticleSpec`] inside [`SpinSpec`] inside [`CardCloudSpec`],
//! and so on. The abstract classes (`ParticleCloud`, `SpinningCloud`, `Singleton`) have no
//! specification factory in the C++ and cannot appear in a file.

use crate::error::{FxError, Result};
use crate::fcurve::*;
use crate::math::{Mat, Vec3};
use crate::mlrshape::MlrShape;
use crate::state::{read_mlr_version, MlrState, ALPHA_PRIORITY, TEXTURE_MASK};
use crate::stream::Stream;

/// `gosFX::*ClassID` relative to `FirstgosFXClassID` (1312 in this build, see
/// `mclib/stuff/stuff.hpp`).
pub const FIRST_GOSFX_CLASS_ID: i32 = 1312;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EffectClass {
    Effect,
    ParticleCloud,
    PointCloud,
    SpinningCloud,
    ShardCloud,
    PertCloud,
    CardCloud,
    ShapeCloud,
    EffectCloud,
    Singleton,
    Card,
    Shape,
    Tube,
    DebrisCloud,
    PointLight,
}

impl EffectClass {
    pub const ALL: [EffectClass; 15] = [
        EffectClass::Effect,
        EffectClass::ParticleCloud,
        EffectClass::PointCloud,
        EffectClass::SpinningCloud,
        EffectClass::ShardCloud,
        EffectClass::PertCloud,
        EffectClass::CardCloud,
        EffectClass::ShapeCloud,
        EffectClass::EffectCloud,
        EffectClass::Singleton,
        EffectClass::Card,
        EffectClass::Shape,
        EffectClass::Tube,
        EffectClass::DebrisCloud,
        EffectClass::PointLight,
    ];

    pub fn from_class_id(id: i32) -> Option<EffectClass> {
        let idx = id.checked_sub(FIRST_GOSFX_CLASS_ID)?;
        EffectClass::ALL.get(usize::try_from(idx).ok()?).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            EffectClass::Effect => "Effect",
            EffectClass::ParticleCloud => "ParticleCloud",
            EffectClass::PointCloud => "PointCloud",
            EffectClass::SpinningCloud => "SpinningCloud",
            EffectClass::ShardCloud => "ShardCloud",
            EffectClass::PertCloud => "PertCloud",
            EffectClass::CardCloud => "CardCloud",
            EffectClass::ShapeCloud => "ShapeCloud",
            EffectClass::EffectCloud => "EffectCloud",
            EffectClass::Singleton => "Singleton",
            EffectClass::Card => "Card",
            EffectClass::Shape => "Shape",
            EffectClass::Tube => "Tube",
            EffectClass::DebrisCloud => "DebrisCloud",
            EffectClass::PointLight => "PointLight",
        }
    }
}

/// `gosFX::Event`: a child effect scheduled at a fraction of the parent's life.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub time: f32,
    pub flags: u32,
    pub effect_id: u32,
    pub local_to_parent: Mat,
}

/// `Effect__Specification`.
#[derive(Clone, Debug)]
pub struct BaseSpec {
    pub name: String,
    pub effect_id: u32,
    pub events: Vec<Event>,
    pub life_span: ConstantCurve,
    pub min_child_seed: SplineCurve,
    pub max_child_seed: SplineCurve,
    pub state: MlrState,
}

impl Default for BaseSpec {
    /// `Effect__Specification::BuildDefaults`: life span 1, child seeds spanning 0..1.
    fn default() -> Self {
        BaseSpec {
            name: String::new(),
            effect_id: 0,
            events: Vec::new(),
            life_span: ConstantCurve { value: 1.0 },
            min_child_seed: SplineCurve::default(),
            max_child_seed: SplineCurve {
                value: 1.0,
                ..SplineCurve::default()
            },
            state: MlrState::default(),
        }
    }
}

impl BaseSpec {
    fn load(s: &mut Stream, gfx_version: i32) -> Result<BaseSpec> {
        if gfx_version < 9 {
            return Err(FxError::new(
                "This version of gosFX is no longer supported (< 9)",
            ));
        }
        let name = s.mstring()?;
        let event_count = s.count(4 + 4 + 4 + 48)?;
        let mut events = Vec::with_capacity(event_count);
        for _ in 0..event_count {
            events.push(Event {
                time: s.f32()?,
                flags: s.u32()?,
                effect_id: s.u32()?,
                local_to_parent: s.mat()?,
            });
        }
        let life_span = ConstantCurve::load(s, gfx_version)?;
        let min_child_seed = SplineCurve::load(s, gfx_version)?;
        let max_child_seed = SplineCurve::load(s, gfx_version)?;
        let mlr_version = read_mlr_version(s)?;
        let mut state = MlrState::load(s, mlr_version)?;
        if gfx_version < 14 {
            state.render_permission_mask |= TEXTURE_MASK;
        }
        if state.blend() != crate::state::BlendMode::OneZero && state.priority() < ALPHA_PRIORITY {
            state.set_priority(ALPHA_PRIORITY);
        }
        Ok(BaseSpec {
            name,
            effect_id: 0,
            events,
            life_span,
            min_child_seed,
            max_child_seed,
            state,
        })
    }
}

/// `ParticleCloud__Specification`.
#[derive(Clone, Debug, Default)]
pub struct ParticleSpec {
    pub starting_population: ConstantCurve,
    pub particles_per_second: ComplexCurve,
    pub emitter_size_x: LinearCurve,
    pub emitter_size_y: LinearCurve,
    pub emitter_size_z: LinearCurve,
    pub minimum_deviation: ConstantCurve,
    pub maximum_deviation: SplineCurve,
    pub starting_speed: ComplexComplex,
    pub p_life_span: ComplexSpline,
    pub p_ether_velocity_x: ConstantLinear,
    pub p_ether_velocity_y: ConstantLinear,
    pub p_ether_velocity_z: ConstantLinear,
    pub p_acceleration_x: SplineLinear,
    pub p_acceleration_y: SplineLinear,
    pub p_acceleration_z: SplineLinear,
    pub p_drag: ComplexComplex,
    pub p_red: ComplexLinear,
    pub p_green: ComplexLinear,
    pub p_blue: ComplexLinear,
    pub p_alpha: ComplexLinear,
    pub max_particle_count: i32,
}

impl ParticleSpec {
    fn load(s: &mut Stream, v: i32) -> Result<ParticleSpec> {
        Ok(ParticleSpec {
            starting_population: ConstantCurve::load(s, v)?,
            particles_per_second: ComplexCurve::load(s, v)?,
            emitter_size_x: LinearCurve::load(s, v)?,
            emitter_size_y: LinearCurve::load(s, v)?,
            emitter_size_z: LinearCurve::load(s, v)?,
            minimum_deviation: ConstantCurve::load(s, v)?,
            maximum_deviation: SplineCurve::load(s, v)?,
            starting_speed: FCurve::load(s, v)?,
            p_life_span: FCurve::load(s, v)?,
            p_ether_velocity_x: FCurve::load(s, v)?,
            p_ether_velocity_y: FCurve::load(s, v)?,
            p_ether_velocity_z: FCurve::load(s, v)?,
            p_acceleration_x: FCurve::load(s, v)?,
            p_acceleration_y: FCurve::load(s, v)?,
            p_acceleration_z: FCurve::load(s, v)?,
            p_drag: FCurve::load(s, v)?,
            p_red: FCurve::load(s, v)?,
            p_green: FCurve::load(s, v)?,
            p_blue: FCurve::load(s, v)?,
            p_alpha: FCurve::load(s, v)?,
            max_particle_count: s.i32()?,
        })
    }
}

/// `SpinningCloud__Specification`.
#[derive(Clone, Debug, Default)]
pub struct SpinSpec {
    pub particle: ParticleSpec,
    pub p_spin: ConstantLinear,
    pub p_scale: ComplexComplex,
    pub random_starting_rotation: bool,
    pub align_y_using_velocity: bool,
    pub align_z_using_x: bool,
    pub align_z_using_y: bool,
}

impl SpinSpec {
    fn load(s: &mut Stream, v: i32) -> Result<SpinSpec> {
        let particle = ParticleSpec::load(s, v)?;
        let p_spin = FCurve::load(s, v)?;
        let p_scale = FCurve::load(s, v)?;
        let random_starting_rotation = s.bool()?;
        let align_z_using_x = s.bool()?;
        let align_z_using_y = s.bool()?;
        let align_y_using_velocity = if v >= 10 { s.bool()? } else { false };
        Ok(SpinSpec {
            particle,
            p_spin,
            p_scale,
            random_starting_rotation,
            align_y_using_velocity,
            align_z_using_x,
            align_z_using_y,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShardCloudSpec {
    pub spin: SpinSpec,
    pub size: ComplexSpline,
    pub angularity: ConstantSpline,
}

#[derive(Clone, Debug, Default)]
pub struct PertCloudSpec {
    pub spin: SpinSpec,
    pub size: ComplexSpline,
    pub perturbation: ConstantSpline,
    pub p_center_red: ComplexLinear,
    pub p_center_green: ComplexLinear,
    pub p_center_blue: ComplexLinear,
    pub p_center_alpha: ComplexLinear,
    pub vertices: u32,
}

/// The UV-animation block shared by `CardCloud` and `Card`.
#[derive(Clone, Debug, Default)]
pub struct CardUv {
    pub u_offset: ConstantCurve,
    pub v_offset: ConstantCurve,
    pub u_size: ConstantCurve,
    pub v_size: ConstantCurve,
    pub animated: bool,
    /// `SetWidth`: how many frames fit across the texture, `BYTE(1 / uSize(0))`.
    pub width: u8,
}

impl CardUv {
    /// The gfx version < 10 layout: no index curve (a constant 0 that is not seeded) and two
    /// animated offset channels that the loader collapses to their value at age 0, then the
    /// size curves.
    fn load_old(&mut self, s: &mut Stream, v: i32, index: &mut ComplexSpline) -> Result<()> {
        index.age_curve.set_constant(0.0);
        index.seed_curve = SplineCurve {
            value: 1.0,
            ..SplineCurve::default()
        };
        index.seeded = false;
        let temp = ComplexLinear::load(s, v)?;
        self.u_offset.set_curve(temp.compute(0.0, 0.0));
        let temp = ComplexLinear::load(s, v)?;
        self.v_offset.set_curve(temp.compute(0.0, 0.0));
        self.u_size = ConstantCurve::load(s, v)?;
        self.v_size = ConstantCurve::load(s, v)?;
        self.animated = false;
        Ok(())
    }

    fn set_width(&mut self) {
        // `static_cast<BYTE>(float)`: truncate, wrapping modulo 256 like the x86 cast.
        let w = 1.0 / self.u_size.compute(0.0, 0.0);
        self.width = if w.is_finite() { (w as i64) as u8 } else { 0 };
    }
}

#[derive(Clone, Debug, Default)]
pub struct CardCloudSpec {
    pub spin: SpinSpec,
    pub half_height: ComplexComplex,
    pub aspect_ratio: ComplexComplex,
    pub p_index: ComplexSpline,
    pub uv: CardUv,
}

#[derive(Clone, Debug)]
pub struct ShapeCloudSpec {
    pub spin: SpinSpec,
    pub shape: MlrShape,
    pub radius: f32,
}

#[derive(Clone, Debug, Default)]
pub struct EffectCloudSpec {
    pub spin: SpinSpec,
    pub particle_effect_id: u32,
}

/// `Singleton__Specification`.
#[derive(Clone, Debug, Default)]
pub struct SingletonSpec {
    pub red: ComplexLinear,
    pub green: ComplexLinear,
    pub blue: ComplexLinear,
    pub alpha: ComplexLinear,
    pub scale: ComplexComplex,
    pub align_z_using_x: bool,
    pub align_z_using_y: bool,
}

impl SingletonSpec {
    fn load(s: &mut Stream, v: i32) -> Result<SingletonSpec> {
        Ok(SingletonSpec {
            red: FCurve::load(s, v)?,
            green: FCurve::load(s, v)?,
            blue: FCurve::load(s, v)?,
            alpha: FCurve::load(s, v)?,
            scale: FCurve::load(s, v)?,
            align_z_using_x: s.bool()?,
            align_z_using_y: s.bool()?,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CardSpec {
    pub singleton: SingletonSpec,
    pub half_height: ConstantComplex,
    pub aspect_ratio: ConstantComplex,
    pub index: ComplexSpline,
    pub uv: CardUv,
}

#[derive(Clone, Debug)]
pub struct ShapeSpec {
    pub singleton: SingletonSpec,
    pub shape: MlrShape,
    pub radius: f32,
}

/// `Tube__Specification::ProfileType`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ProfileType {
    #[default]
    Ribbon,
    AlignedRibbon,
    Triangle,
    Square,
    Cross,
    Pentagon,
    Hexagon,
    VerticalRibbon,
}

impl ProfileType {
    fn from_i32(v: i32) -> Result<ProfileType> {
        Ok(match v {
            0 => ProfileType::Ribbon,
            1 => ProfileType::AlignedRibbon,
            2 => ProfileType::Triangle,
            3 => ProfileType::Square,
            4 => ProfileType::Cross,
            5 => ProfileType::Pentagon,
            6 => ProfileType::Hexagon,
            7 => ProfileType::VerticalRibbon,
            _ => return Err(FxError::new(format!("unknown tube profile type {v}"))),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TubeSpec {
    pub profiles_per_second: ConstantCurve,
    pub p_life_span: ConstantCurve,
    pub emitter_size_x: LinearCurve,
    pub emitter_size_y: LinearCurve,
    pub emitter_size_z: LinearCurve,
    pub minimum_deviation: ConstantCurve,
    pub maximum_deviation: ComplexCurve,
    pub p_displacement: ComplexSpline,
    pub p_scale: ComplexSpline,
    pub p_red: ComplexLinear,
    pub p_green: ComplexLinear,
    pub p_blue: ComplexLinear,
    pub p_alpha: ComplexLinear,
    pub p_u_offset: ComplexSpline,
    pub p_v_offset: ComplexSpline,
    pub p_u_size: SplineSpline,
    pub p_v_size: SplineSpline,
    pub max_profile_count: i32,
    pub profile_type: ProfileType,
    pub inside_out: bool,
    /// `BuildTemplate`: the profile cross-section and its V coordinates.
    pub vertices: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub u_bias: f32,
}

impl TubeSpec {
    /// `Tube__Specification::BuildTemplate`.
    pub fn build_template(profile: ProfileType) -> (Vec<Vec3>, Vec<[f32; 2]>) {
        let v = Vec3::new;
        let uv = |y: f32| [0.0, y];
        match profile {
            ProfileType::Ribbon | ProfileType::AlignedRibbon => (
                vec![v(1.0, 0.0, 0.0), v(-1.0, 0.0, 0.0)],
                vec![uv(0.0), uv(1.0)],
            ),
            ProfileType::VerticalRibbon => (
                vec![v(0.0, 0.0, -1.0), v(0.0, 0.0, 1.0)],
                vec![uv(0.0), uv(1.0)],
            ),
            ProfileType::Triangle => (
                vec![
                    v(0.0, 0.0, 1.0),
                    v(-0.866_025_4, 0.0, -0.5),
                    v(0.866_025_4, 0.0, -0.5),
                    v(0.0, 0.0, 1.0),
                ],
                vec![uv(0.0), uv(1.0 / 3.0), uv(2.0 / 3.0), uv(1.0)],
            ),
            ProfileType::Square => (
                vec![
                    v(0.0, 0.0, 1.0),
                    v(-1.0, 0.0, 0.0),
                    v(0.0, 0.0, -1.0),
                    v(1.0, 0.0, 0.0),
                    v(0.0, 0.0, 1.0),
                ],
                vec![uv(0.0), uv(0.25), uv(0.5), uv(0.75), uv(1.0)],
            ),
            ProfileType::Cross => (
                vec![
                    v(0.0, 0.0, 0.0),
                    v(0.0, 0.0, 1.0),
                    v(-1.0, 0.0, 0.0),
                    v(0.0, 0.0, -1.0),
                    v(1.0, 0.0, 0.0),
                ],
                vec![uv(0.0), uv(1.0), uv(1.0), uv(1.0), uv(1.0)],
            ),
            ProfileType::Pentagon => (
                vec![
                    v(0.0, 0.0, 1.0),
                    v(-0.951_056_5, 0.0, 0.309_017),
                    v(-0.587_785_25, 0.0, -0.809_016_7),
                    v(0.587_785_25, 0.0, -0.809_016_7),
                    v(0.951_056_5, 0.0, 0.309_017),
                    v(0.0, 0.0, 1.0),
                ],
                vec![uv(0.0), uv(0.2), uv(0.4), uv(0.6), uv(0.8), uv(1.0)],
            ),
            ProfileType::Hexagon => (
                vec![
                    v(0.0, 0.0, 1.0),
                    v(-0.866_025_4, 0.0, 0.5),
                    v(-0.866_025_4, 0.0, -0.5),
                    v(0.0, 0.0, -1.0),
                    v(0.866_025_4, 0.0, -0.5),
                    v(0.866_025_4, 0.0, 0.5),
                    v(0.0, 0.0, 1.0),
                ],
                vec![
                    uv(0.0),
                    uv(1.0 / 6.0),
                    uv(2.0 / 6.0),
                    uv(0.5),
                    uv(4.0 / 6.0),
                    uv(5.0 / 6.0),
                    uv(1.0),
                ],
            ),
        }
    }

    /// `Tube__Specification::CalculateUBias(true)`. When the worst-case U range does not fit in
    /// 198 the U-size age curve is replaced by a constant small enough to fit (and unseeded)
    /// and the calculation retried. The C++ retries forever if the offsets alone are too wide;
    /// here the retry count is bounded and the bias then stays 0.
    fn calculate_u_bias(&mut self) {
        let max_index = (self.max_profile_count - 1) as f32;
        for _ in 0..8 {
            let (min_scale, max_scale) = self.p_u_size.range();
            let mut lower = min_scale * max_index;
            if lower > 0.0 {
                lower = 0.0;
            }
            let mut upper = max_scale * max_index;
            let (min_offset, max_offset) = self.p_u_offset.range();
            lower += min_offset;
            upper += max_offset;
            if upper - lower >= 198.0 {
                let width = max_offset - min_offset;
                let scale = ((198.0 - width) / max_index).floor();
                self.p_u_size.age_curve = SplineCurve {
                    value: scale,
                    ..SplineCurve::default()
                };
                self.p_u_size.seeded = false;
                continue;
            }
            lower += 99.0;
            if lower < 0.0 {
                self.u_bias = (0.5 - lower).floor();
                return;
            }
            upper -= 99.0;
            if upper > 0.0 {
                self.u_bias = (0.5 - upper).floor();
                return;
            }
            self.u_bias = 0.0;
            return;
        }
        self.u_bias = 0.0;
    }

    fn load(s: &mut Stream, v: i32) -> Result<TubeSpec> {
        let profiles_per_second = ConstantCurve::load(s, v)?;
        let p_life_span = ConstantCurve::load(s, v)?;
        let emitter_size_x = LinearCurve::load(s, v)?;
        let emitter_size_y = LinearCurve::load(s, v)?;
        let emitter_size_z = LinearCurve::load(s, v)?;
        let minimum_deviation = ConstantCurve::load(s, v)?;
        let maximum_deviation = ComplexCurve::load(s, v)?;
        let p_displacement = FCurve::load(s, v)?;
        let p_scale = if v < 12 {
            // Old data has no scale curve: a constant 1 that is not seeded.
            let mut c = ComplexSpline::default();
            c.age_curve.set_constant(1.0);
            c.seed_curve.value = 1.0;
            c.seeded = false;
            c
        } else {
            FCurve::load(s, v)?
        };
        let p_red = FCurve::load(s, v)?;
        let p_green = FCurve::load(s, v)?;
        let p_blue = FCurve::load(s, v)?;
        let p_alpha = FCurve::load(s, v)?;
        let p_u_offset = FCurve::load(s, v)?;
        let p_v_offset = FCurve::load(s, v)?;
        let p_u_size = FCurve::load(s, v)?;
        let p_v_size = FCurve::load(s, v)?;
        let max_profile_count = s.i32()?;
        let profile_type = ProfileType::from_i32(s.i32()?)?;
        let inside_out = s.bool()?;
        let (vertices, uvs) = TubeSpec::build_template(profile_type);
        let mut t = TubeSpec {
            profiles_per_second,
            p_life_span,
            emitter_size_x,
            emitter_size_y,
            emitter_size_z,
            minimum_deviation,
            maximum_deviation,
            p_displacement,
            p_scale,
            p_red,
            p_green,
            p_blue,
            p_alpha,
            p_u_offset,
            p_v_offset,
            p_u_size,
            p_v_size,
            max_profile_count,
            profile_type,
            inside_out,
            vertices,
            uvs,
            u_bias: 0.0,
        };
        t.calculate_u_bias();
        Ok(t)
    }
}

/// One piece of a `DebrisCloud`.
#[derive(Clone, Debug)]
pub struct DebrisPiece {
    pub position: Vec3,
    pub sphere_center: Vec3,
    pub sphere_radius: f32,
    pub shape: MlrShape,
    /// `debrisSeed`: the piece's radius scaled to `0..1` across all pieces.
    pub seed: f32,
}

#[derive(Clone, Debug)]
pub struct DebrisCloudSpec {
    pub minimum_deviation: ConstantCurve,
    pub maximum_deviation: SplineCurve,
    pub starting_speed: ComplexComplex,
    pub p_life_span: ComplexSpline,
    pub p_ether_velocity_y: ConstantLinear,
    pub p_acceleration_y: SplineLinear,
    pub p_drag: ComplexComplex,
    pub p_alpha: ComplexLinear,
    pub p_spin: ConstantLinear,
    pub center_of_force: Vec3,
    pub pieces: Vec<DebrisPiece>,
}

#[derive(Clone, Debug)]
pub struct PointLightSpec {
    pub red: ComplexCurve,
    pub green: ComplexCurve,
    pub blue: ComplexCurve,
    pub intensity: ComplexCurve,
    pub inner_radius: SplineCurve,
    pub outer_radius: SplineCurve,
    pub two_sided: bool,
    pub light_map: String,
}

#[derive(Clone, Debug)]
pub enum SpecKind {
    /// The base `Effect`: only schedules child events.
    Effect,
    PointCloud(Box<ParticleSpec>),
    ShardCloud(Box<ShardCloudSpec>),
    PertCloud(Box<PertCloudSpec>),
    CardCloud(Box<CardCloudSpec>),
    ShapeCloud(Box<ShapeCloudSpec>),
    EffectCloud(Box<EffectCloudSpec>),
    Card(Box<CardSpec>),
    Shape(Box<ShapeSpec>),
    Tube(Box<TubeSpec>),
    DebrisCloud(Box<DebrisCloudSpec>),
    PointLight(Box<PointLightSpec>),
}

/// One loaded effect specification.
#[derive(Clone, Debug)]
pub struct EffectSpec {
    pub base: BaseSpec,
    pub kind: SpecKind,
}

impl EffectSpec {
    /// A specification built in code (tools and tests).
    pub fn new(base: BaseSpec, kind: SpecKind) -> EffectSpec {
        EffectSpec { base, kind }
    }

    pub fn class(&self) -> EffectClass {
        match &self.kind {
            SpecKind::Effect => EffectClass::Effect,
            SpecKind::PointCloud(_) => EffectClass::PointCloud,
            SpecKind::ShardCloud(_) => EffectClass::ShardCloud,
            SpecKind::PertCloud(_) => EffectClass::PertCloud,
            SpecKind::CardCloud(_) => EffectClass::CardCloud,
            SpecKind::ShapeCloud(_) => EffectClass::ShapeCloud,
            SpecKind::EffectCloud(_) => EffectClass::EffectCloud,
            SpecKind::Card(_) => EffectClass::Card,
            SpecKind::Shape(_) => EffectClass::Shape,
            SpecKind::Tube(_) => EffectClass::Tube,
            SpecKind::DebrisCloud(_) => EffectClass::DebrisCloud,
            SpecKind::PointLight(_) => EffectClass::PointLight,
        }
    }

    pub fn name(&self) -> &str {
        &self.base.name
    }

    /// `Effect__Specification::Create`: the class id, then that class's `Load` constructor.
    pub fn create(s: &mut Stream, gfx_version: i32) -> Result<EffectSpec> {
        let class_id = s.i32()?;
        let class = EffectClass::from_class_id(class_id)
            .ok_or_else(|| FxError::new(format!("unknown gosFX class id {class_id}")))?;
        let base = BaseSpec::load(s, gfx_version)?;
        let v = gfx_version;
        let kind = match class {
            EffectClass::Effect => SpecKind::Effect,
            EffectClass::PointCloud => SpecKind::PointCloud(Box::new(ParticleSpec::load(s, v)?)),
            EffectClass::ShardCloud => {
                let spin = SpinSpec::load(s, v)?;
                let size = FCurve::load(s, v)?;
                let angularity = FCurve::load(s, v)?;
                SpecKind::ShardCloud(Box::new(ShardCloudSpec {
                    spin,
                    size,
                    angularity,
                }))
            }
            EffectClass::PertCloud => {
                let spin = SpinSpec::load(s, v)?;
                let size = FCurve::load(s, v)?;
                let perturbation = FCurve::load(s, v)?;
                let p_center_red = FCurve::load(s, v)?;
                let p_center_green = FCurve::load(s, v)?;
                let p_center_blue = FCurve::load(s, v)?;
                let p_center_alpha = FCurve::load(s, v)?;
                if v <= 5 {
                    return Err(FxError::new("PertCloud needs gfx version > 5"));
                }
                let vertices = s.u32()?;
                if !(5..=1024).contains(&vertices) {
                    return Err(FxError::new(format!("pert cloud vertex count {vertices}")));
                }
                SpecKind::PertCloud(Box::new(PertCloudSpec {
                    spin,
                    size,
                    perturbation,
                    p_center_red,
                    p_center_green,
                    p_center_blue,
                    p_center_alpha,
                    vertices,
                }))
            }
            EffectClass::CardCloud => {
                let spin = SpinSpec::load(s, v)?;
                let half_height = FCurve::load(s, v)?;
                let aspect_ratio = FCurve::load(s, v)?;
                let mut uv = CardUv::default();
                let mut p_index = ComplexSpline::default();
                if v < 10 {
                    uv.load_old(s, v, &mut p_index)?;
                } else {
                    p_index = FCurve::load(s, v)?;
                    uv.u_offset = ConstantCurve::load(s, v)?;
                    uv.v_offset = ConstantCurve::load(s, v)?;
                    uv.u_size = ConstantCurve::load(s, v)?;
                    uv.v_size = ConstantCurve::load(s, v)?;
                    uv.animated = s.bool()?;
                }
                uv.set_width();
                SpecKind::CardCloud(Box::new(CardCloudSpec {
                    spin,
                    half_height,
                    aspect_ratio,
                    p_index,
                    uv,
                }))
            }
            EffectClass::ShapeCloud => {
                let spin = SpinSpec::load(s, v)?;
                let mlr = read_mlr_version(s)?;
                let shape = MlrShape::load(s, mlr)?;
                let radius = s.f32()?;
                SpecKind::ShapeCloud(Box::new(ShapeCloudSpec {
                    spin,
                    shape,
                    radius,
                }))
            }
            EffectClass::EffectCloud => {
                let spin = SpinSpec::load(s, v)?;
                let particle_effect_id = s.u32()?;
                SpecKind::EffectCloud(Box::new(EffectCloudSpec {
                    spin,
                    particle_effect_id,
                }))
            }
            EffectClass::Card => {
                let singleton = SingletonSpec::load(s, v)?;
                let half_height = FCurve::load(s, v)?;
                let aspect_ratio = FCurve::load(s, v)?;
                let mut uv = CardUv::default();
                let mut index = ComplexSpline::default();
                if v < 10 {
                    uv.load_old(s, v, &mut index)?;
                } else {
                    index = FCurve::load(s, v)?;
                    uv.u_offset = ConstantCurve::load(s, v)?;
                    uv.v_offset = ConstantCurve::load(s, v)?;
                    uv.u_size = ConstantCurve::load(s, v)?;
                    uv.v_size = ConstantCurve::load(s, v)?;
                    uv.animated = s.bool()?;
                }
                uv.set_width();
                SpecKind::Card(Box::new(CardSpec {
                    singleton,
                    half_height,
                    aspect_ratio,
                    index,
                    uv,
                }))
            }
            EffectClass::Shape => {
                let singleton = SingletonSpec::load(s, v)?;
                let mlr = read_mlr_version(s)?;
                let shape = MlrShape::load(s, mlr)?;
                let radius = s.f32()?;
                SpecKind::Shape(Box::new(ShapeSpec {
                    singleton,
                    shape,
                    radius,
                }))
            }
            EffectClass::Tube => SpecKind::Tube(Box::new(TubeSpec::load(s, v)?)),
            EffectClass::DebrisCloud => {
                let minimum_deviation = ConstantCurve::load(s, v)?;
                let maximum_deviation = SplineCurve::load(s, v)?;
                let starting_speed = FCurve::load(s, v)?;
                let p_life_span = FCurve::load(s, v)?;
                let p_ether_velocity_y = FCurve::load(s, v)?;
                let p_acceleration_y = FCurve::load(s, v)?;
                let p_drag = FCurve::load(s, v)?;
                let p_alpha = FCurve::load(s, v)?;
                let p_spin = FCurve::load(s, v)?;
                let center_of_force = s.vec3()?;
                // LoadGeometry
                let mlr = read_mlr_version(s)?;
                let n = s.count(12 + 12 + 4)?;
                let mut pieces = Vec::with_capacity(n);
                let mut min_radius = 100_000_000_000.0f32;
                let mut max_radius = 0.0f32;
                for _ in 0..n {
                    let position = s.vec3()?;
                    let sphere_center = s.vec3()?;
                    let sphere_radius = s.f32()?;
                    min_radius = min_radius.min(sphere_radius);
                    max_radius = max_radius.max(sphere_radius);
                    let shape = MlrShape::load(s, mlr)?;
                    pieces.push(DebrisPiece {
                        position,
                        sphere_center,
                        sphere_radius,
                        shape,
                        seed: 0.0,
                    });
                }
                for p in pieces.iter_mut() {
                    // `Verify(maxRadius > minRadius)` in the C++; equal radii give 0 here.
                    p.seed = if max_radius > min_radius {
                        (p.sphere_radius - min_radius) / (max_radius - min_radius)
                    } else {
                        0.0
                    };
                }
                SpecKind::DebrisCloud(Box::new(DebrisCloudSpec {
                    minimum_deviation,
                    maximum_deviation,
                    starting_speed,
                    p_life_span,
                    p_ether_velocity_y,
                    p_acceleration_y,
                    p_drag,
                    p_alpha,
                    p_spin,
                    center_of_force,
                    pieces,
                }))
            }
            EffectClass::PointLight => {
                let red = ComplexCurve::load(s, v)?;
                let green = ComplexCurve::load(s, v)?;
                let blue = ComplexCurve::load(s, v)?;
                let intensity = ComplexCurve::load(s, v)?;
                // Before gfx version 16 the radii were not stored: 0 and 25.
                let mut inner_radius = SplineCurve::default();
                let mut outer_radius = SplineCurve {
                    value: 25.0,
                    ..SplineCurve::default()
                };
                let mut light_map = String::new();
                if v > 15 {
                    inner_radius = SplineCurve::load(s, v)?;
                    outer_radius = SplineCurve::load(s, v)?;
                    if v > 16 {
                        light_map = s.mstring()?;
                    }
                }
                let two_sided = s.bool()?;
                SpecKind::PointLight(Box::new(PointLightSpec {
                    red,
                    green,
                    blue,
                    intensity,
                    inner_radius,
                    outer_radius,
                    two_sided,
                    light_map,
                }))
            }
            EffectClass::ParticleCloud | EffectClass::SpinningCloud | EffectClass::Singleton => {
                return Err(FxError::new(format!(
                    "class {} is abstract and has no specification factory",
                    class.name()
                )))
            }
        };
        Ok(EffectSpec { base, kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(vals: &[f32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn old_card_data_keeps_the_offsets_at_age_zero() {
        // gfx < 10: two Seeded<Complex, Linear> offset channels, then U and V size.
        let mut b = Vec::new();
        for offset in [0.25f32, 0.75] {
            b.extend(f(&[0.0])); // complex curves before gfx 15 start with a duration
            b.extend(1u32.to_le_bytes());
            b.extend(f(&[0.0, 0.0, offset])); // one key: constant `offset`
            b.extend(f(&[1.0, 0.0])); // seed curve
            b.push(0); // not seeded
        }
        b.extend(f(&[0.5, 0.5]));
        let mut uv = CardUv::default();
        let mut index = ComplexSpline::default();
        uv.load_old(&mut Stream::new(&b), 9, &mut index).unwrap();
        assert_eq!((uv.u_offset.value, uv.v_offset.value), (0.25, 0.75));
        assert_eq!((uv.u_size.value, uv.v_size.value), (0.5, 0.5));
        assert!(!uv.animated);
        assert_eq!(index.compute(0.7, 0.3), 0.0);
        uv.set_width();
        assert_eq!(uv.width, 2);
    }

    #[test]
    fn tube_u_bias_squeezes_a_too_wide_u_size() {
        // 8 profiles, U size 40 per profile: 40 * 7 = 280 does not fit in 198. The size curve
        // becomes floor(198 / 7) = 28 and, unseeded, 28 * 7 = 196 fits; the bias then centres
        // the range: upper - 99 = 97 -> floor(0.5 - 97) = -97.
        let mut t = TubeSpec {
            max_profile_count: 8,
            ..TubeSpec::default()
        };
        t.p_u_size.age_curve.value = 40.0;
        t.p_u_size.seed_curve.value = 1.0;
        t.calculate_u_bias();
        assert_eq!(t.p_u_size.age_curve.value, 28.0);
        assert!(!t.p_u_size.seeded);
        assert_eq!(t.u_bias, -97.0);
    }

    #[test]
    fn tube_u_bias_zero_for_a_centred_range() {
        let mut t = TubeSpec {
            max_profile_count: 3,
            ..TubeSpec::default()
        };
        t.p_u_size.age_curve.value = 1.0;
        t.p_u_size.seed_curve.value = 1.0;
        t.calculate_u_bias();
        assert_eq!(t.u_bias, 0.0);
    }

    #[test]
    fn class_ids_map_to_classes() {
        assert_eq!(EffectClass::from_class_id(1312), Some(EffectClass::Effect));
        assert_eq!(
            EffectClass::from_class_id(1318),
            Some(EffectClass::CardCloud)
        );
        assert_eq!(
            EffectClass::from_class_id(1326),
            Some(EffectClass::PointLight)
        );
        assert_eq!(EffectClass::from_class_id(1327), None);
        assert_eq!(EffectClass::from_class_id(5), None);
    }
}

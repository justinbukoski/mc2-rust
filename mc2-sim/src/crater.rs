// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `CraterManager` (`mclib/crater.{h,cpp}`): the ring of terrain decals (bomb craters and mech footprints).
//!
//! The simulation only raises [`crate::SimEvent::Crater`] (`craterManager->addCrater(type, position, rotation)`); whoever draws
//! them owns a [`CraterPool`] and feeds it from those events. The pool is the C++ list exactly: `maxCraters` slots written in
//! turn (`currentCrater` wraps to 0, so the oldest decal is replaced), a decal is four corners rotated by `OppRotate` around the
//! impact point and dropped onto the terrain. There is no lifetime and no fade in the original: a decal lives until its slot is
//! reused.

use crate::math::{Vec3, DEGREES_TO_RADS};

/// `INVALID_CRATER`: an empty slot.
pub const INVALID_CRATER: i32 = -1;
/// `SML_FOOTPRINT`.
pub const SML_FOOTPRINT: i32 = 0;
/// `TURKINA_FOOTPRINT`: the last footprint id.
pub const TURKINA_FOOTPRINT: i32 = 63;
/// `CRATER_1`: the first bomb crater; `CRATER_2..4` follow.
pub const CRATER_1: i32 = 64;
/// `CRATER_4`.
pub const CRATER_4: i32 = 67;
/// `MAX_CRATER_SHAPES`.
pub const MAX_CRATER_SHAPES: i32 = 68;
/// `MAX_FOOTPRINTS` (crater.cpp:48): ids above it are 32 unit craters, the rest 16 unit footprints.
pub const MAX_FOOTPRINTS: i32 = 64;
/// `craterManager->init(1000, ...)` (saveload.cpp:1057).
pub const MAX_CRATERS: usize = 1000;
/// `FOOTPRINT_SLOP` (mech3d.cpp:181): frames either side of a foot-down frame that still count.
pub const FOOTPRINT_SLOP: i32 = 2;

/// The lower left texture coordinate of a shape in its texture (the intent of `craterUVTable`, crater.cpp:56): footprints
/// are an 8x8 grid of 0.125 cells in row order, the four craters a 2x2 grid of 0.5 cells. The C++ table lists only 58
/// footprints and the craters right behind them, but indexes it by the shape id (64..67), which reads the table's zero
/// fill, so every crater drew the first picture; the grid is the intended layout (DIFFERENCES.md, "Persistent craters").
pub fn shape_uv_origin(shape_id: i32) -> [f32; 2] {
    if shape_id >= CRATER_1 {
        let i = (shape_id - CRATER_1).clamp(0, 3);
        [(i % 2) as f32 * 0.5, (i / 2) as f32 * 0.5]
    } else {
        let i = shape_id.clamp(0, MAX_FOOTPRINTS - 1);
        [(i % 8) as f32 * 0.125, (i / 8) as f32 * 0.125]
    }
}

/// One decal: the shape id and its four corners (`CraterData`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crater {
    /// `craterShapeId`.
    pub shape_id: i32,
    /// `position[4]`: the corners, `z` on the terrain.
    pub corners: [Vec3; 4],
}

impl Crater {
    /// A bomb crater uses the crater texture (`feet0000`, 2x2 shapes of 0.5), a footprint the foot texture (`feet0001`,
    /// 8x8 shapes of 0.125): `craterShapeId >= MAX_FOOTPRINTS` in `render`.
    pub fn is_crater(&self) -> bool {
        self.shape_id >= MAX_FOOTPRINTS
    }

    /// Lower left texture coordinate and the shape's extent in the texture (`uvAdd`).
    pub fn uv(&self) -> ([f32; 2], f32) {
        (
            shape_uv_origin(self.shape_id),
            if self.is_crater() { 0.5 } else { 0.125 },
        )
    }
}

/// `CraterManager`: the slots and the next one to write.
#[derive(Clone, Debug, PartialEq)]
pub struct CraterPool {
    slots: Vec<Option<Crater>>,
    current: usize,
}

impl Default for CraterPool {
    fn default() -> Self {
        CraterPool::new(MAX_CRATERS)
    }
}

impl CraterPool {
    /// `init(numCraters, ..)`: `max` empty slots.
    pub fn new(max: usize) -> CraterPool {
        CraterPool {
            slots: vec![None; max.max(1)],
            current: 0,
        }
    }

    /// `maxCraters`.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// The decals now in the pool (slots with a shape id).
    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Whether no decal is recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The next slot `addCrater` writes (`currentCrater`).
    pub fn next_slot(&self) -> usize {
        self.current
    }

    /// The decals in slot order (the order `render` walks them).
    pub fn iter(&self) -> impl Iterator<Item = &Crater> {
        self.slots.iter().flatten()
    }

    /// A slot, `None` when empty.
    pub fn slot(&self, i: usize) -> Option<&Crater> {
        self.slots.get(i).and_then(|c| c.as_ref())
    }

    /// Forgets every decal (a new mission).
    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|s| *s = None);
        self.current = 0;
    }

    /// `addCrater(craterType, position, rotation)` (crater.cpp:196): the corners of a 16 (footprint) or 32 (crater) unit
    /// half-width square, turned by `rotation` degrees about the origin with `OppRotate`, moved to `position` and set on
    /// the ground by `elevation` (`land->getTerrainElevation`). The slot is the next one in turn, wrapping to the first.
    pub fn add(
        &mut self,
        crater_type: i32,
        position: &Vec3,
        rotation: f32,
        elevation: &dyn Fn(&Vec3) -> f32,
    ) {
        // `craterShapeId > MAX_FOOTPRINTS` (crater.cpp:216): `CRATER_1` (64) is not above it, so it and the footprints get the
        // 16 unit half width, `CRATER_2..4` 32. Kept as it plays.
        let size = if crater_type > MAX_FOOTPRINTS {
            32.0
        } else {
            16.0
        };
        let mut corners = [
            Vec3 {
                x: -size,
                y: -size,
                z: 0.0,
            },
            Vec3 {
                x: size,
                y: -size,
                z: 0.0,
            },
            Vec3 {
                x: size,
                y: size,
                z: 0.0,
            },
            Vec3 {
                x: -size,
                y: size,
                z: 0.0,
            },
        ];
        for c in corners.iter_mut() {
            opp_rotate(c, rotation);
            c.x += position.x;
            c.y += position.y;
            c.z += position.z;
            c.z = elevation(c);
        }
        self.slots[self.current] = Some(Crater {
            shape_id: crater_type,
            corners,
        });
        self.current += 1;
        if self.current == self.slots.len() {
            self.current = 0;
        }
    }
}

/// `OppRotate(vec, angle)` (mathfunc.cpp:60): the x/y rotation of `Rotate` in the other direction.
pub fn opp_rotate(v: &mut Vec3, angle: f32) {
    let tx = v.x;
    let (sine, cosine) = mc2_assets::stuff::sin_cos(angle * DEGREES_TO_RADS);
    v.x *= cosine;
    v.x -= v.y * sine;
    v.y *= cosine;
    v.y += tx * sine;
}

/// `craterType` of a bolt that missed and hit land (weaponbolt.cpp:808): by the shot's damage, `CRATER_1..CRATER_4`.
pub fn bolt_crater_type(damage: f32) -> i32 {
    let level = if damage > 9.0 {
        3
    } else if damage > 6.0 {
        2
    } else if damage > 3.0 {
        1
    } else {
        0
    };
    CRATER_1 + level
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: &Vec3) -> f32 {
        7.0
    }

    #[test]
    fn big_craters_are_32_and_footprints_16_on_the_ground() {
        let mut p = CraterPool::new(4);
        let at = Vec3 {
            x: 100.0,
            y: 200.0,
            z: 0.0,
        };
        p.add(CRATER_1 + 1, &at, 0.0, &flat);
        p.add(SML_FOOTPRINT, &at, 0.0, &flat);
        p.add(CRATER_1, &at, 0.0, &flat);
        let c = p.slot(0).unwrap();
        assert!(c.is_crater());
        assert_eq!((c.corners[0].x, c.corners[0].y), (68.0, 168.0));
        assert_eq!((c.corners[2].x, c.corners[2].y), (132.0, 232.0));
        assert!(c.corners.iter().all(|k| k.z == 7.0));
        let f = p.slot(1).unwrap();
        assert!(!f.is_crater());
        assert_eq!((f.corners[0].x, f.corners[0].y), (84.0, 184.0));
        // CRATER_1 is not above MAX_FOOTPRINTS: the footprint size
        assert_eq!(p.slot(2).unwrap().corners[0].x, 84.0);
    }

    #[test]
    fn opp_rotate_turns_counter_clockwise() {
        let mut v = Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        opp_rotate(&mut v, 90.0);
        assert!(v.x.abs() < 1e-5 && (v.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn crater_type_by_damage() {
        assert_eq!(bolt_crater_type(3.0), 64);
        assert_eq!(bolt_crater_type(3.5), 65);
        assert_eq!(bolt_crater_type(7.0), 66);
        assert_eq!(bolt_crater_type(9.5), 67);
    }

    #[test]
    fn uv_table_matches_the_shape_grid() {
        let c = |id| Crater {
            shape_id: id,
            corners: [Vec3::default(); 4],
        };
        assert_eq!(c(1).uv(), ([0.125, 0.0], 0.125));
        assert_eq!(c(8).uv(), ([0.0, 0.125], 0.125));
        assert_eq!(c(57).uv(), ([0.125, 0.875], 0.125));
        assert_eq!(c(63).uv(), ([0.875, 0.875], 0.125));
        assert_eq!(c(CRATER_1).uv(), ([0.0, 0.0], 0.5));
        assert_eq!(c(CRATER_1 + 1).uv(), ([0.5, 0.0], 0.5));
        assert_eq!(c(CRATER_1 + 2).uv(), ([0.0, 0.5], 0.5));
        assert_eq!(c(CRATER_1 + 3).uv(), ([0.5, 0.5], 0.5));
    }
}

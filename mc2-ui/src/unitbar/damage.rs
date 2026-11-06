// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Armour to damage state, and the recolouring of an icon's art (`code/mechicon.cpp`).
//!
//! The icon art paints each location in one of three colours per damage state. The colour of a
//! location is its state's colour with the red channel raised by the location's index, so each
//! location's pixels can be found and recoloured on their own (`doDraw`, `mechicon.cpp:780-871`).

use crate::textures::{TexId, TextureStore};
use crate::unitbar::{Loc, MechArmor, VehicleArmor, ICON_H, ICON_W};

/// `ForceGroupIcon::damageColors` (`code/mechicon.cpp:54-59`): `[state][part]`, states 0 (intact)
/// to 3 (destroyed, grey), parts the three shades of the icon.
pub const DAMAGE_COLORS: [[u32; 3]; 4] = [
    [rgb(194, 229, 255), rgb(92, 150, 194), rgb(0, 83, 146)],
    [rgb(248, 241, 193), rgb(248, 206, 31), rgb(139, 114, 0)],
    [rgb(248, 193, 193), rgb(248, 31, 31), rgb(139, 0, 0)],
    [rgb(94, 101, 101), rgb(56, 64, 64), rgb(26, 33, 33)],
];

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    0xff00_0000 | (r << 16) | (g << 8) | b
}

/// `MechIcon::update`'s state of a mech's eight drawn locations (`code/mechicon.cpp:572-778`, with
/// `bDrawBack` false, as the bar draws it). Index: 0 left leg, 1 right leg, 2 left arm, 3 right arm,
/// 4 right torso, 5 left torso, 6 centre torso, 7 head.
pub fn mech_states(a: &MechArmor) -> [u8; 8] {
    [
        part_state(&a.lleg),
        part_state(&a.rleg),
        part_state(&a.larm),
        part_state(&a.rarm),
        mech_state(torso(&a.rtorso, &a.rrtorso)),
        mech_state(torso(&a.ltorso, &a.rltorso)),
        mech_state(torso(&a.ctorso, &a.rctorso)),
        part_state(&a.head),
    ]
}

/// `VehicleIcon::update`'s state of a vehicle's five locations (`code/mechicon.cpp:1235-1361`):
/// front, rear, left, right, turret. The rest of the array is zero.
pub fn vehicle_states(v: &VehicleArmor) -> [u8; 8] {
    [
        vehicle_state(v.front.ratio()),
        vehicle_state(v.rear.ratio()),
        vehicle_state(v.left.ratio()),
        vehicle_state(v.right.ratio()),
        vehicle_state(v.turret.ratio()),
        0,
        0,
        0,
    ]
}

/// A mech's state from its ratio: below 0 (destroyed) is 3, below .3 is 2, below .6 is 1
/// (`mechicon.cpp:589-626`).
fn mech_state(t: f32) -> u8 {
    if t < 0.0 {
        3
    } else if t < 0.3 {
        2
    } else if t < 0.6 {
        1
    } else {
        0
    }
}

/// A vehicle's state from its ratio: below .1 is 3, below .3 is 2, below .6 is 1
/// (`mechicon.cpp:1245-1286`).
fn vehicle_state(t: f32) -> u8 {
    if t < 0.1 {
        3
    } else if t < 0.3 {
        2
    } else if t < 0.6 {
        1
    } else {
        0
    }
}

/// A single-location part: a destroyed body part reads as -1, else its armour ratio
/// (`mechicon.cpp:706-752`, the head at `:582-585`).
fn part_state(l: &Loc) -> u8 {
    mech_state(if l.destroyed { -1.0 } else { l.ratio() })
}

/// A torso: the front armour averaged with the rear armour behind it when the rear location has
/// armour (`mechicon.cpp:596-602`, `:629-634`, `:673-678`). The average divides by two even when the
/// rear has no maximum, as the C++ does. A destroyed torso reads as -1 (`:608-612`).
fn torso(front: &Loc, rear: &Loc) -> f32 {
    let mut t = front.ratio();
    if rear.max != 0.0 {
        t += rear.ratio();
    }
    t /= 2.0;
    if front.destroyed {
        -1.0
    } else {
        t
    }
}

/// `true` when any of the first `count` states changed (the C++ only recolours then).
pub fn differs(old: &[u8; 8], new: &[u8; 8], count: usize) -> bool {
    old[..count] != new[..count]
}

/// `doDraw` / `VehicleIcon::update`: recolours the locations whose state changed, then records the new
/// states. `pixels` is one icon's RGBA bytes.
pub fn apply(pixels: &mut [u8], old: &mut [u8; 8], new: &[u8; 8], count: usize) {
    for i in 0..count {
        if old[i] != new[i] {
            recolor(pixels, i as u32, old[i], new[i]);
        }
    }
    old[..count].copy_from_slice(&new[..count]);
}

/// The ARGB colour a location paints one of `part` in state `state`: the damage colour with its red
/// channel raised by the location index (`mechicon.cpp:828-838`).
fn key(state: u8, part: usize, loc: u32) -> u32 {
    let tmp = DAMAGE_COLORS[state as usize][part];
    let red = ((tmp >> 16) & 0xff) + loc;
    (tmp & 0xff00_ffff) | (red << 16)
}

/// Recolours the opaque pixels of location `loc` from state `old` to state `new`
/// (`mechicon.cpp:843-863`). Pixels are RGBA bytes; a pixel must match the old colour exactly,
/// alpha included, and transparent pixels are left alone (`mechicon.cpp:849`).
pub fn recolor(pixels: &mut [u8], loc: u32, old: u8, new: u8) {
    let old_c: [u32; 3] = std::array::from_fn(|p| key(old, p, loc));
    let new_c: [u32; 3] = std::array::from_fn(|p| key(new, p, loc));
    for px in pixels.chunks_exact_mut(4) {
        let mut v = argb(px);
        if v & 0xff00_0000 != 0 {
            for p in 0..3 {
                if old_c[p] == v {
                    v = new_c[p];
                }
            }
            set_argb(px, v);
        }
    }
}

/// `0xAARRGGBB` of an RGBA pixel.
pub fn argb(px: &[u8]) -> u32 {
    (u32::from(px[3]) << 24) | (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2])
}

fn set_argb(px: &mut [u8], v: u32) {
    px[0] = (v >> 16) as u8;
    px[1] = (v >> 8) as u8;
    px[2] = v as u8;
    px[3] = (v >> 24) as u8;
}

/// `MechIcon::init` / `VehicleIcon::init`: copies one icon's cell (`ICON_W` by `ICON_H`, row 0 of the
/// atlas, column `col`) out of an atlas. A cell outside the atlas copies as transparent.
pub fn copy_cell(textures: &TextureStore, atlas: TexId, col: u32) -> Vec<u8> {
    let mut out = vec![0u8; (ICON_W * ICON_H * 4) as usize];
    let Some(img) = textures.get(atlas) else {
        log::warn!("unit bar atlas {atlas:?} is not loaded");
        return out;
    };
    let x0 = col * ICON_W;
    if x0 + ICON_W > img.width || ICON_H > img.height {
        log::warn!("unit bar icon {col} is outside its atlas");
        return out;
    }
    let row_bytes = (ICON_W * 4) as usize;
    for y in 0..ICON_H {
        let src = ((y * img.width + x0) * 4) as usize;
        let dst = (y * ICON_W * 4) as usize;
        out[dst..dst + row_bytes].copy_from_slice(&img.rgba[src..src + row_bytes]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Loc {
        Loc {
            cur: 1.0,
            max: 1.0,
            destroyed: false,
        }
    }

    fn mech_full() -> MechArmor {
        MechArmor {
            head: full(),
            ctorso: full(),
            rctorso: full(),
            ltorso: full(),
            rltorso: full(),
            rtorso: full(),
            rrtorso: full(),
            larm: full(),
            rarm: full(),
            lleg: full(),
            rleg: full(),
        }
    }

    fn at(ratio: f32) -> Loc {
        Loc {
            cur: ratio,
            max: 1.0,
            destroyed: false,
        }
    }

    #[test]
    fn mech_at_full_armour_is_intact() {
        assert_eq!(mech_states(&mech_full()), [0; 8]);
    }

    #[test]
    fn mech_thresholds_follow_mechicon_cpp() {
        // head 0.2 is below .3: state 2 (mechicon.cpp:582-618, index 7)
        let mut a = mech_full();
        a.head = at(0.2);
        assert_eq!(mech_states(&a)[7], 2);
        // left arm 0.5: below .6 is 1 (mechicon.cpp:734-746, index 2)
        a = mech_full();
        a.larm = at(0.5);
        assert_eq!(mech_states(&a)[2], 1);
        // right arm exactly 0.6 is not below .6: intact (mechicon.cpp:756-761, index 3)
        a = mech_full();
        a.rarm = at(0.6);
        assert_eq!(mech_states(&a)[3], 0);
    }

    #[test]
    fn destroyed_parts_read_as_three() {
        // mechicon.cpp:708-711: a destroyed left leg is -1, state 3 (index 0)
        let mut a = mech_full();
        a.lleg.destroyed = true;
        assert_eq!(mech_states(&a)[0], 3);
        // mechicon.cpp:608-612 and :652-656: a destroyed left torso is 3 even at full armour (index 5)
        a = mech_full();
        a.ltorso.destroyed = true;
        assert_eq!(mech_states(&a)[5], 3);
    }

    #[test]
    fn centre_torso_averages_the_rear_armour_in() {
        // mechicon.cpp:596-602: (1.0 + 0.0) / 2 = 0.5, below .6: state 1 (index 6)
        let mut a = mech_full();
        a.rctorso = at(0.0);
        assert_eq!(mech_states(&a)[6], 1);
        // a rear with no maximum is not added, but the sum still halves: 0.8 / 2 = 0.4: state 1
        a = mech_full();
        a.ctorso = at(0.8);
        a.rctorso = Loc {
            cur: 0.0,
            max: 0.0,
            destroyed: false,
        };
        assert_eq!(mech_states(&a)[6], 1);
        // right torso: (0.1 + 0.1) / 2 = 0.1, below .3: state 2 (mechicon.cpp:673-696, index 4)
        a = mech_full();
        a.rtorso = at(0.1);
        a.rrtorso = at(0.1);
        assert_eq!(mech_states(&a)[4], 2);
    }

    #[test]
    fn vehicle_thresholds_follow_mechicon_cpp() {
        // mechicon.cpp:1245-1286: below .1 is 3, below .3 is 2, below .6 is 1
        let full = Loc {
            cur: 1.0,
            max: 1.0,
            destroyed: false,
        };
        let v = VehicleArmor {
            front: at(0.05),
            rear: at(0.2),
            left: at(0.5),
            right: full,
            turret: at(0.1),
        };
        assert_eq!(vehicle_states(&v), [3, 2, 1, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn recolour_moves_one_location_from_intact_to_red() {
        // mechicon.cpp:828-838: key = damageColors[0][0] with red + loc, loc 3 gives red 194 + 3 = 197
        let mut px = [0xc5, 0xe5, 0xff, 0xff]; // ARGB 0xFFC5E5FF
        recolor(&mut px, 3, 0, 2);
        // damageColors[2][0] = 0xFFF8C1C1 with red 248 + 3 = 251: 0xFFFBC1C1
        assert_eq!(argb(&px), 0xfffb_c1c1);
    }

    #[test]
    fn recolour_leaves_other_pixels_alone() {
        // a transparent pixel of the key colour (mechicon.cpp:849), a pixel of another location (red
        // 194 + 2), and an unrelated colour
        let mut px = Vec::new();
        for v in [0x00c5_e5ffu32, 0xffc4_e5ff, 0xff12_3456, 0xff5f_96c2] {
            px.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
        }
        recolor(&mut px, 3, 0, 2);
        let out: Vec<u32> = px.chunks_exact(4).map(argb).collect();
        assert_eq!(out[0], 0x00c5_e5ff);
        assert_eq!(out[1], 0xffc4_e5ff);
        assert_eq!(out[2], 0xff12_3456);
        // the middle shade: 0xFF5C96C2 with red 92 + 3 = 95 is 0xFF5F96C2; state 2 shade 1 is
        // 0xFFF81F1F with red 251: 0xFFFB1F1F
        assert_eq!(out[3], 0xfffb_1f1f);
    }

    #[test]
    fn apply_recolours_only_changed_locations_and_records_states() {
        let mut old = [0u8; 8];
        let new = [0, 0, 0, 0, 0, 0, 1, 2];
        let mut px = vec![0u8; 4];
        // one pixel of location 6's intact colour: red 194 + 6 = 200
        px.copy_from_slice(&[200, 229, 255, 255]);
        assert!(differs(&old, &new, 8));
        apply(&mut px, &mut old, &new, 8);
        assert_eq!(old, new);
        // location 6 became state 1, shade 0: red 248 + 6 = 254, G 241, B 193
        assert_eq!(argb(&px), 0xfffe_f1c1);
        assert!(!differs(&old, &new, 8));
    }

    #[test]
    fn copy_cell_takes_row_zero_of_the_column() {
        let (w, h) = (ICON_W * 2, ICON_H);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // a marker in column 1 (x 32), row 0
        let at = (32 * 4) as usize;
        rgba[at..at + 4].copy_from_slice(&[1, 2, 3, 255]);
        let mut store = TextureStore::new();
        let atlas: TexId = store.insert("atlas", w, h, rgba);
        let cell = copy_cell(&store, atlas, 1);
        assert_eq!(cell.len(), (ICON_W * ICON_H * 4) as usize);
        assert_eq!(&cell[..4], &[1, 2, 3, 255]);
        // a column past the atlas copies as transparent
        assert!(copy_cell(&store, atlas, 2).iter().all(|&b| b == 0));
    }
}

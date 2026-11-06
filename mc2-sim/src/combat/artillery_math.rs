// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Artillery strike maths: `code/artlry.cpp`.

use crate::math::Vec3;

/// `MOVETYPE_AIR` from `code/mover.h:742-747`.
pub const MOVETYPE_AIR: i32 = 1;

/// `ARTILLERY_SMALL` (dartlry.h enum order).
pub const ARTILLERY_SMALL: i32 = 0;
/// `ARTILLERY_LARGE` (dartlry.h enum order).
pub const ARTILLERY_LARGE: i32 = 1;
/// `ARTILLERY_SENSOR` (dartlry.h enum order).
pub const ARTILLERY_SENSOR: i32 = 2;

/// The strike type `IfaceCallStrike` passes to `CallArtillery` (artlry.cpp:159-232): `None` for an id that is not a strike type
/// (the C++ returns early), SMALL becomes LARGE, and a LARGE strike becomes SMALL when it is `randomOff` (the target is not in
/// line of sight).
pub fn normalize_strike(strike_id: i32, random_off: bool) -> Option<i32> {
    if !matches!(
        strike_id,
        ARTILLERY_LARGE | ARTILLERY_SMALL | ARTILLERY_SENSOR
    ) {
        return None;
    }
    let mut id = strike_id;
    if id == ARTILLERY_SMALL {
        id = ARTILLERY_LARGE;
    }
    if id == ARTILLERY_LARGE && random_off {
        id = ARTILLERY_SMALL;
    }
    Some(id)
}

/// `CallArtillery`'s time-to-impact (artlry.cpp:130-142). `-1` leaves the type's full duration in place; any value under 2 s is
/// replaced by `-1` too (the "full duration" marker, filled on the first update from `nominalTimeToImpact`).
pub fn strike_time_to_impact(seconds: i64) -> f32 {
    let mut t = -1.0_f32;
    if seconds != -1 {
        t = seconds as f32;
    }
    if seconds < 2 {
        t = -1.0;
    }
    t
}

/// `IfaceCallStrike`'s `(long)timeToImpact` (artlry.cpp:188): truncation toward zero.
pub fn seconds_from_time_to_impact(t: f32) -> i64 {
    t as i64
}

/// The per-commander strike cap. In a multiplayer server `CallArtillery` drops a strike once the commander has 10 live ones
/// (artlry.cpp:110-116); single player has no cap.
pub const fn single_player_strike_cap() -> Option<usize> {
    None
}

/// The hit plan `ArtilleryType::handleCollision` derives for one collider (artlry.cpp:532-617).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitPlan {
    /// `numHits`: the major or minor hit count, truncated from the type's float (`long numHits = float`).
    pub count: i32,
    /// `range <= minArtilleryHeadRange`: the shot's attack source is `ATTACKSOURCE_DFA` when true, else `ATTACKSOURCE_ARTILLERY`.
    pub head_range: bool,
}

/// `isMajorHit` picks `nominalMajorHits` (range <= `nominalMajorRange`) or `nominalMinorHits`; `head_range` is the
/// `minArtilleryHeadRange` test on the same range.
pub fn artillery_hit_plan(
    range_m: f32,
    major_range: f32,
    major_hits: f32,
    minor_hits: f32,
    min_head_range: f32,
) -> HitPlan {
    let is_major_hit = range_m <= major_range;
    let hits = if is_major_hit { major_hits } else { minor_hits };
    HitPlan {
        count: hits as i32,
        head_range: range_m <= min_head_range,
    }
}

/// The gate/turret extent gate (artlry.cpp:540-575): a collider is blocked when the strike is outside its "little"
/// extent by more than the nominal major range.
pub fn extent_blocks_damage(little_extent_m: f32, range_m: f32, major_range: f32) -> bool {
    if little_extent_m < range_m {
        let real_range = range_m - little_extent_m;
        if real_range > major_range {
            return true;
        }
    }
    false
}

/// `Stuff::Vector3D` distance from the strike to the collider in meters (artlry.cpp:537-543). Elevation is ignored.
pub fn range_meters(collider: Vec3, strike: Vec3, meters_per_world_unit: f32) -> f32 {
    let dx = collider.x - strike.x;
    let dy = collider.y - strike.y;
    // `distance.z = 0; distance.GetLength() * metersPerWorldUnit`: `GetLength` takes the square root in double
    let len = ((dx * dx + dy * dy) as f64).sqrt() as f32;
    len * meters_per_world_unit
}

/// Artillery immunity predicate for airborne mechs.
/// Returns `true` when the mech is airborne (`move_type == MOVETYPE_AIR`) and not shutting down
/// (`status != ObjectStatus::ShutDown`), matching `code/artlry.cpp:586-592`.
pub fn airborne_mech_immune(move_type: i32, status: crate::object::ObjectStatus) -> bool {
    move_type == MOVETYPE_AIR && status != crate::object::ObjectStatus::ShutDown
}

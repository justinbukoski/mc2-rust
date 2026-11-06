// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Artillery` strikes (artlry.cpp): `CallArtillery` minimum time to impact 2 s. Single player has no per-commander cap; only a
//! multiplayer server caps a commander at 10 strikes (see `artillery_math::single_player_strike_cap`).
//!
//! The impact effect, the incoming-strike sound, the bomber flyover (artlry.cpp:938-962, 2.5 s before impact) and its contrail are
//! presentation events ([`crate::SimEvent::AirstrikeFlyover`], [`crate::SimEvent::AirstrikeContrail`]) shared by every strike,
//! whoever called it.
//! A sensor strike (`nominalDamage == 0`, a sensor range) takes a sensor from the
//! merged sensor pool (`SensorManager::newSensor`, artlry.cpp:825 + contact.cpp:1221) for the type's `nominalSensorTime` at the
//! type's `nominalSensorRange`, owned by the striking team, and releases it when the countdown ends (artlry.cpp:1097-1128).

use mc2_sim_data::objtype::ObjectTypeData;

use crate::ai::sensors;
use crate::combat::artillery_math::{
    airborne_mech_immune, artillery_hit_plan, extent_blocks_damage, range_meters,
    strike_time_to_impact, ARTILLERY_SENSOR,
};
use crate::combat::{ArtilleryState, AttackSource, WeaponShotInfo};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::{flags, hooks, Kind, ObjectClass, ObjectStore, Pool};
use crate::terrain::METERS_PER_WORLD_UNIT;
use crate::World;

/// `strikeObjectId[NUM_ARTILLERY_TYPES]` (objmgr.cpp:2748): `SMALL_ARTLRY`, `BIG_ARTLRY`, `SENSOR_ARTLRY` (code/dobjnum.h:221-223).
pub const STRIKE_OBJECT_IDS: [i32; 3] = [0xf9, 0xf8, 0xfa];
/// The bomber appears this many seconds before impact (artlry.cpp:938).
pub const BOMB_RUN_TIME: f32 = 2.5;
/// `JET_CONTRAIL_ID` (mclib/weaponfx.h:26).
pub const JET_CONTRAIL_ID: i32 = 58;
/// `INCOMING_AIRSTRIKE` (mclib/sounds.h:75).
pub const INCOMING_AIRSTRIKE: i32 = 63;

/// Reported (in [`crate::FrameOutput::unavailable`], same frame) when a new strike reuses the pool slot of a strike that already
/// detonated; the strike is refused. The C++ keeps that slot occupied until the gosFX hit effect ends and would then refuse the strike
/// (`getArtillery`, objmgr.cpp:757-772); the effect lifetime is unported, so Rust refuses the strike (no object created) and reports this hook.
pub const SLOT_REUSE_HOOK: &str =
    "gosFX hit-effect lifetime: artillery pool slot of a detonated strike reused (C++ may refuse the strike)";

/// Reported when a sensor strike finds the sensor pool full. The C++ `Fatal`s ("No More Free Sensors", contact.cpp:1223-1224);
/// Rust refuses the strike (no object is created) and reports this hook.
pub const SENSOR_POOL_EXHAUSTED_HOOK: &str =
    "sensor pool exhausted: a sensor strike cannot take a SensorManager slot";

/// The artillery type data of a strike object.
fn type_data(w: &World, h: ObjectHandle) -> Option<&mc2_sim_data::objtype::ArtilleryType> {
    match &w.objects.type_of(h)?.data {
        ObjectTypeData::Artillery(a) => Some(a),
        _ => None,
    }
}

fn state_mut(w: &mut World, h: ObjectHandle) -> Option<&mut ArtilleryState> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Artillery(a) => Some(a),
        _ => None,
    }
}

/// `Artillery::setJustCreated` (artlry.cpp:760): the first update sets the timers from the type, and a sensor round takes its
/// sensor from the pool (`SensorManager->newSensor()`, artlry.cpp:825) and joins the striking team with
/// `Artillery::setSensorData` (artlry.cpp:1302-1317): the strike owns the sensor, the team adds it, it gets the type's range, and
/// it has no line-of-sight capability ("ALWAYS true UNLESS this is a sensor probe or a sensor tower!!", contact.h:111).
///
/// DESIGN pin (headless): the effects the C++ creates here are created by the app when the strike events arrive; the water test
/// that only decrements `effectId` (artlry.cpp:776-781) has no sim effect.
pub fn set_just_created(w: &mut World, h: ObjectHandle) {
    let Some(t) = type_data(w, h) else { return };
    let (impact, launch, sensor_range) = (
        t.nominal_time_to_impact,
        t.nominal_time_to_launch,
        t.nominal_sensor_range,
    );
    if let Some(o) = w.objects.get_mut(h) {
        o.c.set_flag(flags::JUSTCREATED, false);
        o.c.set_flag(flags::TANGIBLE, false);
        if let Kind::Artillery(a) = &mut o.kind {
            a.just_created = false;
            if a.time_to_impact == -1.0 {
                a.time_to_impact = impact;
            }
            a.time_to_launch = launch;
            a.sensor_range = sensor_range;
            a.time_to_blind = 0.0;
            a.sensors_going = false;
        }
    }
    if sensor_range == 0.0 {
        return;
    }
    // if (info.strike.sensorRange) { newSensor; setSensorData(getTeam()) } (artlry.cpp:815-827)
    let team = state_mut(w, h).map_or(-1, |a| a.team);
    let Some(id) = sensors::new_sensor(w) else {
        // "No More Free Sensors" is fatal in C++; the strike keeps its sensor range and never gets a sensor.
        w.note_frame_unavailable(SENSOR_POOL_EXHAUSTED_HOOK);
        return;
    };
    sensors::add_team_sensor(w, team, id);
    if let Some(sensor) = w.sensors.pool.get_mut(id) {
        sensor.owner = h;
        sensor.range = sensor_range;
        sensor.has_los_capability = false;
    }
    if let Some(a) = state_mut(w, h) {
        a.sensor_system_index = id as i32;
    }
}

/// `Artillery::update` (artlry.cpp:852). Returns whether the strike still exists.
///
/// DESIGN pin ("headless: no lingering effects"): the C++ strike lives on until its gosFX hit effect has finished. Here a boomed
/// strike lives exactly one more frame: the update that sets `boom` makes it tangible, the collision pass of that frame delivers the
/// damage, and the next update returns false and clears `exists`.
pub fn update(w: &mut World, h: ObjectHandle) -> bool {
    let frame_length = w.clock.frame_length;
    let Some(t) = type_data(w, h) else {
        return false;
    };
    let damage = t.nominal_damage;
    let sensor_time = t.nominal_sensor_time;
    let nominal_sensor_range = t.nominal_sensor_range;
    let Some(a) = state_mut(w, h) else {
        return false;
    };
    if a.just_created {
        set_just_created(w, h);
    } else {
        a.time_to_impact -= frame_length;
        a.time_to_launch -= frame_length;
    }
    let a = state_mut(w, h).expect("artillery");
    if a.boom && damage > 0.0 {
        if let Some(o) = w.objects.get_mut(h) {
            o.c.set_flag(flags::TANGIBLE, false);
            o.set_exists(false);
        }
        return false;
    }
    let a = state_mut(w, h).expect("artillery");
    let (impact, whoosh, boom) = (a.time_to_impact, a.whoosh, a.boom);
    let (bomb_run_started, strike_type) = (a.bomb_run_started, a.strike_type);
    // `if (bomber && timeToImpact <= 2.5f && !bombRunStarted)` (artlry.cpp:938): only the small and large strikes own a bomber.
    if impact <= BOMB_RUN_TIME && !bomb_run_started && strike_type != 2 {
        if let Some(a) = state_mut(w, h) {
            a.bomb_run_started = true;
        }
        let strike = w.objects.watch_id(h);
        if let Some(at) = w.objects.get(h).map(|o| o.c.position) {
            w.events.push(crate::SimEvent::AirstrikeFlyover {
                strike,
                strike_type,
                at,
                time_to_impact: impact,
            });
            w.events.push(crate::SimEvent::AirstrikeContrail {
                strike,
                effect_id: JET_CONTRAIL_ID,
                at,
            });
        }
    }
    let a = state_mut(w, h).expect("artillery");
    let mut sound = None;
    if impact <= 5.5 && !whoosh {
        // soundSystem->playDigitalSample(INCOMING_AIRSTRIKE, position, true). The C++ gate also requires nominalDamage > 0
        // (artlry.cpp:1044-1051): a sensor probe makes no incoming sound.
        if damage > 0.0 {
            a.whoosh = true;
            sound = Some(INCOMING_AIRSTRIKE);
        }
    }
    if !boom && impact <= 0.0 && damage > 0.0 {
        a.boom = true;
        let effect_id = match a.strike_type {
            0 => 51, // SMALL_AIRSTRIKE_ID
            1 => 50, // LARGE_AIRSTRIKE_ID
            _ => 72, // SENSOR_AIRSTRIKE_ID
        };
        if let Some(o) = w.objects.get_mut(h) {
            o.c.set_flag(flags::TANGIBLE, true);
            let at = o.c.position;
            // The impact's explosion object type carries the sound (carnage.cpp:677-678), as createExplosion does.
            let sound_id = w.objects.explosion_sound_id(effect_id);
            w.events.push(crate::SimEvent::Explosion {
                effect_id,
                sound_id,
                at,
                radius: 0.0,
            });
        }
    }
    if let Some(id) = sound {
        let source = w.objects.watch_id(h);
        let at = w.objects.get(h).map(|o| o.c.position);
        w.events.push(crate::SimEvent::Sound {
            id,
            source,
            at,
            allow_dupes: true,
        });
    }
    // Sensor round countdown (artlry.cpp:1097-1128), in the C++ order: tick, then expire, then start.
    let a = state_mut(w, h).expect("artillery");
    if a.time_to_blind > 0.0 && a.sensors_going {
        a.time_to_blind -= frame_length;
        // sensorRange = timeToBlind / nominalSensorTime * nominalSensorRange * worldUnitsPerMeter (artlry.cpp:1101-1103), then
        // sensor->setRange(sensorRange * metersPerWorldUnit) (artlry.cpp:1106). The two conversions are the shipped
        // `worldUnitsPerMeter = 5.01f` (mclib/terrain.cpp:63) and `metersPerWorldUnit = 0.2f` (mclib/terrain.cpp:64), which are NOT
        // exact inverses, so the countdown range carries their product 1.002: a sensor strike starts its countdown at
        // `nominalSensorRange * 1.002` meters, not `nominalSensorRange` (see DIFFERENCES.md). The state field keeps the world-unit
        // figure like `info.strike.sensorRange` does.
        let mut range = a.time_to_blind / sensor_time;
        range *= nominal_sensor_range;
        range *= crate::terrain::WORLD_UNITS_PER_METER;
        a.sensor_range = range;
        // sensor->setRange(info.strike.sensorRange * metersPerWorldUnit) (artlry.cpp:1105-1106)
        let id = a.sensor_system_index;
        if id >= 0 {
            if let Some(sensor) = w.sensors.pool.get_mut(id as usize) {
                sensor.range = range * METERS_PER_WORLD_UNIT;
            }
        }
        // "Actual scan is done in team update..." -- the merged SensorManager::update scans it like any team sensor.
    } else if a.time_to_blind <= 0.0 && a.sensors_going {
        // All done: removeTeamSensor(getTeam()->getId(), sensor); freeSensor(sensor); return 0 (artlry.cpp:1113-1121).
        let (id, team) = state_mut(w, h).map_or((-1, -1), |a| (a.sensor_system_index, a.team));
        if id >= 0 {
            sensors::remove_team_sensor(w, team, id as usize);
            sensors::free_sensor(w, id as usize);
        }
        if let Some(o) = w.objects.get_mut(h) {
            o.set_exists(false);
        }
        return false;
    } else if sensor_time > 0.0 {
        // "If we are a sensor round, start the sensor countdown. ALWAYS on, even before effect hits." (artlry.cpp:1123-1127)
        a.time_to_blind = sensor_time;
        a.sensors_going = true;
    }
    true
}

/// `GameObjectManager::createArtillery(artilleryType, position)` (objmgr.cpp:2757): the next pool slot (`None` while the slot is
/// still in use), `init(false, type)` (class, exists, just created, no boom, time to impact -1), `setPosition`, `setExists(true)`.
pub fn create_artillery(w: &mut World, strike_type: i32, position: Vec3) -> Option<ObjectHandle> {
    let n = w.objects.artillery.len();
    if n > 0 {
        let next = w.objects.artillery[(w.objects.artillery_index + 1) % n];
        let reused = w
            .objects
            .get(next)
            .is_some_and(|o| !o.exists() && matches!(&o.kind, Kind::Artillery(a) if a.boom));
        if reused {
            // C++ may still hold this slot for the hit effect (objmgr.cpp:757-773); the lifetime is unported, so the strike
            // is refused (no object, no pool advance) and reported instead of being accepted on a guess.
            w.note_frame_unavailable(SLOT_REUSE_HOOK);
            return None;
        }
    }
    let h = ObjectStore::next_pool_slot(w, Pool::Artillery)?;
    let type_num = *STRIKE_OBJECT_IDS.get(usize::try_from(strike_type).ok()?)?;
    w.objects.type_by_num(type_num)?;
    let o = w.objects.get_mut(h)?;
    o.c.type_handle = type_num;
    o.c.class = ObjectClass::Artillery;
    o.c.set_flag(flags::JUSTCREATED, true);
    if let Kind::Artillery(a) = &mut o.kind {
        a.just_created = true;
        a.boom = false;
        a.time_to_impact = -1.0;
        a.strike_type = strike_type;
        // Artillery::init (artlry.cpp:642): info.strike.sensorSystemIndex = -1.
        a.sensor_system_index = -1;
    }
    crate::object::set_position(w, h, position, true);
    w.objects.get_mut(h)?.set_exists(true);
    Some(h)
}

/// `CallArtillery(commanderID, strikeType, strikeLoc, secondsToImpact, randomOff)` (artlry.cpp:99) for single player: no strike cap.
/// `seconds` is `secondsToImpact` (-1: the type's full duration; under 2 s also means full duration). A sensor strike with the pool
/// full creates nothing and returns false (the C++ `Fatal`s; see [`SENSOR_POOL_EXHAUSTED_HOOK`]).
pub fn call_strike_seconds(
    w: &mut World,
    commander: i8,
    strike_type: i32,
    at: Vec3,
    seconds: i64,
) -> bool {
    if strike_type == ARTILLERY_SENSOR && sensors::free_sensor_slot(w).is_none() {
        // The C++ takes the sensor inside `Artillery::setJustCreated`, after `createArtillery` has consumed the strike slot.
        // Rust checks the sensor pool first so a refused strike leaves no half-created strike behind; the outcome (no strike)
        // is the C++ outcome of the fatal it never survives.
        w.note_frame_unavailable(SENSOR_POOL_EXHAUSTED_HOOK);
        return false;
    }
    let Some(h) = create_artillery(w, strike_type, at) else {
        return false;
    };
    let team = usize::try_from(commander)
        .ok()
        .and_then(|c| w.teams.commanders.get(c).cloned().flatten())
        .map_or(-1, |c| i32::from(c.team));
    if let Some(o) = w.objects.get_mut(h) {
        o.c.position = at;
    }
    if let Some(a) = state_mut(w, h) {
        a.i_face_position = at;
        a.team = team;
        a.commander = commander;
        a.time_to_impact = strike_time_to_impact(seconds);
    }
    // call this so if in pause mode, there is something to draw
    update(w, h);
    true
}

/// [`call_strike_seconds`] with the type's full duration (`secondsToImpact == -1`).
pub fn call_strike(w: &mut World, commander: i8, strike_type: i32, at: Vec3) -> bool {
    call_strike_seconds(w, commander, strike_type, at, -1)
}

/// `ArtilleryType::handleCollision(collidee = the strike, collider)` (artlry.cpp:532-617), single player. Damage is only done by a
/// strike that has boomed. Always returns false (the strike is not consumed by a collision).
///
/// A mech with `MOVETYPE_AIR` that is not shut down is immune (artlry.cpp:586-592): the `else if` after the gate/turret branch.
pub fn handle_collision(w: &mut World, collidee: ObjectHandle, collider: ObjectHandle) -> bool {
    let Some(a) = w.objects.get(collidee).and_then(|o| match &o.kind {
        Kind::Artillery(a) => Some(a.boom),
        _ => None,
    }) else {
        return false;
    };
    if !a {
        return false;
    }
    let Some(t) = type_data(w, collidee) else {
        return false;
    };
    let (major_range, major_hits, minor_hits, damage) = (
        t.nominal_major_range,
        t.nominal_major_hits,
        t.nominal_minor_hits,
        t.nominal_damage,
    );
    let min_head_range = t
        .explosions
        .as_ref()
        .map_or(5, |e| e.min_artillery_head_range) as f32;
    let (Some(cp), Some(sp)) = (
        w.objects.get(collider).map(|o| o.c.position),
        w.objects.get(collidee).map(|o| o.c.position),
    ) else {
        return false;
    };
    let range = range_meters(cp, sp, METERS_PER_WORLD_UNIT);
    let is_gate_or_turret = matches!(
        w.objects.get(collider).map(|o| &o.kind),
        Some(Kind::Gate(_)) | Some(Kind::Turret(_))
    );
    if is_gate_or_turret {
        // An explosion may NOT damage a gate or turret unless it is within the "little" extent radius.
        let little_extent =
            crate::object::structure::little_extent(w, collider) * METERS_PER_WORLD_UNIT;
        if extent_blocks_damage(little_extent, range, major_range) {
            return false;
        }
    } else if let Some(o) = w.objects.get(collider) {
        if let (Kind::Mech(_), Some(ObjectTypeData::BattleMech(m))) =
            (&o.kind, w.objects.type_of(collider).map(|t| &t.data))
        {
            if airborne_mech_immune(m.move_type, o.c.status) {
                return false;
            }
        }
    }
    let plan = artillery_hit_plan(range, major_range, major_hits, minor_hits, min_head_range);
    let artillery_wid = w.objects.watch_id(collidee);
    let is_mover = w.objects.get(collider).is_some_and(|o| o.is_mover());
    for _ in 0..plan.count {
        let mut shot = WeaponShotInfo::init(w, artillery_wid, -3, damage, 0, 0.0);
        if is_mover {
            let source = if plan.head_range {
                AttackSource::Dfa
            } else {
                AttackSource::Artillery
            };
            shot.hit_location = hooks::calc_hit_location(w, collider, collidee, -1, source, 0);
            let (pos, rot) = {
                let o = w.objects.get(collider).expect("collider");
                (o.c.position, o.c.rotation)
            };
            shot.set_entry_angle(crate::math::rel_facing_to(&pos, rot, &sp));
        }
        hooks::weapon_hit(w, collider, &shot);
    }
    false
}

/// The sensor pool slot of the strike `h` (`info.strike.sensorSystemIndex`), or -1 before it owns one.
pub fn sensor_system_index(w: &World, h: ObjectHandle) -> i32 {
    match &w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Artillery(a)) => a.sensor_system_index,
        _ => -1,
    }
}

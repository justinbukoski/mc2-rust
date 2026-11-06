// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The support-vehicle VTOL: `MissionInterfaceManager::beginVtol` (missiongui.cpp:4240), `updateVTol` (:918), `addVehicle` (:4214)
//! and `BringInReinforcement` (:4149).
//!
//! The player buys a support vehicle (repair truck, scout, mine layer, guard tower) or a recovery craft; the VTOL (or the Karnov for
//! the recovery craft) flies in on the `vtol` / `karnov` building animation and, when its frame counter passes 145, drops the
//! vehicle (a new mover of the home commander); at frame 300 it leaves and the vehicle powers up. The Karnov lands (gesture 1),
//! hovers (gesture 0) for `Mover::recoverTime`, recovers the disabled mech and trades it to the home team, then flies away
//! (gesture 2). The animations are the shipped `vtol.ini` / `karnov.ini` ([`BuildingAppearance`]), so the timing is the data's.
//!
//! The VTOL, its dust cloud and the recovery beam are presentation and are not modelled. Flight sounds are emitted once when each
//! flight begins. The pilot of a recovered mech comes from the host's [`RecoveryPilots`](crate::mission::trade::RecoveryPilots); without one the recovery is refused and
//! reported, never faked (`rust/docs/DIFFERENCES.md`).

use crate::appearance::mech::UpdateCtx;
use crate::appearance::structure::{BuildingAppearance, BuildingAppearanceType};
use crate::events::{fx_id, FxAction, FxEffect, FxPlacement, FxSlot, SimEvent};
use crate::ids::{ObjectHandle, PartId, WatchId, MAX_TEAMS};
use crate::math::Vec3;
use crate::mission::init::MoverInitData;
use crate::object::Kind;
use crate::World;

/// `vehicleIDs[]` / `vehiclePilots[]` (controlgui.cpp:96-120): the object number and pilot file of each support vehicle.
pub const SUPPORT_VEHICLES: [(i32, &str); 5] = [
    (182, "PMWRepair"),
    (393, "PMWScout"),
    (120, "PMWMinelayer"),
    (147, "PMWRecover"),
    (415, "PMWGuardTower"),
];
/// The recovery craft's id: it flies the Karnov and creates no vehicle (missiongui.cpp:4259).
pub const RECOVERY_VEHICLE_ID: i32 = 147;
/// `vTol->currentFrame >= 145`: the VTOL drops its vehicle (missiongui.cpp:926).
pub const DROP_FRAME: f32 = 145.0;
/// `vTol->currentFrame >= 300`: the VTOL is gone and the vehicle powers up (missiongui.cpp:949).
pub const LEAVE_FRAME: f32 = 300.0;
/// The resource points returned when a recovery craft ends without a recovery (missiongui.cpp:1112-1121).
pub const RECOVERY_REFUND: i32 = 10_000;

/// `ControlGui::getVehicleNameFromID`: the pilot file of a support vehicle id.
pub fn support_vehicle_pilot(id: i32) -> Option<&'static str> {
    SUPPORT_VEHICLES
        .iter()
        .find(|(v, _)| *v == id)
        .map(|(_, p)| *p)
}

/// The shipped `vtol` and `karnov` animations.
#[derive(Clone, Debug)]
pub struct VtolAssets {
    /// `appearanceTypeList->getAppearance(BLDG_TYPE << 24, "vtol")`.
    pub vtol: BuildingAppearanceType,
    /// `... "karnov"`.
    pub karnov: BuildingAppearanceType,
}

/// One VTOL in flight (`vTol[commanderID]` and the `paintingVtol` state around it).
#[derive(Clone, Debug)]
pub struct VtolFlight {
    /// `vehicleID[commanderID]`.
    pub vehicle_id: i32,
    /// `vPos[commanderID]`: where the vehicle is dropped.
    pub position: Vec3,
    /// The animation (`vTol[commanderID]`).
    pub animation: BuildingAppearance,
    /// `vehicleDropped`.
    pub dropped: bool,
    /// `reinforcement`: the dropped vehicle, powered up when the VTOL leaves.
    pub reinforcement: WatchId,
    /// `mechToRecover`.
    pub mech: WatchId,
    /// `mechRecovered`.
    pub recovered: bool,
    /// `vTolTime`.
    pub time: f32,
    /// The recovery was refused (the campaign has no pilot): the craft flies away without the mech and its cost is returned.
    pub refused: bool,
    /// Whether the flight-start sound has already been requested.
    pub flight_sound_started: bool,
    /// `recoveryBeam` has been started (it executes only then, missiongui.cpp:1146).
    pub beam_started: bool,
}

/// The VTOLs of every commander (`vTol[]`, `paintingVtol[]`).
#[derive(Clone, Debug, Default)]
pub struct VtolSystem {
    /// The loaded animations (none until the host prepares the world: a purchase is then refused and reported).
    pub assets: Option<VtolAssets>,
    /// `vTol[commanderID]` while `paintingVtol[commanderID]`.
    pub flights: [Option<VtolFlight>; MAX_TEAMS],
    /// The vehicle the commander's last drop created (`MissionInterfaceManager::reinforcement`), for the host to find it.
    pub last_dropped: [Option<PartId>; MAX_TEAMS],
}

impl VtolSystem {
    /// `paintingVtol[commander]`.
    pub fn in_flight(&self, commander: i32) -> bool {
        usize::try_from(commander)
            .ok()
            .and_then(|c| self.flights.get(c))
            .is_some_and(Option::is_some)
    }
}

/// Why a purchase was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VtolRefusal {
    /// `canAddVehicle` / `canRecover` said no (or a VTOL is already in flight): the GUI plays `INVALID_GUI`.
    InvalidPlacement,
    /// The animations or the vehicle's files were not prepared by the host ([`prepare`]).
    NotPrepared,
    /// Not one of the five support vehicles.
    UnknownVehicle,
}

/// Loads what a drop needs, which the C++ opens on demand: the `vtol` and `karnov` animations, and the chassis, appearance types and
/// pilot files of the support vehicles. Called by the host once after the mission load.
pub fn prepare(w: &mut World, vfs: &mut mc2_formats::vfs::Vfs) -> Result<(), String> {
    let types: Vec<u32> = SUPPORT_VEHICLES.iter().map(|(id, _)| *id as u32).collect();
    let pilots: Vec<&str> = SUPPORT_VEHICLES.iter().map(|(_, p)| *p).collect();
    crate::mission::init::prepare_spawn_in(w, vfs, &types, &pilots)?;
    let load = |vfs: &mut mc2_formats::vfs::Vfs, name: &str| {
        BuildingAppearanceType::load(vfs, name).map_err(|e| e.to_string())
    };
    w.vtol.assets = Some(VtolAssets {
        vtol: load(vfs, "vtol")?,
        karnov: load(vfs, "karnov")?,
    });
    Ok(())
}

fn commander_index(commander: i32) -> Option<usize> {
    usize::try_from(commander).ok().filter(|c| *c < MAX_TEAMS)
}

/// `Commander::commanders[id]->team->getId()`.
fn team_of(w: &World, commander: usize) -> Option<i32> {
    w.teams
        .commanders
        .get(commander)
        .and_then(|c| c.as_ref())
        .map(|c| i32::from(c.team))
}

/// `canAddVehicle(pos)` (missiongui.cpp:3646): on the map, seen by the home team, passable and dry.
fn can_add_vehicle(w: &mut World, team: i32, pos: Vec3) -> bool {
    let (tile_r, tile_c) = w.terrain.world_to_tile_rc(&pos);
    let side = w.terrain.vertices_per_side;
    if tile_c <= -1 || tile_c >= side || tile_r <= -1 || tile_r >= side {
        return false;
    }
    if !crate::object::hooks::team_line_of_sight(w, team, pos, 0.0) {
        return false;
    }
    let (row, col) = w.terrain.world_to_cell(&pos);
    w.terrain.get_passable(row, col) && w.terrain.get_water(&pos) == 0
}

/// `canRecover(pos)` (missiongui.cpp:3672): the target is a disabled, not destroyed `BattleMech` the home team sees.
fn can_recover(w: &mut World, team: i32, pos: Vec3, target: Option<ObjectHandle>) -> bool {
    let (tile_r, tile_c) = w.terrain.world_to_tile_rc(&pos);
    let side = w.terrain.vertices_per_side;
    if tile_c <= -1 || tile_c >= side || tile_r <= -1 || tile_r >= side {
        return false;
    }
    if !crate::object::hooks::team_line_of_sight(w, team, pos, 0.0) {
        return false;
    }
    target.and_then(|t| w.objects.get(t)).is_some_and(|o| {
        matches!(o.kind, Kind::Mech(_)) && o.c.is_disabled() && !o.c.is_destroyed()
    })
}

/// `MissionInterfaceManager::beginVtol(-1, commanderID, NULL, salvageTarget)` after the click chain's `canAddVehicle` / `canRecover`
/// test (missiongui.cpp:1324-1331). `pos` is the clicked point; a recovery craft hovers over the target mech instead.
pub fn begin_vtol(
    w: &mut World,
    commander: i32,
    vehicle_id: i32,
    pos: Vec3,
    target: Option<WatchId>,
) -> Result<(), VtolRefusal> {
    let c = commander_index(commander).ok_or(VtolRefusal::InvalidPlacement)?;
    if support_vehicle_pilot(vehicle_id).is_none() {
        return Err(VtolRefusal::UnknownVehicle);
    }
    let Some(team) = team_of(w, c) else {
        return Err(VtolRefusal::InvalidPlacement);
    };
    let target_handle = target.and_then(|t| w.objects.by_watch_id(t));
    if w.vtol.in_flight(commander) {
        return Err(VtolRefusal::InvalidPlacement);
    }
    let allowed = if vehicle_id == RECOVERY_VEHICLE_ID {
        can_recover(w, team, pos, target_handle)
    } else {
        can_add_vehicle(w, team, pos)
    };
    if !allowed {
        return Err(VtolRefusal::InvalidPlacement);
    }
    let Some(assets) = w.vtol.assets.as_mut() else {
        return Err(VtolRefusal::NotPrepared);
    };
    let recovery = vehicle_id == RECOVERY_VEHICLE_ID;
    let ty = if recovery {
        &mut assets.karnov
    } else {
        &mut assets.vtol
    };
    let mut animation = BuildingAppearance::new(ty, 0);
    // vTol->setObjectParameters(newPos, ...); update(); setGesture(0) (the Karnov starts with its landing, gesture 1)
    let mut position = pos;
    let mut mech = WatchId::NONE;
    if recovery {
        if let Some(t) = target_handle {
            position = w.objects.get(t).map_or(pos, |o| o.c.position);
            mech = w.objects.watch_id(t);
        }
    }
    animation.set_object_parameters(position, 0.0);
    {
        let streams = w.rng.0.clone();
        let mut guard = streams.borrow_mut();
        let r = &mut *guard;
        let assets = w.vtol.assets.as_mut().expect("checked above");
        let ty = if recovery {
            &mut assets.karnov
        } else {
            &mut assets.vtol
        };
        animation.set_gesture(ty, u32::from(recovery), &mut r.visual);
        update_animation(
            &mut animation,
            ty,
            w.clock.turn,
            0.0,
            &mut r.sim,
            Some(&mut r.visual),
        );
    }
    // The dust cloud starts once, where the craft is (missiongui.cpp:4282-4447), with the non-weapon effects on.
    support_fx(
        w,
        c,
        FxSlot::SupportDust(c as u8),
        if recovery {
            fx_id::KARNOV_DUST_CLOUD
        } else {
            fx_id::VTOL_DUST_CLOUD
        },
        Some(FxAction::Start {
            place: FxPlacement::plain(position),
            looping: false,
        }),
        true,
    );
    w.vtol.flights[c] = Some(VtolFlight {
        vehicle_id,
        position: pos,
        animation,
        dropped: false,
        reinforcement: WatchId::NONE,
        mech,
        recovered: false,
        time: 0.0,
        refused: false,
        flight_sound_started: false,
        beam_started: false,
    });
    let home_commander = w.teams.commanders.iter().position(|commander| {
        commander
            .as_ref()
            .is_some_and(|commander| commander.team as usize == w.teams.home)
    });
    if home_commander == Some(c) {
        let voice = match vehicle_id {
            120 => Some(crate::events::support::MINELAYER),
            182 => Some(crate::events::support::REPAIR),
            393 => Some(crate::events::support::SCOUT),
            147 => Some(crate::events::support::RECOVER),
            415 => Some(crate::events::support::ARTILLERY),
            _ => None,
        };
        if let Some(id) = voice {
            w.events.push(SimEvent::Support(id));
        }
    }
    Ok(())
}

fn update_animation(
    a: &mut BuildingAppearance,
    ty: &mut BuildingAppearanceType,
    turn: i64,
    frame_length: f32,
    sim: &mut crate::rng::SimRng,
    visual: Option<&mut crate::rng::SimRng>,
) {
    let mut ctx = UpdateCtx {
        turn,
        frame_length,
        sim_rng: sim,
        gesture_rng: visual,
        terrain: None,
    };
    a.update(ty, true, &mut ctx);
}

/// `BringInReinforcement(vehicleID, 255, commanderID, pos, true)` (missiongui.cpp:4149): a new mover of the commander.
fn bring_in_reinforcement(
    w: &mut World,
    commander: usize,
    vehicle_id: i32,
    pos: Vec3,
) -> Result<ObjectHandle, String> {
    let pilot =
        support_vehicle_pilot(vehicle_id).ok_or("BringInReinforcement: null vehicleFile")?;
    let team = team_of(w, commander).ok_or("BringInReinforcement: no such commander")?;
    let spec = MoverInitData {
        pilot_file: pilot.to_string(),
        brain: "pbrain".to_string(),
        object_number: vehicle_id,
        roster_index: 255,
        control_type: 2,
        control_data_type: 2,
        position: pos,
        rotation: 0,
        team: team as i8,
        commander: commander as i8,
        active: true,
        exists: true,
        capturable: false,
        ..MoverInitData::default()
    };
    let h = crate::mission::spawn::add_mover(w, &spec)?;
    // the helicopters start landed (`getMoveType() == MOVETYPE_AIR`, missiongui.cpp:4195)
    let air = matches!(
        w.objects.type_of(h).map(|t| &t.data),
        Some(mc2_sim_data::objtype::ObjectTypeData::BattleMech(m))
            if m.move_type == crate::combat::artillery_math::MOVETYPE_AIR
    );
    if air {
        crate::ai::mover_ops::shut_down(w, h);
    }
    Ok(h)
}

/// `MissionInterfaceManager::updateVTol` (missiongui.cpp:918) for every commander, once per frame, after the player's orders.
pub fn update(w: &mut World) {
    if w.mission.paused {
        return;
    }
    for c in 0..MAX_TEAMS {
        if w.vtol.flights[c].is_some() {
            update_one(w, c);
        }
    }
}

fn update_one(w: &mut World, c: usize) {
    let Some(mut f) = w.vtol.flights[c].take() else {
        return;
    };
    let now = w.clock.scenario_time;
    let recovery_craft = f.vehicle_id == RECOVERY_VEHICLE_ID;
    request_flight_sound(
        &mut w.events,
        &mut f.flight_sound_started,
        f.position,
        recovery_craft,
    );
    let mut done = false;
    if f.vehicle_id != RECOVERY_VEHICLE_ID {
        if !f.dropped && f.animation.current_frame >= DROP_FRAME {
            f.dropped = true;
            w.events.push(SimEvent::Sound {
                id: crate::events::sfx::VTOL_DROP,
                source: WatchId::NONE,
                at: Some(f.position),
                allow_dupes: false,
            });
            match bring_in_reinforcement(w, c, f.vehicle_id, f.position) {
                Ok(h) => {
                    f.reinforcement = w.objects.watch_id(h);
                    let part = w.objects.get(h).map(|o| o.c.part_id);
                    w.vtol.last_dropped[c] = part;
                    if let Some(part_id) = part {
                        w.events.push(SimEvent::SupportVehicleDropped {
                            commander: c as i32,
                            vehicle_id: f.vehicle_id,
                            part_id: part_id.0,
                        });
                    }
                }
                Err(e) => {
                    let _ = e;
                    w.note_frame_unavailable(
                        "support vehicle: the dropped vehicle could not be created",
                    );
                }
            }
        } else if f.animation.current_frame >= LEAVE_FRAME {
            done = true;
            // the reinforcement powers up now that the VTOL is out of the way
            if let Some(h) = w.objects.by_watch_id(f.reinforcement) {
                if let Some(slot) = crate::ai::warrior::slot_of(w, h) {
                    crate::ai::orders::order_power_up(
                        w,
                        slot,
                        true,
                        crate::command::OrderOrigin::SelfOrder,
                    );
                }
            }
        }
    } else {
        done = update_karnov(w, c, &mut f, now);
    }
    if done {
        for (slot, id, gated) in support_slots(c, f.vehicle_id) {
            support_fx(w, c, slot, id, Some(FxAction::Kill), gated);
        }
        let recovered = f.recovered;
        // a recovery craft that recovered nothing returns its cost (missiongui.cpp:1112-1121)
        if f.vehicle_id == RECOVERY_VEHICLE_ID && !recovered {
            w.mission.resource_points += RECOVERY_REFUND;
        }
        w.events.push(SimEvent::VtolFinished {
            commander: c as i32,
            vehicle_id: f.vehicle_id,
            recovered,
        });
        return;
    }
    // The dust cloud and the recovery beam follow the craft (missiongui.cpp:1129-1168).
    let at = f.animation.position;
    for (slot, id, gated) in support_slots(c, f.vehicle_id) {
        if matches!(slot, FxSlot::RecoveryBeam(_)) && !f.beam_started {
            continue;
        }
        let place = FxPlacement::plain(at);
        support_fx(
            w,
            c,
            slot,
            id,
            Some(FxAction::Move {
                place,
                looping: None,
            }),
            gated,
        );
    }
    // vTol->update(): the animation advances while the game runs
    if let Some(assets) = w.vtol.assets.as_mut() {
        let ty = if f.vehicle_id == RECOVERY_VEHICLE_ID {
            &mut assets.karnov
        } else {
            &mut assets.vtol
        };
        let streams = w.rng.0.clone();
        let mut guard = streams.borrow_mut();
        let r = &mut *guard;
        update_animation(
            &mut f.animation,
            ty,
            w.clock.turn,
            w.clock.frame_length,
            &mut r.sim,
            Some(&mut r.visual),
        );
    }
    w.vtol.flights[c] = Some(f);
}

/// `missiongui.cpp:4414,5860`: request the flight sound once when the animation starts/restores.
fn request_flight_sound(
    events: &mut Vec<SimEvent>,
    sound_started: &mut bool,
    position: Vec3,
    recovery_craft: bool,
) {
    if *sound_started {
        return;
    }
    events.push(SimEvent::Sound {
        id: if recovery_craft {
            crate::events::sfx::SALVAGE_CRAFT
        } else {
            crate::events::sfx::VTOL_ANIMATE
        },
        source: WatchId::NONE,
        at: Some(position),
        allow_dupes: false,
    });
    *sound_started = true;
}

/// The Karnov branch of `updateVTol` (missiongui.cpp:1005-1105). Returns whether the craft has flown away.
fn update_karnov(w: &mut World, c: usize, f: &mut VtolFlight, now: f32) -> bool {
    let Some(assets) = w.vtol.assets.as_mut() else {
        return true;
    };
    let ty = &mut assets.karnov;
    let in_transition = !f.animation.can_transition;
    let gesture = f.animation.current_gesture;
    let mech = w.objects.by_watch_id(f.mech);
    let mech_gone = mech.is_none_or(|m| w.objects.get(m).is_none_or(|o| o.c.is_destroyed()));
    let streams = w.rng.0.clone();
    if gesture == 1 && !in_transition {
        w.events.push(SimEvent::Sound {
            id: crate::events::sfx::RADAR_HUM,
            source: WatchId::NONE,
            at: Some(f.position),
            allow_dupes: false,
        });
        // landed: hover (and hold the mech); "mechRecovered = false" stays false
        let mut guard = streams.borrow_mut();
        let r = &mut *guard;
        f.animation.set_gesture(ty, 0, &mut r.visual);
        f.recovered = false;
        f.beam_started = true;
        f.time = now;
        // The Karnov hovers now: the recovery beam starts, once (missiongui.cpp:1013-1033).
        let place = FxPlacement::plain(f.animation.position);
        support_fx(
            w,
            c,
            FxSlot::RecoveryBeam(c as u8),
            fx_id::KARNOV_RECOVERY_BEAM,
            Some(FxAction::Start {
                place,
                looping: false,
            }),
            false,
        );
    } else if gesture == 0 && !f.recovered && !mech_gone && !f.refused {
        let recover_time = w
            .data
            .gamesys
            .tables
            .as_ref()
            .map_or(0.0, |t| t.mover_general.recover_time);
        if now > f.time + recover_time {
            if let Some(m) = mech {
                f.recovered = recover_mech(w, m);
                f.refused = !f.recovered;
            }
        }
    } else if gesture == 0 && (f.recovered || mech_gone || f.refused) && !in_transition {
        let mut guard = streams.borrow_mut();
        let r = &mut *guard;
        f.animation.set_gesture(ty, 2, &mut r.visual);
    } else if gesture == 2 && !in_transition {
        return true;
    }
    false
}

/// The recovery itself (missiongui.cpp:1040-1075): `recover()` until done, the mech shut down, then traded to the home team with the
/// campaign's best pilot, and ordered to power up. A refused trade (no pilot) leaves the mech as it was and is reported unavailable.
fn recover_mech(w: &mut World, mech: ObjectHandle) -> bool {
    let tonnage = w.objects.get(mech).map_or(0.0, |o| o.c.tonnage);
    // the pilot first: `LogisticsData::getBestPilot` always has one in the original (the button is disabled otherwise)
    let Some(chooser) = w.recovery_pilots.clone() else {
        w.note_frame_unavailable("recovery: the campaign has no pilot for the recovered mech");
        return false;
    };
    let pilot = {
        let world: &World = w;
        chooser.best_pilot(tonnage as i64, &|file| {
            crate::mission::trade::warrior_in_use(world, file)
        })
    };
    let Some(pilot) = pilot else {
        w.note_frame_unavailable("recovery: the campaign has no pilot for the recovered mech");
        return false;
    };
    for _ in 0..1000 {
        if crate::mover::refit::recover(w, mech) {
            break;
        }
    }
    let disabled = w.objects.get(mech).is_some_and(|o| o.c.is_disabled());
    if disabled {
        if let Some(o) = w.objects.get_mut(mech) {
            o.c.set_status(crate::object::ObjectStatus::ShutDown, true);
            // `timeLeft = 1.0; exploding = false` (the sensors are repaired by the trade)
            if let Kind::Mech(m) = &mut o.kind {
                m.m.time_left = 1.0;
                m.m.flags.exploding = false;
            }
        }
    }
    let (home_team, home_commander) = (w.teams.home as i32, crate::combat::HOME_COMMANDER);
    if crate::mission::trade::trade_mover(w, mech, home_team, home_commander, Some(&pilot)).is_err()
    {
        w.note_frame_unavailable("recovery: the recovered mech could not be given a pilot");
        return false;
    }
    if let Some(slot) = crate::ai::warrior::slot_of(w, mech) {
        crate::ai::orders::order_power_up(w, slot, true, crate::command::OrderOrigin::SelfOrder);
    }
    true
}

/// The effects of a support craft: the dust cloud (with the non-weapon effects on) and, for the Karnov, the recovery beam.
fn support_slots(c: usize, vehicle_id: i32) -> Vec<(FxSlot, i32, bool)> {
    if vehicle_id == RECOVERY_VEHICLE_ID {
        vec![
            (FxSlot::SupportDust(c as u8), fx_id::KARNOV_DUST_CLOUD, true),
            (
                FxSlot::RecoveryBeam(c as u8),
                fx_id::KARNOV_RECOVERY_BEAM,
                false,
            ),
        ]
    } else {
        vec![(FxSlot::SupportDust(c as u8), fx_id::VTOL_DUST_CLOUD, true)]
    }
}

/// Pushes one effect call of commander `c`'s support craft. `gated` effects need `UseNonWeaponEffects`.
fn support_fx(
    w: &mut World,
    _c: usize,
    slot: FxSlot,
    id: i32,
    action: Option<FxAction>,
    gated: bool,
) {
    if gated && !w.objects.use_non_weapon_effects {
        return;
    }
    if let Some(action) = action {
        w.events.push(SimEvent::AppearanceFx {
            object: crate::ids::ObjectHandle::NONE,
            slot,
            effect: FxEffect::Id(id),
            action,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvage_flight_sound_is_requested_once_across_the_full_animation() {
        let mut events = Vec::new();
        let mut sound_started = false;
        let position = Vec3 {
            x: 10.0,
            y: 20.0,
            z: 30.0,
        };
        for _ in 0..600 {
            request_flight_sound(&mut events, &mut sound_started, position, true);
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    SimEvent::Sound { id, at: Some(at), allow_dupes: false, .. }
                        if *id == crate::events::sfx::SALVAGE_CRAFT && *at == position
                ))
                .count(),
            1
        );
    }
}

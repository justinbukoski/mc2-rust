// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Support-vehicle mine behaviour: the mine under a mover, mine sweepers, mine layers.
//!
//! `GroundVehicle::mineCheck` (gvehicl.cpp:1366), the mine laying block of `GroundVehicle::update` (gvehicl.cpp:3637-3668),
//! the sweeper/layer throttle (gvehicl.cpp:2069-2072) and `BattleMech::mineCheck` (mech.cpp:3760).

use crate::combat::{AttackSource, WeaponShotInfo};
use crate::ids::{ObjectHandle, WatchId};
use crate::object::carnage::MINE_EXPLOSION_ID;
use crate::object::{terr, Kind, ObjectStatus, ObjectStore};
use crate::World;

/// `GROUNDVEHICLE_LOCATION_FRONT` / `_TURRET`.
const GV_FRONT: usize = 0;
const GV_TURRET: i32 = 4;

/// The sweeping minesweeper / laying minelayer throttle cap (gvehicl.cpp:2069-2072): `Some(MineSweepThrottle)` while a sweeper's
/// `sweepTime` is inside `(0, gvSweepTime)`, then `Some(MineLayThrottle)` for a layer with mines left on a mine-laying order
/// (the layer wins when both apply).
pub fn mine_throttle(w: &World, h: ObjectHandle, slot: u16) -> Option<i32> {
    let tables = w.data.gamesys.tables.as_ref()?;
    let Some(Kind::Vehicle(v)) = w.objects.get(h).map(|o| &o.kind) else {
        return None;
    };
    let mut cap = None;
    if v.loadout.stats.mine_sweeper
        && v.sweep_time > 0.0
        && v.sweep_time < tables.vehicle.sweep_time
    {
        cap = Some(tables.mine.mine_sweep_throttle);
    }
    let laying = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.move_params.mine_laying);
    if v.loadout.stats.mine_layer != 0 && laying {
        cap = Some(tables.mine.mine_lay_throttle);
    }
    cap
}

/// `GroundVehicle::mineCheck()` (gvehicl.cpp:1366): a live mine in the vehicle's cell. A sweeper sets it off at the price of one
/// armour point of its front (and stops sweeping when that is gone), a mine layer is immune, any other vehicle over 35 tons
/// takes the mine's splash and hit.
pub fn vehicle_mine_check(w: &mut World, h: ObjectHandle, slot: u16) {
    let Some(obj) = w.objects.get(h) else { return };
    let (row, col) = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
    let (position, tonnage) = (obj.c.position, obj.c.tonnage);
    let Kind::Vehicle(v) = &obj.kind else { return };
    let (sweeper, layer) = (v.loadout.stats.mine_sweeper, v.loadout.stats.mine_layer);
    if !w.terrain.in_bounds(row, col) || terr::get_mine(&w.terrain, row, col) != 1 {
        return;
    }
    if sweeper {
        ObjectStore::create_explosion(w, MINE_EXPLOSION_ID, None, position, 0.0, 0.0);
        let mut stop = false;
        if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
            v.sweep_time = 0.0;
            let front = &mut v.loadout.armor[GV_FRONT].0;
            *front -= 1.0;
            // C++ tests `== 0`, which a fractional or already empty front never reaches; the sweeper is spent at `<= 0`.
            if *front <= 0.0 {
                v.loadout.stats.mine_sweeper = false;
                v.sweep_time = -1.0;
                stop = true;
            }
        }
        if stop {
            crate::ai::warrior::clear_cur_tac_order(w, slot, true);
        }
        // Mark this tile as empty (we just set the mine off).
        terr::set_mine(&mut w.terrain, row, col, 2);
    } else if layer == 0 && tonnage > 35.0 {
        mine_blast(w, h, position);
        crate::ai::warrior::pause_path(w, slot);
        terr::set_mine(&mut w.terrain, row, col, 2);
    }
}

/// `BattleMech::mineCheck()` (mech.cpp:3760): a mine under a grounded mech over 35 tons goes off; the mech loses its order.
pub fn mech_mine_check(w: &mut World, h: ObjectHandle, slot: u16) {
    let Some(obj) = w.objects.get(h) else { return };
    let Kind::Mech(m) = &obj.kind else { return };
    // Can't set 'em off while flying, and helicopters don't count.
    if m.in_jump || (m.m.move_level == 2 && obj.c.status != ObjectStatus::ShutDown) {
        return;
    }
    let (row, col) = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
    let (position, tonnage) = (obj.c.position, obj.c.tonnage);
    if !w.terrain.in_bounds(row, col)
        || terr::get_mine(&w.terrain, row, col) != 1
        || tonnage <= 35.0
    {
        return;
    }
    terr::set_mine(&mut w.terrain, row, col, 2);
    crate::ai::warrior::clear_cur_tac_order(w, slot, true);
    mine_blast(w, h, position);
}

/// The explosion and the hit of a mine under `h` (`MineSplashDamage`, `MineSplashRange`, `MineDamage`).
fn mine_blast(w: &mut World, h: ObjectHandle, position: crate::math::Vec3) {
    let (splash, range) = crate::object::hooks::mine_splash(w);
    ObjectStore::create_explosion(
        w,
        MINE_EXPLOSION_ID,
        None,
        position,
        splash,
        range * crate::terrain::WORLD_UNITS_PER_METER,
    );
    let damage = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or(0.0, |t| t.mine.mine_damage);
    let location = crate::object::hooks::calc_hit_location(
        w,
        h,
        ObjectHandle::NONE,
        -1,
        AttackSource::Mine,
        0,
    );
    let shot = WeaponShotInfo {
        attacker: WatchId::NONE,
        master_id: -2,
        damage,
        hit_location: location,
        entry_angle: 0.0,
    };
    crate::object::hooks::weapon_hit(w, h, &shot);
}

/// The mine laying block of `GroundVehicle::update` (gvehicl.cpp:3637-3668): a layer on a mine-laying order that has left the cell it
/// mined last drops a mine after `MineWaitTime` seconds in the new cell and spends one mine (a `-2` hit on its turret).
pub fn lay_mine(w: &mut World, h: ObjectHandle, slot: u16) {
    let Some(tables) = w.data.gamesys.tables.as_ref() else {
        return;
    };
    let wait = tables.mine.mine_wait_time;
    let frame = w.clock.frame_length;
    let Some(obj) = w.objects.get(h) else { return };
    let cell = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
    let Kind::Vehicle(v) = &obj.kind else { return };
    let laying = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.move_params.mine_laying);
    if v.loadout.stats.mine_layer == 0 || !laying || cell == v.cell_to_mine {
        return;
    }
    let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) else {
        return;
    };
    v.time_in_current += frame;
    if v.time_in_current <= wait {
        return;
    }
    v.cell_to_mine = cell;
    v.time_in_current = 0.0;
    // Always 1, and it always replaces the current mine data. Everyone can see mines.
    terr::set_mine(&mut w.terrain, cell.0, cell.1, 1);
    let shot = WeaponShotInfo {
        attacker: WatchId::NONE,
        master_id: -2,
        damage: 1.0,
        hit_location: GV_TURRET,
        entry_angle: 0.0,
    };
    crate::object::hooks::weapon_hit(w, h, &shot);
}

// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `WeaponShotInfo` (gameobj.h:86, gameobj.cpp:139-196) and the difficulty scaling it applies (`applyDifficultyWeapon`,
//! `applyDifficultySkill`, mission.cpp:826-905).
//!
//! `init` scales the damage by the difficulty tables when the attacker is known (single player: `MPlayer == NULL`), and quantises
//! it to a multiple of 0.25 in `[0, 255]`. `setDamage` is a plain assignment (the 0.25 rounding there is multiplayer only).
//! `operator=` is `init(copy...)`, so assigning a shot scales it again; copy construction does not. The damage code relies on
//! both: it copy-constructs the shot for a transfer, and its closing `*shotInfo = startShotInfo` re-runs `init` on the caller's shot.

use crate::combat::{WeaponShotInfo, HOME_COMMANDER};
use crate::ids::WatchId;
use crate::object::ObjectClass;
use crate::World;

/// `applyDifficultyWeapon(dmg, isPlayer)` (mission.cpp:857) with the tables of `[DifficultySettings]`.
pub fn apply_difficulty_weapon(w: &World, dmg: f32, is_player: bool) -> f32 {
    let mut dmg = dmg;
    if let Some(t) = w.data.gamesys.tables.as_ref() {
        let d = w.mission.difficulty;
        if (0..4).contains(&d) {
            let table = if is_player {
                &t.difficulty.player_weapons
            } else {
                &t.difficulty.enemy_weapons
            };
            // `dmg *= (float(table[d]) / 100.0)`: the factor is a double, the product narrows back
            dmg = (f64::from(dmg) * (f64::from(table[d as usize] as f32) / 100.0)) as f32;
        }
    }

    quantize_damage(dmg)
}

/// The tail of `applyDifficultyWeapon`: damage must stay an integral multiple of 0.25 (or bad things happen, in theory only in
/// multiplayer), within [0, 255]. `(float)((long)(dmg / 0.25)) * 0.25` is a double expression.
pub fn quantize_damage(dmg: f32) -> f32 {
    let mut dmg = dmg;
    let quarters = f64::from((f64::from(dmg) / 0.25) as i64 as f32) * 0.25;
    if quarters != f64::from(dmg) {
        dmg = quarters as f32;
    }
    dmg.clamp(0.0, 255.0)
}

/// `applyDifficultySkill(chance, isPlayer)` (mission.cpp:826).
pub fn apply_difficulty_skill(w: &World, chance: f32, is_player: bool) -> f32 {
    let d = w.mission.difficulty;
    match w.data.gamesys.tables.as_ref() {
        Some(t) if (0..4).contains(&d) => {
            let table = if is_player {
                &t.difficulty.player_skills
            } else {
                &t.difficulty.enemy_skills
            };
            (f64::from(chance) * (f64::from(table[d as usize] as f32) / 100.0)) as f32
        }
        _ => chance,
    }
}

impl WeaponShotInfo {
    /// `WeaponShotInfo::init(attackerWID, masterId, damage, hitLocation, entryAngle)` (gameobj.cpp:139).
    pub fn init(
        w: &World,
        attacker: WatchId,
        master_id: i32,
        damage: f32,
        hit_location: i32,
        entry_angle: f32,
    ) -> WeaponShotInfo {
        let mut s = WeaponShotInfo {
            attacker,
            master_id,
            damage,
            hit_location,
            entry_angle,
        };
        // No multiplayer skill levels
        if !attacker.is_none() {
            // Skill levels need the attacker's alignment and class
            if let Some(a) = w.objects.get_by_watch_id(attacker) {
                let is_player = a.commander_id() == HOME_COMMANDER;
                if !is_player {
                    // Only enemy mechs, vehicles, elementals and turrets get modified
                    if matches!(
                        a.c.class,
                        ObjectClass::BattleMech | ObjectClass::GroundVehicle | ObjectClass::Turret
                    ) {
                        s.damage = apply_difficulty_weapon(w, damage, false);
                    }
                } else {
                    s.damage = apply_difficulty_weapon(w, damage, true);
                }
            }
        }
        s
    }

    /// `WeaponShotInfo::setDamage` (gameobj.cpp:189): a plain assignment outside multiplayer.
    pub fn set_damage(&mut self, damage: f32) {
        self.damage = damage;
    }

    /// `WeaponShotInfo::setEntryAngle` (gameobj.cpp:198): a plain assignment outside multiplayer.
    pub fn set_entry_angle(&mut self, entry_angle: f32) {
        self.entry_angle = entry_angle;
    }

    /// `operator = (_WeaponShotInfo copy)` (gameobj.h:99): `init(copy.attackerWID, ...)`, i.e. the damage is scaled again.
    pub fn assign(&mut self, w: &World, copy: WeaponShotInfo) {
        *self = WeaponShotInfo::init(
            w,
            copy.attacker,
            copy.master_id,
            copy.damage,
            copy.hit_location,
            copy.entry_angle,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_is_cut_down_to_quarters_and_clamped() {
        assert_eq!(quantize_damage(0.1), 0.0);
        assert_eq!(quantize_damage(0.7), 0.5);
        assert_eq!(quantize_damage(0.75), 0.75);
        assert_eq!(quantize_damage(3.3), 3.25);
        assert_eq!(quantize_damage(7.77), 7.75);
        assert_eq!(quantize_damage(254.99), 254.75);
        assert_eq!(quantize_damage(255.0), 255.0);
        assert_eq!(quantize_damage(300.0), 255.0);
        assert_eq!(quantize_damage(-1.0), 0.0);
        // truncation is toward zero: -0.1 is cut to -0.0, which is not < 0 and stays a (negative) zero
        assert_eq!(quantize_damage(-0.1), 0.0);
    }
}

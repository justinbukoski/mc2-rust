// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Turret fire control: `code/turret.cpp`.

/// Result of [`turret_attack_chance`]: the attack chance plus the general fire range class
/// (`FIRERANGE_SHORT` = 0, `FIRERANGE_MEDIUM` = 1, `FIRERANGE_LONG` = 2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurretChance {
    pub chance: f32,
    pub range_class: Option<i32>,
}

/// Specialist flags and tonnage of the target mech's pilot, as read by `Turret::calcAttackChance`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MechSpec {
    pub tonnage: f32,
    pub light: bool,
    pub medium: bool,
    pub heavy: bool,
    pub assault: bool,
}

/// Port of `Turret::calcAttackChance` (`code/turret.cpp:997-1087`).
///
/// `target_speed_sq` is `Some(vx*vx + vy*vy)` when the target exists, `None` otherwise. `mech` is `Some` only when the
/// target is a mech with a pilot.
#[allow(clippy::too_many_arguments, clippy::if_same_then_else)] // the C++ keeps the branches apart
pub fn turret_attack_chance(
    weapon_master_id: i32,
    pilot_skill: f32,
    distance: f32,
    short_range: f32,
    medium_range: f32,
    weapon_min: f32,
    weapon_max: f32,
    target_speed_sq: Option<f32>,
    mech: Option<MechSpec>,
) -> TurretChance {
    if weapon_master_id == -1 {
        return TurretChance {
            chance: 0.0,
            range_class: None,
        };
    }

    let mut attack_chance = pilot_skill;

    // General fire range.
    let range_class = if distance <= short_range {
        0
    } else if distance <= medium_range {
        1
    } else {
        2
    };
    let range_class = Some(range_class);

    // Range (per weapon).
    if distance <= weapon_min {
        return TurretChance {
            chance: -1.0,
            range_class,
        };
    } else if distance > weapon_max {
        return TurretChance {
            chance: -1.0,
            range_class,
        };
    }

    // Target movement.
    if let Some(speed_sq) = target_speed_sq {
        if speed_sq == 0.0 {
            attack_chance += 50.0;
        }
    }

    // Mech specialist modifiers.
    if let Some(mech) = mech {
        if mech.tonnage < 40.0 && mech.light {
            attack_chance -= 30.0;
        } else if mech.tonnage >= 40.0 && mech.tonnage < 60.0 && mech.medium {
            attack_chance -= 20.0;
        } else if mech.tonnage >= 60.0 && mech.tonnage < 80.0 && mech.heavy {
            attack_chance -= 10.0;
        } else if mech.tonnage >= 80.0 && mech.tonnage < 100.0 && mech.assault {
            attack_chance -= 10.0;
        }

        if attack_chance < 5.0 {
            attack_chance = 5.0;
        }
    }

    TurretChance {
        chance: attack_chance,
        range_class,
    }
}

/// Outcome of the turret's line-of-sight decision (`Turret::fireWeapon`, turret.cpp:1376-1391).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LosOutcome {
    /// The turret can see the target (direct line of sight, or the team fallback for indirect fire).
    Clear,
    /// No direct line of sight, and neither the weapon's team line of sight (indirect fire only) sees the target.
    Blocked,
}

/// Port of the line-of-sight decision in `Turret::fireWeapon` (turret.cpp:1386-1391): `LOS = direct`; if `!LOS && indirect`,
/// `LOS = getTeam()->teamLineOfSight(target->getPosition(), target->getAppearRadius())`, the merged
/// [`team_line_of_sight`](crate::object::hooks::team_line_of_sight) (`Team::teamLineOfSight`, team.cpp:762).
pub fn resolve_los(
    w: &mut crate::World,
    turret: crate::ids::ObjectHandle,
    target: crate::ids::ObjectHandle,
    direct_los: bool,
    indirect: bool,
) -> LosOutcome {
    if direct_los {
        return LosOutcome::Clear;
    }
    if indirect {
        let team = w.objects.get(turret).map_or(-1, |o| o.team_id());
        let pos = w.objects.get(target).map_or_else(
            || crate::math::Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            |o| o.c.position,
        );
        let ext = crate::object::hooks::appear_radius(w, target);
        if crate::object::hooks::team_line_of_sight(w, team, pos, ext) {
            return LosOutcome::Clear;
        }
    }
    LosOutcome::Blocked
}

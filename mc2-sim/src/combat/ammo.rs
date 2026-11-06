// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ammo accounting: `Mover::reduceAmmo` / ammo totals (the ammo half of `code/mover.cpp`).

use crate::combat::Loadout;

/// What [`reduce_ammo`] reports back: `used` is the `amountUsed` the C++ returns; `spent` is true when the matching
/// `ammoTypeTotal` fell to zero or below and was clamped (the caller then runs `calcWeaponEffectiveness(false)`,
/// `calcFireRanges()` and `RADIO_AMMO_OUT`, which are ported elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReduceResult {
    /// `amountUsed`: the original `amount`.
    pub used: i32,
    /// The ammo type ran out (`curAmount <= 0` after the deduction).
    pub spent: bool,
}

/// `Mover::reduceAmmo` (mover.cpp:6054-6090) without its tail side effects.
pub fn reduce_ammo(l: &mut Loadout, ammo_master_id: i32, amount: i32) -> ReduceResult {
    let amount_used = amount;
    let mut amount = amount;

    // Deduct it from the first available ammo critical space...
    let first_ammo = l.num_other() + l.num_weapons();
    let end_ammo = first_ammo + l.num_ammos();
    for item in &mut l.inventory[first_ammo..end_ammo] {
        if i32::from(item.master_id) == ammo_master_id {
            if i32::from(item.amount) > amount {
                item.amount = (i32::from(item.amount) - amount) as i16;
                break;
            } else {
                amount -= i32::from(item.amount);
                item.amount = 0;
            }
        }
    }

    // Make sure the ammoTypeTotal list is adjusted...
    let mut spent = false;
    if let Some(total) = l.ammo_totals.iter_mut().find(|t| t.0 == ammo_master_id) {
        total.1 -= amount_used;
        if total.1 <= 0 {
            total.1 = 0;
            spent = true;
        }
    }

    ReduceResult {
        used: amount_used,
        spent,
    }
}

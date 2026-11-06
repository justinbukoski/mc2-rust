//! Ammo accounting: `Mover::reduceAmmo` walks the ammo slots and the ammo type totals.

use mc2_sim::combat::ammo::{reduce_ammo, ReduceResult};
use mc2_sim::combat::{InventoryItem, Loadout};

fn item(master_id: u8, amount: i16) -> InventoryItem {
    InventoryItem {
        master_id,
        amount,
        ..Default::default()
    }
}

#[test]
fn reduce_ammo_walks_the_ammo_slots() {
    let mut l = Loadout {
        counts: (1, 0, 3),
        inventory: vec![item(5, 0), item(7, 10), item(7, 5), item(9, 4)],
        ammo_totals: vec![(7, 15, 15), (9, 4, 4)],
        ..Default::default()
    };

    // 10 from slot 1 (not enough), then 2 from slot 2 -> slot 2 keeps 3.
    assert_eq!(
        reduce_ammo(&mut l, 7, 12),
        ReduceResult {
            used: 12,
            spent: false
        }
    );
    assert_eq!(l.inventory[1].amount, 0);
    assert_eq!(l.inventory[2].amount, 3);
    assert_eq!(l.ammo_totals[0].1, 3);

    // Slot 1 is empty; slot 2 covers exactly 3 and is emptied (not greater, so it continues).
    assert_eq!(
        reduce_ammo(&mut l, 7, 3),
        ReduceResult {
            used: 3,
            spent: true
        }
    );
    assert_eq!(l.inventory[1].amount, 0);
    assert_eq!(l.inventory[2].amount, 0);
    assert_eq!(l.inventory[3].amount, 4, "other master id untouched");
    assert_eq!(l.ammo_totals[0].1, 0);

    // Slot 3 (master 9) cannot cover 10; the total goes negative and clamps to 0.
    assert_eq!(
        reduce_ammo(&mut l, 9, 10),
        ReduceResult {
            used: 10,
            spent: true
        }
    );
    assert_eq!(l.inventory[3].amount, 0);
    assert_eq!(l.ammo_totals[1].1, 0);

    // No such ammo: nothing changes.
    assert_eq!(
        reduce_ammo(&mut l, 8, 1),
        ReduceResult {
            used: 1,
            spent: false
        }
    );
    assert_eq!(l.ammo_totals, vec![(7, 0, 15), (9, 0, 4)]);
}

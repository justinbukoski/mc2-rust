// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The order of the icons on the bar: `ForceGroupIcon::sort` (`code/mechicon.cpp:1383-1428`).

use super::Icon;
use std::cmp::Ordering;

/// `ForceGroupIcon::sort` as the comparator sees it. Force groups 1 to 9 come first, lowest number
/// first; then mechs before vehicles; then lighter before heavier; then by pilot name.
///
/// Two details are the C++'s own. The group loop stops at the first group both units are in
/// (`mechicon.cpp:1401`), so later groups do not count. Group 0 is never tested, because the C++
/// maps `i == 10` to group 0 and `i` never reaches 10 (`mechicon.cpp:1392-1394`). A complete tie
/// returns `m1 < m2` on the pointers, so the later allocation sorts first when addresses rise with
/// allocation order; here the allocation sequence stands in for the address.
pub fn compare(a: &Icon, b: &Icon) -> Ordering {
    let (g1, g2) = (a.unit.groups, b.unit.groups);
    for i in 1..10 {
        let m1 = (g1 >> i) & 1 == 1;
        let m2 = (g2 >> i) & 1 == 1;
        if m1 && !m2 {
            return Ordering::Less;
        }
        if m2 && !m1 {
            return Ordering::Greater;
        }
        if m1 && m2 {
            break;
        }
    }
    let (v1, v2) = (a.is_vehicle(), b.is_vehicle());
    if v2 && !v1 {
        return Ordering::Less;
    }
    if v1 && !v2 {
        return Ordering::Greater;
    }
    match a.unit.tonnage.cmp(&b.unit.tonnage) {
        Ordering::Equal => {}
        o => return o,
    }
    match a
        .unit
        .pilot
        .name
        .as_bytes()
        .cmp(b.unit.pilot.name.as_bytes())
    {
        Ordering::Equal => {}
        o => return o,
    }
    if a.seq < b.seq {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// Sorts the icons with [`compare`]. An insertion sort: at most sixteen icons, and it stays
/// well-defined even where the group rule is not transitive (`slice::sort_by` may panic on such
/// a comparator).
pub fn sort(icons: &mut [Icon]) {
    for i in 1..icons.len() {
        let mut j = i;
        while j > 0 && compare(&icons[j - 1], &icons[j]) == Ordering::Greater {
            icons.swap(j - 1, j);
            j -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Armor, Icon, Loc, MechArmor, Pilot, UnitBarUnit, VehicleArmor};
    use super::*;
    use crate::textures::TexId;

    fn icon(name: &str, groups: u16, tonnage: u32, vehicle: bool, seq: u64) -> Icon {
        let loc = Loc {
            cur: 1.0,
            max: 1.0,
            destroyed: false,
        };
        let armor = if vehicle {
            Armor::Vehicle(VehicleArmor {
                front: loc,
                rear: loc,
                left: loc,
                right: loc,
                turret: loc,
            })
        } else {
            Armor::Mech(MechArmor {
                head: loc,
                ctorso: loc,
                rctorso: loc,
                ltorso: loc,
                rltorso: loc,
                rtorso: loc,
                rrtorso: loc,
                larm: loc,
                rarm: loc,
                lleg: loc,
                rleg: loc,
            })
        };
        Icon {
            unit: UnitBarUnit {
                id: seq as u32,
                icon_picture: 0,
                tonnage,
                pilot: Pilot {
                    name: name.to_string(),
                    photo: 0,
                    message_playing: false,
                },
                armor,
                bar_status: 1.0,
                selected: false,
                groups,
                shutdown: false,
                destroyed: false,
                recover_pending: false,
                power_used_up: false,
                can_jump: false,
            },
            seq,
            image: TexId(0),
            image_slot: 0,
            damage: [0; 8],
            slot: 0,
            death_time: 0.0,
            death_reported: false,
            msg_time: 0.0,
            hidden: false,
        }
    }

    #[test]
    fn lower_force_group_comes_first() {
        // group 1 (bit 1) before group 2 (bit 2), mechicon.cpp:1395-1400
        let g1 = icon("Z", 1 << 2, 90, false, 1);
        let g2 = icon("A", 1 << 1, 20, false, 2);
        assert_eq!(compare(&g1, &g2), Ordering::Greater);
        assert_eq!(compare(&g2, &g1), Ordering::Less);
    }

    #[test]
    fn a_grouped_unit_comes_before_an_ungrouped_one() {
        let grouped = icon("Z", 1 << 4, 90, false, 1);
        let loose = icon("A", 0, 20, false, 2);
        assert_eq!(compare(&grouped, &loose), Ordering::Less);
    }

    #[test]
    fn the_group_loop_stops_where_both_share_a_group() {
        // both in group 1: the loop breaks, so group 2 is not looked at (mechicon.cpp:1401-1404)
        // and the tonnage decides: 40 before 60
        let a = icon("A", (1 << 1) | (1 << 2), 60, false, 1);
        let b = icon("B", 1 << 1, 40, false, 2);
        assert_eq!(compare(&a, &b), Ordering::Greater);
    }

    #[test]
    fn mechs_come_before_vehicles_then_lighter_then_name() {
        // mechicon.cpp:1407-1418
        let mech = icon("Z", 0, 90, false, 1);
        let vehicle = icon("A", 0, 20, true, 2);
        assert_eq!(compare(&mech, &vehicle), Ordering::Less);
        let light = icon("Z", 0, 20, false, 3);
        let heavy = icon("A", 0, 90, false, 4);
        assert_eq!(compare(&light, &heavy), Ordering::Less);
        let al = icon("ALPHA", 0, 50, false, 5);
        let be = icon("BETA", 0, 50, false, 6);
        assert_eq!(compare(&al, &be), Ordering::Less);
    }

    #[test]
    fn a_full_tie_puts_the_later_allocation_first() {
        // mechicon.cpp:1424-1425 `return m1 < m2`: the C++ int is 1 when the first pointer is the
        // lower one, and 0 otherwise. So the earlier allocation compares Greater (sorts after).
        let first = icon("SAME", 0, 50, false, 1);
        let second = icon("SAME", 0, 50, false, 2);
        assert_eq!(compare(&first, &second), Ordering::Greater);
        assert_eq!(compare(&second, &first), Ordering::Equal);
    }

    #[test]
    fn sort_orders_a_lance() {
        let mut icons = vec![
            icon("C", 0, 50, false, 1),
            icon("A", 1 << 1, 90, false, 2),
            icon("B", 0, 20, true, 3),
            icon("D", 0, 20, false, 4),
        ];
        sort(&mut icons);
        let names: Vec<&str> = icons.iter().map(|ic| ic.unit.pilot.name.as_str()).collect();
        assert_eq!(names, ["A", "D", "C", "B"]);
    }
}

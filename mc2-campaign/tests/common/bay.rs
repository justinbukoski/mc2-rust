//! Script commands shared by the bay and results oracle tests: what the oracle driver
//! (`oracle/logistics_main.cpp`) does for a command and prints, applied to the Rust port.
#![allow(dead_code)]

use std::fmt::Write as _;

use mc2_campaign::interface::{ObjectiveReport, ObjectiveStatus, Outcome};
use mc2_campaign::Logistics;

pub fn code<T>(r: Result<T, mc2_campaign::LogisticsError>) -> i32 {
    r.map_or_else(|e| e.code(), |_| 0)
}

pub fn dump_bay(l: &Logistics) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "  bay cbills {} drop {}/{} mission {}",
        l.c_bills(),
        l.current_drop_weight(),
        l.max_drop_weight(),
        l.current_mission()
    )
    .unwrap();
    write!(s, "  inv").unwrap();
    for m in l.inventory() {
        let pilot = m.pilot.map_or("-", |p| l.pilots()[p].file_name.as_str());
        write!(
            s,
            " [{:08x} {} fg{} {}]",
            m.id as u32,
            l.mech_name(m.key).unwrap(),
            m.force_group,
            pilot
        )
        .unwrap();
    }
    write!(s, "\n  vars").unwrap();
    let mut total = 0;
    for (_, v) in l.variants() {
        total += 1;
        if !v.designer {
            write!(
                s,
                " [{:08x} {} a{} n{}]",
                v.id,
                v.name,
                i32::from(v.available_to_user),
                v.components.len()
            )
            .unwrap();
        }
    }
    writeln!(s, " total {total}").unwrap();
    write!(s, "  pilots-used").unwrap();
    for p in l.pilots().iter().filter(|p| p.used) {
        write!(s, " {}", p.file_name).unwrap();
    }
    writeln!(s).unwrap();
    s
}

pub fn mech_at(l: &Logistics, i: usize) -> Option<mc2_campaign::MechKey> {
    l.inventory().get(i).map(|m| m.key)
}

/// Applies one script line to the Rust port and returns what the oracle prints for it.
pub fn apply(l: &mut Logistics, line: &str) -> String {
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let nums: Vec<i64> = rest
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    let mut out = String::new();
    match cmd {
        "quiet" => return String::new(),
        "campaign" => {
            l.start_new_campaign(rest).unwrap();
            return String::new();
        }
        "win" => {
            let cbills: i32 = nums[0] as i32;
            let outcome = Outcome {
                objectives: vec![ObjectiveReport {
                    status: ObjectiveStatus::Successful,
                    resource_points: cbills,
                }],
                ..Outcome::won()
            };
            l.apply_outcome(&outcome).unwrap();
            if !l.campaign_over() {
                let current = l.current_mission().to_string();
                let _ = l.set_current_mission(&current);
            }
            return String::new();
        }
        "choose" => {
            let r = code(l.set_current_mission(rest));
            writeln!(out, "CHOOSE {rest} => {r}").unwrap();
        }
        "give" => {
            l.add_c_bills(nums[0]);
            writeln!(out, "GIVE {}", nums[0]).unwrap();
        }
        "buy" => {
            let r = match l.variant_by_name(rest) {
                Some(v) => code(l.purchase_mech(v)),
                None => -1,
            };
            writeln!(out, "BUY {rest} => {r}").unwrap();
        }
        "salvage" => {
            if let Some(v) = l.variant_by_name(rest) {
                l.add_mech_to_inventory_with_pilot(v, None, 0, false);
            }
            writeln!(out, "SALVAGE {rest}").unwrap();
        }
        "sell" => {
            let r = mech_at(l, nums[0] as usize).map_or(-1, |m| code(l.sell_mech(m)));
            writeln!(out, "SELL {} => {r}", nums[0]).unwrap();
        }
        "fg" => {
            let r = mech_at(l, nums[0] as usize)
                .map_or(-1, |m| code(l.add_mech_to_force_group(m, nums[1] as i32)));
            writeln!(out, "FG {} {} => {r}", nums[0], nums[1]).unwrap();
        }
        "unfg" => {
            let r = mech_at(l, nums[0] as usize).map_or(-1, |m| {
                code(l.remove_mech_from_force_group(m, nums[1] != 0))
            });
            writeln!(out, "UNFG {} {} => {r}", nums[0], nums[1]).unwrap();
        }
        "unslot" => {
            let r = code(l.remove_slot_from_force_group(nums[0] as i32));
            writeln!(out, "UNSLOT {} => {r}", nums[0]).unwrap();
        }
        "setpilot" => {
            let mut it = rest.split_whitespace();
            let i: usize = it.next().unwrap().parse().unwrap();
            let file = it.next().unwrap();
            let pilot = (file != "none")
                .then(|| l.pilots().iter().position(|p| p.file_name == file).unwrap());
            l.set_mech_pilot(mech_at(l, i).unwrap(), pilot);
            writeln!(out, "SETPILOT {i} {file}").unwrap();
        }
        "edit" => {
            let r = mech_at(l, nums[0] as usize).map_or(-1, |m| code(l.set_mech_to_modify(m)));
            writeln!(out, "EDIT {} => {r}", nums[0]).unwrap();
        }
        "labadd" => {
            let (id, ox, oy) = (nums[0], nums[1], nums[2]);
            let (mut x, mut y) = (ox, oy);
            let r =
                code(l.lab_add_component(l.component_index(id as i32).unwrap(), &mut x, &mut y));
            writeln!(out, "LABADD {id} {ox} {oy} => {r} {x} {y}").unwrap();
        }
        "labremove" => {
            let r = code(l.lab_remove_component(nums[0], nums[1]));
            writeln!(out, "LABREMOVE {} {} => {r}", nums[0], nums[1]).unwrap();
        }
        "labcopy" => {
            let v = l.variant_by_name(rest).unwrap();
            l.lab_copy_loadout_from(v).unwrap();
            writeln!(out, "LABCOPY {rest}").unwrap();
        }
        "canafford" => {
            writeln!(out, "CANAFFORD => {}", code(l.lab_can_afford())).unwrap();
            return out;
        }
        "accept" => {
            let r = code(l.accept_mech_modifications(rest));
            assert_ne!(r, 1019, "the script only accepts names canreplace allows");
            writeln!(out, "ACCEPT {rest} => {r}").unwrap();
        }
        "acceptold" => {
            let r = code(l.accept_mech_modifications_use_old_variant(rest));
            writeln!(out, "ACCEPTOLD {rest} => {r}").unwrap();
        }
        "cancel" => {
            writeln!(out, "CANCEL => {}", code(l.cancel_mech_modifications())).unwrap();
        }
        "canreplace" => {
            writeln!(
                out,
                "CANREPLACE {rest} => {}",
                i32::from(l.can_replace_variant(rest))
            )
            .unwrap();
            return out;
        }
        "candelete" => {
            writeln!(
                out,
                "CANDELETE {rest} => {}",
                i32::from(l.can_delete_variant(rest))
            )
            .unwrap();
            return out;
        }
        "rmvariant" => {
            let r = code(l.remove_variant(rest));
            writeln!(out, "RMVARIANT {rest} => {r}").unwrap();
        }
        other => panic!("unknown command {other}"),
    }
    out += &dump_bay(l);
    out
}

/// The pilot lines the oracle prints (`dumpPilot`).
pub fn dump_pilot(p: &mc2_campaign::pilot::Pilot) -> String {
    use mc2_campaign::pilot::{MAX_MEDAL, MAX_MISSIONS};
    let mut s = String::new();
    writeln!(
        s,
        "  P {} rank {} g {:.4} p {:.4} ng {:.4} np {:.4} kills {}/{}/{} done {} used {} dead {} just {} avail {}",
        p.file_name,
        p.rank,
        p.gunnery,
        p.piloting,
        p.new_gunnery,
        p.new_piloting,
        p.mech_kills,
        p.vehicle_kills,
        p.infantry_kills,
        p.missions_completed,
        i32::from(p.used),
        i32::from(p.dead),
        i32::from(p.just_died),
        i32::from(p.available)
    )
    .unwrap();
    let bits = |v: &[bool]| {
        v.iter()
            .map(|b| if *b { '1' } else { '0' })
            .collect::<String>()
    };
    write!(s, "    played ").unwrap();
    for i in 0..MAX_MISSIONS {
        write!(s, "{}", p.missions_played[i]).unwrap();
    }
    writeln!(
        s,
        " medals {} last {} spec {}",
        bits(&p.medals[..MAX_MEDAL]),
        bits(&p.medals_last_mission[..MAX_MEDAL]),
        bits(&p.specialty_skills)
    )
    .unwrap();
    s
}

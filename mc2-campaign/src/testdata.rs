//! A small synthetic [`GameData`] for unit tests that do not need the shipped files.
//!
//! Components (ids 10-17, see `variant::test_support`), two chassis (`alpha`: 4x3 grid, jump
//! jets, ECM and sensors; `beta`: 3x3, none), designer variants, four pilots, the five-mission
//! three-group campaign of `mission_info::tests`, and purchase lists for it.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc2_sim_data::campaign::PurchaseList;

use crate::data::{purchase_key, GameData, PilotTemplate};
use crate::variant::test_support::{catalog, chassis, BIG_GUN, JUMPJET, LASER, PPC};
use crate::variant::{Chassis, VariantSpec};

pub(crate) fn list(components: &[i64], mechs: &[&str], pilots: &[&str]) -> PurchaseList {
    PurchaseList {
        file_name: String::new(),
        all_components: false,
        components: components.to_vec(),
        mechs: mechs.iter().map(|s| s.to_string()).collect(),
        pilots: pilots.iter().map(|s| s.to_string()).collect(),
    }
}

pub(crate) fn pilot(id: u32, name: &str, gunnery: f32, piloting: f32) -> PilotTemplate {
    PilotTemplate {
        id,
        file_name: name.into(),
        name_id: 100 + i64::from(id),
        audio: String::new(),
        video: String::new(),
        photo_index: 0,
        flavor_text_id: 0,
        icon_file: String::new(),
        gunnery,
        piloting,
        specialty_skills: [false; 23],
    }
}

pub(crate) fn beta() -> Chassis {
    let mut c = chassis();
    c.index = 1;
    c.id = 0x0301;
    c.file_name = "data/objects/beta.csv".into();
    c.max_weight = 80.0;
    c.chassis_name_id = 888;
    c.base_cost = 200_000;
    c.component_area_width = 3;
    c.component_area_height = 3;
    c.can_have_jump_jets = false;
    c.can_have_ecm = false;
    c.can_have_adv_sensor = false;
    c.fit_id = 7;
    c
}

pub(crate) fn alpha() -> Chassis {
    let mut c = chassis();
    c.file_name = "data/objects/alpha.csv".into();
    c.fit_id = 5;
    c
}

pub(crate) fn data() -> Arc<GameData> {
    let cat = catalog();
    let spec = |file_id: i32, name: &str, hidden: bool, comps: &[usize]| VariantSpec {
        file_id,
        name: name.into(),
        hidden,
        components: comps.iter().map(|&c| (c, -1, -1)).collect(),
    };
    let mut campaigns = BTreeMap::new();
    campaigns.insert(
        "campaign".to_string(),
        crate::mission_info::tests::campaign(),
    );

    let mut lists = BTreeMap::new();
    let mut put = |name: &str, l: PurchaseList| {
        lists.insert(purchase_key(name), l);
    };
    // component ids: laser 10, ppc 11, heat sink 12, jump jet 13, bulk 14, ecm 15, sensor 16, big gun 17
    put("P_a", list(&[10, 13, 12], &["alpha"], &["p1", "p2"]));
    put(
        "P_b",
        list(&[10, 11, 12, 13], &["alpha", "beta"], &["p1", "p2", "p3"]),
    );
    put("P_c", list(&[10, 11], &["beta"], &["p1"]));
    put("P_d", list(&[10], &["alpha"], &["p1"]));
    put("P_e", list(&[10, 11, 17], &["beta"], &["p4"]));
    put("purchase", list(&[10], &[], &[]));
    put("bonus", list(&[11], &["beta"], &["p3"]));

    Arc::new(GameData {
        catalog: cat,
        chassis: vec![alpha(), beta()],
        designer_variants: vec![
            (0, spec(0, "Alpha Prime", false, &[LASER, LASER, JUMPJET])),
            (0, spec(1, "Alpha Hidden", true, &[LASER])),
            (1, spec(0, "Beta Wide", false, &[BIG_GUN, LASER])),
            (1, spec(1, "Beta Prime", false, &[PPC])),
        ],
        pilots: vec![
            pilot(1, "p1", 40.0, 40.0),
            pilot(2, "p2", 55.0, 50.0),
            pilot(3, "p3", 60.0, 62.0),
            pilot(4, "p4", 70.0, 75.0),
        ],
        campaigns,
        purchase_lists: lists,
        vehicles: Vec::new(),
        buildings: Vec::new(),
    })
}

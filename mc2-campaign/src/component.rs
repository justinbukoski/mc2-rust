// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The component catalogue the mech lab and the purchase screens draw from.
//!
//! The parser lives in `mc2-sim-data` (`component::logistics`, a port of
//! `code/logisticscomponent.cpp` and `LogisticsData::initComponents`), next to the master
//! component loader that reads the same file; this module only gives the types their campaign
//! names.

pub use mc2_sim_data::component::logistics::{
    parse_row, LogisticsCatalog as ComponentCatalog, LogisticsComponent as Component, ParsedRow,
    WeaponRangeType, COMPONENT_FORMS,
};
pub use mc2_sim_data::component::COMPBAS_FILE;

/// Convenience for tests: a component with the given form, `(weight, heat, damage)`, cost and
/// `(width, height)`.
#[cfg(test)]
pub(crate) fn test_component(
    id: i32,
    form: mc2_sim_data::component::ComponentForm,
    (weight, heat, damage): (f32, f32, f32),
    cost: i32,
    (width, height): (i32, i32),
) -> Component {
    Component {
        id,
        form,
        recycle_time: 1.0,
        heat,
        weight,
        damage,
        cost,
        range_type: if form.is_weapon() {
            WeaponRangeType::Short
        } else {
            WeaponRangeType::None
        },
        head: false,
        torso: false,
        legs: false,
        ammo: 0,
        icon_file: "icon.tga".into(),
        picture_file: None,
        string_id: 0,
        help_string_id: 0,
        width,
        height,
    }
}

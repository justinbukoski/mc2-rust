// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The loadout list of a mech (`ComponentListBox`, `code/simplecomponentlistbox.cpp`): the
//! components the mech carries, grouped short, medium, long and then the bandless ones, each group
//! listed once with its count, then the sensor and the ECM. The mech bay and the mech display show
//! it beside the statistics. It is read-only: the mech lab is where components change.

use super::{strings, text_item};
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::font::FontRef;
use crate::widgets::ListBox;
use mc2_campaign::{Logistics, VariantKey};
use mc2_sim_data::component::ComponentForm;

/// `colors[4]` of `setComponents`: short, medium, long, and the components with no range.
const BAND_COLOURS: [u32; 4] = [0xff6e_7c00, 0xff00_5392, 0xffa2_1600, 0xffc2_9b00];
/// The sensor and ECM lines (`0xffc29b00`).
const SENSOR_COLOUR: u32 = 0xffc2_9b00;
/// `IDS_SALVAGE_AREA_COMPONENTS` ids: the sensor and the ECM names are `32000 + id`.
const COMPONENT_NAME_BASE: u32 = 32000;

/// One line of the loadout list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadoutRow {
    /// `"N/N name"`, or the bare name for a jump jet, or the sensor or ECM name.
    pub text: String,
    pub colour: u32,
    /// How many of the component the mech carries; `0` for the sensor and the ECM.
    pub count: usize,
}

/// `ComponentListBox::setComponents`: the distinct components of `components` (catalogue
/// indices) per range band, in the order they first appear, each with its count.
pub fn component_rows(
    assets: &UiAssets,
    catalog: &mc2_campaign::component::ComponentCatalog,
    components: &[usize],
) -> Vec<LoadoutRow> {
    let mut rows = Vec::new();
    for (band, &colour) in BAND_COLOURS.iter().enumerate() {
        // distinct components of this band, in the order they first appear, with their counts
        let mut found: Vec<(usize, usize)> = Vec::new();
        for &component in components {
            let Some(c) = catalog.get(component) else {
                continue;
            };
            if c.range_type as usize != band {
                continue;
            }
            match found.iter_mut().find(|(index, _)| *index == component) {
                Some((_, n)) => *n += 1,
                None => found.push((component, 1)),
            }
        }
        for (index, count) in found {
            let Some(c) = catalog.get(index) else {
                continue;
            };
            let name = assets.string(c.string_id as u32);
            let text = if c.form != ComponentForm::JumpJet {
                format!("{count}/{count} {name}")
            } else {
                name.to_string()
            };
            rows.push(LoadoutRow {
                text,
                colour,
                count,
            });
        }
    }
    rows
}

/// The rows `ComponentListBox::setMech` adds for `variant`.
pub fn loadout_rows(assets: &UiAssets, lg: &Logistics, variant: VariantKey) -> Vec<LoadoutRow> {
    let (Some(v), Some(chassis)) = (lg.variant(variant), lg.chassis_of(variant)) else {
        return Vec::new();
    };
    let ctx = lg.ctx_for(v);
    let indices: Vec<usize> = v.components.iter().map(|p| p.component).collect();
    let mut rows = component_rows(assets, ctx.catalog, &indices);
    for id in [chassis.sensor_id(), chassis.ecm_id()] {
        if id > 0 {
            rows.push(LoadoutRow {
                text: assets.string(COMPONENT_NAME_BASE + id as u32).to_string(),
                colour: SENSOR_COLOUR,
                count: 0,
            });
        }
    }
    rows
}

/// The loadout list box: a [`ListBox`] of text entries in the list font.
pub struct Loadout {
    pub list: ListBox,
    font: FontRef,
}

impl Loadout {
    /// `ComponentListBox::init(x, y, w, h)` with the list font of the mech screens.
    pub fn new(ld: &mut Loader<'_>, x: f32, y: f32, w: f32, h: f32) -> Loadout {
        let font = ld.font(strings::MN_LB_FONT);
        Loadout {
            list: ListBox::new(ld, x, y, w, h),
            font,
        }
    }

    /// `setMech(variant)`: the list shows the variant's loadout, or nothing for `None`.
    pub fn set_mech(&mut self, assets: &UiAssets, lg: &Logistics, variant: Option<VariantKey>) {
        self.clear();
        let Some(variant) = variant else {
            return;
        };
        let width = self.list.obj.width() - self.list.scroll_bar_width() - 10.0;
        for row in loadout_rows(assets, lg, variant) {
            let item = text_item(assets, self.font, width, &row.text, row.colour);
            self.list.add_item(Box::new(item));
        }
    }

    /// `setMech(NULL)`: the list empties.
    pub fn clear(&mut self) {
        self.list.remove_all_items();
    }

    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.list.update(ctx);
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
    }
}

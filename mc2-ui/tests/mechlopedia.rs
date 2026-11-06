// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Shipped-data, headless Mechlopedia evidence.  These tests intentionally use pointer input:
//! selection must follow the same list-box path a player uses, rather than a test-only setter.

use std::sync::{Arc, Mutex, OnceLock};

use mc2_campaign::component::ComponentCatalog;
use mc2_campaign::{GameData, Logistics};
use mc2_formats::csv::CsvFile;
use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_sim_data::component::ComponentForm;
use mc2_sim_data::mech::load_chassis;
use mc2_ui::harness::{events, Harness};
use mc2_ui::mechlopedia::{Category, Mechlopedia};
use mc2_ui::preview::ModelPreviews;
use mc2_ui::textures::art_file_path;
use mc2_ui::widgets::ListItem;
use mc2_ui::{Loader, UiAssets};

fn open_vfs() -> Option<Vfs> {
    std::env::var_os("MC2_DATA").map(|root| Vfs::from_system_cfg(root).expect("MC2 VFS"))
}

// The shipped FST reader owns shared archive buffers; serialize the two integration tests just
// as the workspace gate may run this binary with more than one test thread.
fn data_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn click(h: &mut Harness, screen: &mut Mechlopedia, point: (f32, f32)) {
    for event in [
        events::move_to(point.0, point.1),
        events::left(true),
        events::left(false),
    ] {
        h.frame(std::slice::from_ref(&event), |ctx| screen.update(ctx));
    }
}

fn centre(item: &dyn ListItem) -> (f32, f32) {
    let o = item.obj();
    (
        o.global_x() as f32 + o.width() / 2.0,
        o.global_y() as f32 + o.height() / 2.0,
    )
}

fn switch(h: &mut Harness, screen: &mut Mechlopedia, category: Category) {
    let button = screen
        .base
        .button(Mechlopedia::button_id(category))
        .expect("category button");
    click(
        h,
        screen,
        (
            button.global_x() as f32 + button.width() / 2.0,
            button.global_y() as f32 + button.height() / 2.0,
        ),
    );
}

/// Select a row through the same pointer/list-box path as a player, including rows below the
/// initial viewport. `set_scroll_pos` is ordinary list-box state, not a test-only selection API.
fn select_row(h: &mut Harness, screen: &mut Mechlopedia, row: usize) {
    let item = screen.group_list.item(row).expect("row to select");
    let visible = screen.group_list.point_inside(
        item.obj().global_x(),
        item.obj().global_y() + item.obj().height() as i32 / 2,
    );
    if !visible {
        let offset = item.obj().global_y() as f32
            - screen.group_list.obj.global_y() as f32
            - screen.group_list.obj.height() / 2.0;
        screen
            .group_list
            .set_scroll_pos(offset.max(0.0).round() as i32);
        h.idle(1, |ctx| screen.update(ctx));
    }
    let at = centre(screen.group_list.item(row).expect("row to select"));
    // The FIT list rows become hit-testable after their first update frame, just as they do
    // before a player can click an opened category.
    h.idle(8, |ctx| screen.update(ctx));
    click(h, screen, at);
    assert_eq!(screen.group_list.selected_item(), row as i32);
}

fn rendered_texture_names(h: &Harness, screen: &mut Mechlopedia) -> Vec<String> {
    let draw = h.render(|ctx| screen.render(ctx));
    draw.batches
        .iter()
        .filter_map(|batch| batch.texture)
        .filter_map(|texture| h.assets.textures.get(texture))
        .map(|image| image.name.clone())
        .collect()
}

fn rendered_texture_rect(
    h: &Harness,
    screen: &mut Mechlopedia,
    texture_name: &str,
) -> Option<(f32, f32, f32, f32)> {
    let texture = h.assets.textures.find(texture_name)?;
    let draw = h.render(|ctx| screen.render(ctx));
    draw.batches
        .iter()
        .find(|batch| batch.texture == Some(texture) && batch.quad_count != 0)
        .map(|batch| {
            let quad = &draw.vertices[batch.first_quad as usize * 4..][..4];
            (quad[0].x, quad[0].y, quad[2].x, quad[2].y)
        })
}

fn assert_preview_has_transparent_background_and_model(h: &Harness, name: &str) {
    let texture = h
        .assets
        .textures
        .find(name)
        .and_then(|id| h.assets.textures.get(id))
        .expect("resolved preview texture");
    let pixels = texture.rgba.as_chunks::<4>().0;
    let transparent = pixels.iter().filter(|px| px[3] == 0).count();
    let visible = pixels.iter().filter(|px| px[3] != 0).count();
    assert!(
        transparent > 0,
        "{name} keys its offscreen background transparent"
    );
    assert!(visible > 0, "{name} contains visible model pixels");
}

/// The C++ screen's independently selected description string for every supported category.
/// These are sourced from shipped IDs/records, never from `Mechlopedia` output.
#[derive(Debug)]
struct ExpectedRow {
    label: String,
    description: String,
}

/// Reads `buildings.csv` and each listed chassis CSV through a second VFS.  This deliberately
/// does not use `GameData`, `Logistics::encyclopedia_mechs`, or the screen's list builder.
fn expected_mech_rows(vfs: &mut Vfs, assets: &UiAssets) -> Vec<ExpectedRow> {
    let bytes = vfs
        .read("data/art/buildings.csv")
        .expect("read shipped buildings.csv")
        .expect("shipped buildings.csv");
    let buildings = CsvFile::from_bytes(bytes).expect("parse shipped buildings.csv");
    let mut rows = Vec::new();
    for row in 1.. {
        let Some(kind) = buildings
            .read_string(row, 4, 256)
            .expect("read shipped buildings.csv kind")
        else {
            break;
        };
        if !kind.eq_ignore_ascii_case("MECH") {
            continue;
        }
        let stem = buildings
            .read_string(row, 1, 256)
            .expect("read shipped chassis stem")
            .expect("MECH row has a chassis stem")
            .to_ascii_lowercase();
        let chassis = load_chassis(vfs, &stem).expect("parse shipped chassis CSV");
        let label = assets
            .string(chassis.descriptor_id.unwrap_or_default() as u32)
            .to_uppercase();
        let description = assets
            .string(chassis.abbr_index.unwrap_or_default() as u32)
            .to_string();
        rows.extend(
            chassis
                .variants
                .iter()
                .filter(|variant| variant.name.contains("Prime"))
                .map(|_| ExpectedRow {
                    label: label.clone(),
                    description: description.clone(),
                }),
        );
    }
    rows.sort_by_cached_key(|row| row.label.to_ascii_lowercase());
    rows
}

/// Player-visible list and description values from shipped records, never from `Mechlopedia`.
fn expected_rows(category: Category, assets: &UiAssets, vfs: &mut Vfs) -> Vec<ExpectedRow> {
    match category {
        Category::Mechs => expected_mech_rows(vfs, assets),
        Category::Weapons => {
            let catalog = ComponentCatalog::load(vfs).expect("parse shipped component data");
            let mut rows: Vec<_> = (0..catalog.len()).collect();
            rows.sort_by_cached_key(|i| {
                catalog
                    .get(*i)
                    .map(|c| assets.string(c.string_id as u32).to_ascii_lowercase())
                    .unwrap_or_default()
            });
            rows.into_iter()
                .map(|i| {
                    let component = catalog.get(i).expect("shipped component");
                    ExpectedRow {
                        label: assets.string(component.string_id as u32).to_uppercase(),
                        description: assets.string(component.help_string_id as u32).to_string(),
                    }
                })
                .collect()
        }
        Category::Personalities => {
            let mut rows: Vec<i32> = (0..11).collect();
            rows.sort_by_cached_key(|i| assets.string((45300 + *i) as u32).to_ascii_lowercase());
            rows.into_iter()
                .map(|i| ExpectedRow {
                    label: assets.string((45300 + i) as u32).to_uppercase(),
                    description: assets.string((45316 + i) as u32).to_string(),
                })
                .collect()
        }
        Category::History => (0..5)
            .map(|i| ExpectedRow {
                label: assets.string((45360 + i) as u32).to_uppercase(),
                description: assets.string((45371 + i) as u32).to_string(),
            })
            .collect(),
        Category::Buildings | Category::Vehicles => Vec::new(),
    }
}

/// The C++ `ComponentListBox::setMech` grouping, calculated directly from `buildings.csv`, each
/// chassis CSV, and `compbas.csv` in a separate VFS.  It intentionally does not use `GameData`,
/// `Logistics`, or `loadout_rows`, so an omission in the UI path cannot satisfy this evidence.
fn expected_mech_loadouts(vfs: &mut Vfs, assets: &UiAssets) -> Vec<(String, Vec<String>)> {
    let catalog = ComponentCatalog::load(vfs).expect("parse independent shipped component data");
    let bytes = vfs
        .read("data/art/buildings.csv")
        .expect("read independent shipped buildings.csv")
        .expect("shipped buildings.csv");
    let buildings = CsvFile::from_bytes(bytes).expect("parse independent shipped buildings.csv");
    let mut loadouts = Vec::new();
    for row in 1.. {
        let Some(kind) = buildings
            .read_string(row, 4, 256)
            .expect("read independent shipped buildings.csv kind")
        else {
            break;
        };
        if !kind.eq_ignore_ascii_case("MECH") {
            continue;
        }
        let stem = buildings
            .read_string(row, 1, 256)
            .expect("read independent shipped chassis stem")
            .expect("MECH row has a chassis stem")
            .to_ascii_lowercase();
        let chassis = load_chassis(vfs, &stem).expect("parse independent shipped chassis CSV");
        let label = assets
            .string(chassis.descriptor_id.unwrap_or_default() as u32)
            .to_uppercase();
        for variant in chassis
            .variants
            .iter()
            .filter(|variant| variant.name.contains("Prime"))
        {
            // `LogisticsVariant::init` (`code/logisticsvariant.cpp:365-385`) replays the
            // CSV placements largest-footprint first: height 4..1, width 3..1, retaining CSV
            // order inside each footprint.  This is input construction evidence, not the
            // `loadout_rows` grouping being tested below.
            let mut installed = Vec::new();
            for height in (1..=4).rev() {
                for width in (1..=3).rev() {
                    for item in variant.items.iter().take(71) {
                        let Some(id) = item.comp_bas else { continue };
                        let Some(index) = catalog.find(id as i32) else {
                            continue;
                        };
                        let component = catalog.get(index).expect("catalogue index from find");
                        if component.width == width && component.height == height {
                            installed.push(index);
                        }
                    }
                }
            }
            let mut lines = Vec::new();
            for band in 0..4 {
                let mut found: Vec<(usize, usize)> = Vec::new();
                for &index in &installed {
                    let component = catalog.get(index).expect("catalogue index from find");
                    if component.range_type as usize != band {
                        continue;
                    }
                    match found.iter_mut().find(|(seen, _)| *seen == index) {
                        Some((_, count)) => *count += 1,
                        None => found.push((index, 1)),
                    }
                }
                for (index, count) in found {
                    let component = catalog.get(index).expect("grouped component");
                    let name = assets.string(component.string_id as u32);
                    if component.form == ComponentForm::JumpJet {
                        lines.push(name.to_string());
                    } else {
                        lines.push(format!("{count}/{count} {name}"));
                    }
                }
            }
            let sensor = if chassis.allow.advanced_sensor {
                Some(15)
            } else if chassis.allow.extended_sensor {
                Some(14)
            } else if chassis.allow.global_sensor {
                Some(17)
            } else {
                None
            };
            for id in sensor.into_iter().chain(chassis.allow.ecm.then_some(38)) {
                lines.push(assets.string(32000 + id).to_string());
            }
            loadouts.push((label.clone(), lines));
        }
    }
    loadouts.sort_by_cached_key(|(label, _)| label.to_ascii_lowercase());
    loadouts
}

#[test]
fn shipped_lists_are_visible_sorted_and_select_distinct_details() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    screen.load_visuals(&mut Loader::new(&mut vfs, &mut assets));
    let mut h = Harness::with_assets(assets);
    let mut evidence_vfs = open_vfs().expect("independent shipped-data VFS");

    for category in [
        Category::Mechs,
        Category::Weapons,
        Category::Personalities,
        Category::History,
    ] {
        switch(&mut h, &mut screen, category);
        h.idle(1, |ctx| screen.update(ctx));
        let rows = screen.group_text();
        let expected = expected_rows(category, &h.assets, &mut evidence_vfs);
        assert!(rows.len() >= 2, "{category:?} has shipped entries");
        assert_eq!(
            rows,
            expected
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
            "{category:?} has every independently read shipped row in display order"
        );
        if category != Category::History {
            let mut sorted = rows.clone();
            sorted.sort_by_cached_key(|row| row.to_ascii_lowercase());
            assert_eq!(rows, sorted, "{category:?} is ordered by displayed label");
        }
        assert!(
            screen.group_list.item(0).expect("first row").obj().height() > 0.0,
            "{category:?} row font is loaded"
        );
        assert_eq!(
            screen.group_list.item(0).expect("first row").obj().loc[0].argb,
            0xff86_6234,
            "{category:?} starts with the C++ selected-row tint"
        );
        for row in [0, 1] {
            select_row(&mut h, &mut screen, row);
            assert!(
                screen.detail_text().contains(&expected[row].description),
                "{category:?} row {row} uses its independently sourced shipped description"
            );
        }
    }
}

#[test]
fn shipped_mech_and_component_stats_follow_the_visible_cxx_rules() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    screen.load_visuals(&mut Loader::new(&mut vfs, &mut assets));

    // Expected player-visible values are derived directly from the shipped variant/component
    // records, not from `Mechlopedia::detail_text`.
    let mut mechs = logistics.encyclopedia_mechs();
    mechs.sort_by_cached_key(|key| {
        logistics
            .variant(*key)
            .and_then(|v| data.chassis.get(v.chassis))
            .map(|c| assets.string(c.chassis_name_id as u32).to_ascii_lowercase())
            .unwrap_or_default()
    });
    assert!(mechs.len() >= 2);
    let mech_expected: Vec<Vec<String>> = mechs
        .iter()
        .take(2)
        .map(|key| {
            let variant = logistics.variant(*key).expect("variant");
            let chassis = data.chassis.get(variant.chassis).expect("chassis");
            let ctx = logistics.ctx_for(variant);
            vec![
                format!("HOUSE : {}", assets.string(45131 + chassis.house_id as u32)),
                format!(
                    "ARMOR: {} : {}",
                    assets.string(chassis.armor_class_string_id()),
                    variant.armor(&ctx)
                ),
                format!("JUMP : {} m", variant.jump_range(&ctx) * 25),
            ]
        })
        .collect();
    let mut components: Vec<usize> = (0..data.catalog.len()).collect();
    components.sort_by_cached_key(|i| {
        data.catalog
            .get(*i)
            .map(|c| assets.string(c.string_id as u32).to_ascii_lowercase())
            .unwrap_or_default()
    });
    let bulk_row = components
        .iter()
        .position(|i| {
            data.catalog
                .get(*i)
                .is_some_and(|c| c.form == ComponentForm::Bulk)
        })
        .expect("shipped bulk component");
    let ammo_row = components
        .iter()
        .position(|i| data.catalog.get(*i).is_some_and(|c| c.ammo != 0))
        .expect("shipped ammunition component");
    let ammo = data
        .catalog
        .get(components[ammo_row])
        .expect("ammo component")
        .ammo;

    let mut h = Harness::with_assets(assets);
    for (row, expected) in mech_expected.iter().enumerate() {
        select_row(&mut h, &mut screen, row);
        let detail = screen.detail_text();
        for value in expected {
            assert!(
                detail.contains(value),
                "mech row {row} displays {value:?}: {detail:?}"
            );
        }
    }

    switch(&mut h, &mut screen, Category::Weapons);
    select_row(&mut h, &mut screen, bulk_row);
    assert!(screen.detail_text().contains(&"ARMOR : 32".to_string()));
    let picture_row = components
        .iter()
        .position(|i| {
            data.catalog
                .get(*i)
                .is_some_and(|c| c.picture_file.is_some())
        })
        .expect("shipped component with a 3D picture");
    select_row(&mut h, &mut screen, picture_row);
    assert!(
        !screen
            .detail_text()
            .iter()
            .any(|line| line.contains("NOT YET AVAILABLE") || line.contains("UNPORTED")),
        "a component with a picture shows no placeholder: {:?}",
        screen.detail_text()
    );
    let rate_row = components
        .iter()
        .position(|i| data.catalog.get(*i).is_some_and(|c| c.recycle_time != 0.0))
        .expect("shipped component with a rate of fire");
    let rate = data
        .catalog
        .get(components[rate_row])
        .expect("rate component");
    select_row(&mut h, &mut screen, rate_row);
    assert!(
        screen
            .detail_text()
            .contains(&format!("RATE OF FIRE: {:.6}", 10.0 / rate.recycle_time)),
        "rate of fire uses the C++ %f precision"
    );
    screen.set_unlimited_ammo(false, &h.assets);
    select_row(&mut h, &mut screen, ammo_row);
    assert!(
        screen.detail_text().contains(&format!("AMMO : {ammo}")),
        "limited-ammo preference shows the shipped component's ammunition"
    );
    screen.set_unlimited_ammo(true, &h.assets);
    assert!(
        !screen.detail_text().contains(&format!("AMMO : {ammo}")),
        "unlimited-ammo preference hides ammunition as the C++ screen does"
    );
}

#[test]
fn shipped_mech_loadouts_use_the_cxx_grouped_rows_including_sensor_and_ecm() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    let mut h = Harness::with_assets(assets);
    let mut evidence_vfs = open_vfs().expect("independent shipped-data VFS");
    let expected = expected_mech_loadouts(&mut evidence_vfs, &h.assets);
    assert!(
        expected.len() >= 2,
        "shipped data has multiple encyclopedia mechs"
    );
    assert_eq!(
        screen.group_text(),
        expected
            .iter()
            .map(|(label, _)| label.clone())
            .collect::<Vec<_>>(),
        "the mech list agrees with independently read buildings and chassis CSVs"
    );
    for (row, (_, expected_rows)) in expected.into_iter().take(2).enumerate() {
        select_row(&mut h, &mut screen, row);
        assert_eq!(
            screen.component_text(),
            expected_rows,
            "mech row {row} preserves every independently sourced range-band row, count, sensor and ECM entry"
        );
    }
}

#[test]
fn selected_models_icons_and_portraits_render_from_shipped_files() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    screen.load_visuals(&mut Loader::new(&mut vfs, &mut assets));
    let mut h = Harness::with_assets(assets);
    let gpu = match Gpu::headless(&RenderConfig::from_env()) {
        Ok(gpu) => gpu,
        Err(error)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|value| value == "1")
                && format!("{error:?}").contains("no suitable GPU adapter") =>
        {
            eprintln!(
                "SKIPPED mechlopedia preview: MC2_SKIP_GPU_TESTS=1 and no usable GPU ({error:?})"
            );
            return;
        }
        Err(error) => panic!("headless preview GPU: {error:?}"),
    };
    let mut previews = ModelPreviews::new(
        gpu,
        mc2_render::game::GameData::open(root).expect("preview data"),
    )
    .expect("preview renderer");

    // Rendering asks for the first selected model. Resolving it is the production host's second
    // half of the frame, and the next render must reference the stored model picture.
    let _ = rendered_texture_names(&h, &mut screen);
    previews.resolve(&mut h.assets);
    let first_model = rendered_texture_names(&h, &mut screen)
        .into_iter()
        .find(|name| name.starts_with("preview:"))
        .expect("first selected mech preview is drawn");
    assert_preview_has_transparent_background_and_model(&h, &first_model);
    select_row(&mut h, &mut screen, 1);
    let _ = rendered_texture_names(&h, &mut screen);
    previews.resolve(&mut h.assets);
    let second_model = rendered_texture_names(&h, &mut screen)
        .into_iter()
        .find(|name| name.starts_with("preview:") && name != &first_model)
        .expect("second selected mech preview is distinct and drawn");
    assert_ne!(first_model, second_model);
    assert_preview_has_transparent_background_and_model(&h, &second_model);
    let second_pixels = h
        .assets
        .textures
        .find(&second_model)
        .and_then(|id| h.assets.textures.get(id))
        .expect("second preview texture")
        .rgba
        .clone();

    // `MechScreen::setMech` gets both prefs fields: the highlight fills red and green masks,
    // and base fills blue. A changed scheme receives a distinct picture key and pixels.
    screen.set_preview_paint(0x0011_2233, 0x0044_5566, &h.assets);
    let _ = rendered_texture_names(&h, &mut screen);
    previews.resolve(&mut h.assets);
    let repainted = rendered_texture_names(&h, &mut screen)
        .into_iter()
        .find(|name| name.starts_with("preview:") && name != &second_model)
        .expect("custom base and highlight paints make a distinct selected preview");
    assert!(repainted.contains(":445566:445566:112233:"));
    let repainted_pixels = &h
        .assets
        .textures
        .find(&repainted)
        .and_then(|id| h.assets.textures.get(id))
        .expect("repainted preview texture")
        .rgba;
    assert_ne!(&second_pixels, repainted_pixels);

    switch(&mut h, &mut screen, Category::Weapons);
    select_row(&mut h, &mut screen, 0);
    assert!(
        rendered_texture_names(&h, &mut screen)
            .iter()
            .any(|name| name.contains("data/art/") && !name.starts_with("preview:")),
        "selected weapon/component icon is drawn"
    );
    // A component with a picture file shows its building appearance (`SimpleCamera::setComponent`).
    let mut components: Vec<_> = (0..data.catalog.len()).collect();
    components.sort_by_cached_key(|i| {
        data.catalog
            .get(*i)
            .map(|c| h.assets.string(c.string_id as u32).to_ascii_lowercase())
            .unwrap_or_default()
    });
    let picture_rows: Vec<usize> = components
        .iter()
        .enumerate()
        .filter(|(_, i)| {
            data.catalog
                .get(**i)
                .is_some_and(|c| c.picture_file.is_some())
        })
        .map(|(row, _)| row)
        .collect();
    assert!(!picture_rows.is_empty(), "shipped components with pictures");
    let mut drawn = 0;
    for row in picture_rows {
        select_row(&mut h, &mut screen, row);
        let _ = rendered_texture_names(&h, &mut screen);
        previews.resolve(&mut h.assets);
        let names = rendered_texture_names(&h, &mut screen);
        if let Some(name) = names.iter().find(|n| n.starts_with("preview:bldg:")) {
            assert_preview_has_transparent_background_and_model(&h, name);
            drawn += 1;
        }
        assert!(
            !screen
                .detail_text()
                .iter()
                .any(|line| line.contains("NOT YET AVAILABLE")),
            "no placeholder on component row {row}"
        );
    }
    assert!(
        drawn > 0,
        "at least one component picture is rendered and drawn"
    );

    switch(&mut h, &mut screen, Category::Personalities);
    select_row(&mut h, &mut screen, 0);
    assert!(
        rendered_texture_names(&h, &mut screen)
            .iter()
            .any(|name| name.contains("mcl_en_person_")),
        "selected personality portrait is drawn"
    );
    switch(&mut h, &mut screen, Category::History);
    select_row(&mut h, &mut screen, 0);
    assert!(
        rendered_texture_names(&h, &mut screen)
            .iter()
            .any(|name| name.contains("mcl_en_history_")),
        "selected history portrait is drawn"
    );
}

#[test]
fn a_missing_selected_mech_shape_is_explicitly_not_available() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let mut data = GameData::load(&mut vfs).expect("shipped campaign data");
    // Exercise the production `ModelPreviews::resolve` failure path after successful renderer
    // initialization.  The selected chassis names a shape that cannot exist in shipped data.
    for chassis in &mut data.chassis {
        chassis.file_name = "data/objects/not_a_shipped_preview_shape.csv".into();
    }
    let data = Arc::new(data);
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    let mut h = Harness::with_assets(assets);
    let gpu = match Gpu::headless(&RenderConfig::from_env()) {
        Ok(gpu) => gpu,
        Err(error)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|value| value == "1")
                && format!("{error:?}").contains("no suitable GPU adapter") =>
        {
            eprintln!(
                "SKIPPED mechlopedia preview: MC2_SKIP_GPU_TESTS=1 and no usable GPU ({error:?})"
            );
            return;
        }
        Err(error) => panic!("headless preview GPU: {error:?}"),
    };
    let mut previews = ModelPreviews::new(
        gpu,
        mc2_render::game::GameData::open(root).expect("preview data"),
    )
    .expect("preview renderer");

    let _ = rendered_texture_names(&h, &mut screen);
    previews.resolve(&mut h.assets);
    let _ = rendered_texture_names(&h, &mut screen);
    assert!(
        screen.detail_text()[0].contains("MECH MODEL NOT AVAILABLE"),
        "a failed selected shape has an explicit first visible fallback"
    );
}

#[test]
fn a_failed_component_picture_is_explicitly_not_available() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let mut data = GameData::load(&mut vfs).expect("shipped campaign data");
    // the renderer starts, but every component's picture names a shape that cannot exist
    let components: Vec<_> = (0..data.catalog.len())
        .filter_map(|i| data.catalog.get(i).cloned())
        .map(|mut c| {
            if c.picture_file.is_some() {
                c.picture_file = Some("not_a_shipped_preview_shape".into());
            }
            c
        })
        .collect();
    data.catalog =
        mc2_sim_data::component::logistics::LogisticsCatalog::from_components(components);
    let data = Arc::new(data);
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    let mut h = Harness::with_assets(assets);
    let gpu = match Gpu::headless(&RenderConfig::from_env()) {
        Ok(gpu) => gpu,
        Err(error)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|value| value == "1")
                && format!("{error:?}").contains("no suitable GPU adapter") =>
        {
            eprintln!("SKIPPED component preview failure: no usable GPU ({error:?})");
            return;
        }
        Err(error) => panic!("headless preview GPU: {error:?}"),
    };
    let mut previews = ModelPreviews::new(
        gpu,
        mc2_render::game::GameData::open(root).expect("preview data"),
    )
    .expect("preview renderer");
    switch(&mut h, &mut screen, Category::Weapons);
    let row = (0..screen.group_text().len())
        .find(|row| {
            let mut components: Vec<_> = (0..data.catalog.len()).collect();
            components.sort_by_cached_key(|i| {
                data.catalog
                    .get(*i)
                    .map(|c| h.assets.string(c.string_id as u32).to_ascii_lowercase())
                    .unwrap_or_default()
            });
            data.catalog
                .get(components[*row])
                .is_some_and(|c| c.picture_file.is_some())
        })
        .expect("a component with a picture");
    select_row(&mut h, &mut screen, row);
    let _ = rendered_texture_names(&h, &mut screen);
    previews.resolve(&mut h.assets);
    let _ = rendered_texture_names(&h, &mut screen);
    assert!(
        screen.detail_text()[0].contains("COMPONENT MODEL NOT AVAILABLE"),
        "a failed component picture has an explicit first visible fallback: {:?}",
        screen.detail_text()
    );
}

#[test]
fn component_icons_resize_and_centre_for_their_shipped_footprints() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    screen.load_visuals(&mut Loader::new(&mut vfs, &mut assets));
    let mut h = Harness::with_assets(assets);

    let mut rows: Vec<_> = (0..data.catalog.len()).collect();
    rows.sort_by_cached_key(|index| {
        data.catalog
            .get(*index)
            .map(|component| {
                h.assets
                    .string(component.string_id as u32)
                    .to_ascii_lowercase()
            })
            .unwrap_or_default()
    });
    let first = rows
        .iter()
        .enumerate()
        .find_map(|(row, index)| {
            let component = data.catalog.get(*index)?;
            h.assets
                .textures
                .find(&art_file_path(&component.icon_file))
                .map(|_| (row, component))
        })
        .expect("shipped component with a loaded icon");
    let second = rows
        .iter()
        .enumerate()
        .find_map(|(row, index)| {
            let component = data.catalog.get(*index)?;
            ((component.width, component.height) != (first.1.width, first.1.height)
                && h.assets
                    .textures
                    .find(&art_file_path(&component.icon_file))
                    .is_some())
            .then_some((row, component))
        })
        .expect("two shipped components with different loaded icon footprints");

    switch(&mut h, &mut screen, Category::Weapons);
    let mut rendered = Vec::new();
    for (row, component) in [first, second] {
        select_row(&mut h, &mut screen, row);
        let rect = rendered_texture_rect(&h, &mut screen, &art_file_path(&component.icon_file))
            .expect("selected component icon quad");
        assert_eq!(rect.2 - rect.0, component.width as f32 * 48.0);
        assert_eq!(rect.3 - rect.1, component.height as f32 * 32.0);
        rendered.push(rect);
    }
    assert_ne!(
        (rendered[0].2 - rendered[0].0, rendered[0].3 - rendered[0].1),
        (rendered[1].2 - rendered[1].0, rendered[1].3 - rendered[1].1),
        "different component footprints produce different icon rectangles"
    );
    assert_eq!(
        (
            (rendered[0].0 + rendered[0].2) * 0.5,
            (rendered[0].1 + rendered[0].3) * 0.5
        ),
        (
            (rendered[1].0 + rendered[1].2) * 0.5,
            (rendered[1].1 + rendered[1].3) * 0.5
        ),
        "both footprint-specific icons are centred in the same Rect1 panel"
    );
}

/// What `buildings.csv` and `object2.pak` say about the rows that are not mechs, read here directly
/// (the C++ `initVariants` conditions: `VEHICLE` rows with a scale, any other non-`MECH` row with a
/// scale): `(is_vehicle, packet FIT)`.
fn shipped_object_fits(vfs: &mut Vfs) -> Vec<(bool, mc2_formats::ini::FitIni)> {
    let table = CsvFile::from_bytes(
        vfs.read("data/art/buildings.csv")
            .expect("read buildings.csv")
            .expect("shipped buildings.csv"),
    )
    .expect("parse buildings.csv");
    let pak = mc2_formats::pak::PakFile::from_bytes(
        vfs.read("data/objects/object2.pak")
            .expect("read object2.pak")
            .expect("shipped object2.pak"),
    )
    .expect("parse object2.pak");
    let mut out = Vec::new();
    for row in 1u32.. {
        let Ok(Some(kind)) = table.read_string(row, 4, 256) else {
            break;
        };
        let scale = table.read_f32(row, 11).unwrap_or(0.0);
        if kind.eq_ignore_ascii_case("MECH") || scale == 0.0 {
            continue;
        }
        let packet = pak
            .read_packet(table.read_i64(row, 5).expect("fit id") as usize)
            .expect("packet");
        if packet.is_empty() {
            continue;
        }
        out.push((
            kind.eq_ignore_ascii_case("VEHICLE"),
            mc2_formats::ini::FitIni::from_bytes(packet).expect("object FIT"),
        ));
    }
    out
}

fn sorted_caseless(mut labels: Vec<String>) -> Vec<String> {
    labels.sort_by_key(|l| l.to_ascii_lowercase());
    labels
}

#[test]
fn the_vehicle_and_building_pages_list_the_shipped_object_records() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let fits = shipped_object_fits(&mut vfs);
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    screen.load_visuals(&mut Loader::new(&mut vfs, &mut assets));
    let mut h = Harness::with_assets(assets);

    // ---- vehicles: the pak vehicles plus the helicopters, sorted by name
    let mut expected_vehicles: Vec<(String, String, f32)> = Vec::new();
    for (is_vehicle, fit) in &fits {
        if !*is_vehicle {
            continue;
        }
        let mut fit = fit.clone();
        fit.seek_block("General").unwrap();
        let name_id = fit.read_i64("DescIndex").unwrap();
        let tonnage = fit.read_f32("CurTonnage").unwrap();
        let encyclopedia = fit.read_i64("EncyclopediaID").unwrap_or(45200);
        expected_vehicles.push((
            h.assets.string(name_id as u32).to_uppercase(),
            h.assets.string(encyclopedia as u32).to_string(),
            tonnage,
        ));
    }
    assert!(
        expected_vehicles.len() >= 5,
        "the shipped game has vehicles in object2.pak: {}",
        expected_vehicles.len()
    );
    let mut expected_labels: Vec<String> = expected_vehicles.iter().map(|v| v.0.clone()).collect();
    for key in logistics.helicopters() {
        expected_labels.push(logistics.variant(key).unwrap().name.to_uppercase());
    }
    switch(&mut h, &mut screen, Category::Vehicles);
    assert_eq!(
        sorted_caseless(screen.group_text()),
        sorted_caseless(expected_labels),
        "the vehicle list holds every pak vehicle and helicopter"
    );
    let shown = screen.group_text();
    assert!(
        shown
            .windows(2)
            .all(|w| w[0].to_ascii_lowercase() <= w[1].to_ascii_lowercase()),
        "the vehicle list is in alphabetical order: {shown:?}"
    );
    // pick a pak vehicle through the list box and read what the page shows
    let (label, description, tonnage) = &expected_vehicles[0];
    let row = shown.iter().position(|l| l == label).expect("vehicle row");
    select_row(&mut h, &mut screen, row);
    let details = screen.detail_text();
    assert!(
        details.iter().any(|l| l == description),
        "the encyclopedia text of {label} is shown: {details:?}"
    );
    assert!(
        details
            .iter()
            .any(|l| l == &format!("WEIGHT : {tonnage:.0} tons")),
        "{label} shows its tonnage {tonnage}: {details:?}"
    );

    // ---- buildings: every listed building, sorted, with its damage points
    let mut expected_buildings: Vec<(String, u32)> = Vec::new();
    for (is_vehicle, fit) in &fits {
        if *is_vehicle {
            continue;
        }
        let mut fit = fit.clone();
        fit.seek_block("ObjectType").unwrap();
        let blocks = ["BuildingData", "GateData", "TurretData", "General"];
        let Some(block) = blocks.iter().find(|b| fit.seek_block(b).is_ok()) else {
            continue;
        };
        fit.seek_block(block).unwrap();
        let name_id = fit.read_i64("BuildingName").unwrap();
        let damage = fit.read_u32("DmgLevel").unwrap();
        expected_buildings.push((
            h.assets
                .string(name_id as u32)
                .to_uppercase()
                .replace("LIAO ", ""),
            damage,
        ));
    }
    assert!(
        expected_buildings.len() >= 5,
        "the shipped game has buildings in object2.pak: {}",
        expected_buildings.len()
    );
    switch(&mut h, &mut screen, Category::Buildings);
    assert_eq!(
        sorted_caseless(screen.group_text()),
        sorted_caseless(expected_buildings.iter().map(|b| b.0.clone()).collect()),
        "the building list holds every pak building"
    );
    let shown = screen.group_text();
    let (label, damage) = &expected_buildings[0];
    let row = shown.iter().position(|l| l == label).expect("building row");
    select_row(&mut h, &mut screen, row);
    let details = screen.detail_text();
    // the first building with that name answers (`getBuilding(nameID)`)
    let first_damage = expected_buildings
        .iter()
        .find(|b| &b.0 == label)
        .map(|b| b.1)
        .unwrap_or(*damage);
    assert!(
        details
            .iter()
            .any(|l| l == &format!("Structure Damage Points : {first_damage}")),
        "{label} shows its damage points: {details:?}"
    );
}

#[test]
fn unavailable_preview_is_the_first_visible_detail_line_and_components_show_no_placeholder() {
    let _guard = data_lock();
    let Some(mut vfs) = open_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("shipped campaign data"));
    let logistics = Logistics::new(Arc::clone(&data)).expect("logistics");
    let mut assets = UiAssets::default();
    let mut screen =
        Mechlopedia::load(&mut Loader::new(&mut vfs, &mut assets)).expect("Mechlopedia FITs");
    screen.begin(&assets, &data, &logistics);
    let mut h = Harness::with_assets(assets);

    // This is the state the production UI host sets after its offscreen preview construction
    // fails. It is deliberately the first detail row, so no scroll is required to read it.
    screen.set_preview_available(false, &h.assets);
    assert!(
        screen.detail_text()[0].contains("MECH MODEL NOT AVAILABLE"),
        "the preview-renderer fallback is on screen without scrolling"
    );

    switch(&mut h, &mut screen, Category::Weapons);
    let picture_row = screen
        .group_text()
        .iter()
        .enumerate()
        .find_map(|(row, _)| {
            let mut components: Vec<_> = (0..data.catalog.len()).collect();
            components.sort_by_cached_key(|i| {
                data.catalog
                    .get(*i)
                    .map(|c| h.assets.string(c.string_id as u32).to_ascii_lowercase())
                    .unwrap_or_default()
            });
            data.catalog
                .get(components[row])
                .and_then(|c| c.picture_file.as_ref())
                .map(|_| row)
        })
        .expect("shipped component with a model picture");
    select_row(&mut h, &mut screen, picture_row);
    assert!(
        !screen
            .detail_text()
            .iter()
            .any(|line| line.contains("NOT YET AVAILABLE") || line.contains("UNPORTED")),
        "a component with a picture shows no placeholder: {:?}",
        screen.detail_text()
    );
    assert!(
        screen.detail_text()[0].contains("COMPONENT MODEL NOT AVAILABLE"),
        "an unavailable renderer is named for a component with a picture: {:?}",
        screen.detail_text()
    );
}

#[test]
fn category_button_ids_match_the_shipped_layout() {
    assert_eq!(Mechlopedia::button_id(Category::Mechs), 130);
    assert_eq!(Mechlopedia::button_id(Category::Buildings), 131);
    assert_eq!(Mechlopedia::button_id(Category::Vehicles), 132);
    assert_eq!(Mechlopedia::button_id(Category::Weapons), 133);
    assert_eq!(Mechlopedia::button_id(Category::Personalities), 134);
    assert_eq!(Mechlopedia::button_id(Category::History), 135);
}

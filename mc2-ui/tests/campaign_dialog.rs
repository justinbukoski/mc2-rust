//! The New Campaign list (`LogisticsSaveDialog::beginCampaign`, `mcl_sm.fit`) against the shipped
//! data: its art, the name edit box, the mission info, and the slide-in animation. Needs
//! `MC2_DATA`; the PNG needs a GPU (`MC2_SKIP_GPU_TESTS=1` skips only that).

use std::sync::Arc;

use mc2_campaign::GameData;
use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_platform::keys::KEY_ESCAPE;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::harness::{events, Harness};
use mc2_ui::logistics_dialog::{CampaignDialog, CampaignRow};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screen::Status;
use mc2_ui::{Loader, UiAssets};

fn first_vertex_x(dl: &DrawList) -> f32 {
    dl.vertices.iter().map(|v| v.x).fold(f32::MAX, f32::min)
}

#[test]
fn the_new_campaign_dialog_slides_in_and_shows_its_art_edit_box_and_mission_info() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(&root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("data"));
    let mut assets = UiAssets::default();
    let rows = CampaignRow::from_data(&data);
    assert_eq!(rows.len(), 2, "campaign and tutorial");
    let mut dialog = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut d = CampaignDialog::load(&mut ld).expect("mcl_sm");
        d.begin(&mut ld, rows.clone());
        d
    };
    let mut h = Harness::with_assets(assets);

    // the slide: the panel is somewhere else on the first frames than once it has entered
    h.idle(1, |ctx| {
        dialog.update(ctx);
    });
    let early = h.render(|ctx| dialog.render(ctx));
    let early_x = (
        first_vertex_x(&early),
        dialog.anim.frame().1,
        dialog.anim.frame().2,
    );
    h.idle(60, |ctx| {
        dialog.update(ctx);
    });
    assert!(dialog.anim.entered());
    let settled = h.render(|ctx| dialog.render(ctx));
    let settled_frame = dialog.anim.frame();
    assert_eq!(
        (settled_frame.1, settled_frame.2),
        (0.0, 0.0),
        "settled panel"
    );
    assert!(
        early_x.1 != 0.0 || early_x.2 != 0.0,
        "the enter animation offsets the panel on its first frame: {early_x:?}"
    );

    // art: the layout's statics are drawn, the mission map is drawn, text and edit are present
    assert!(settled.quad_count() > 10, "dialog art quads");
    let names: Vec<String> = settled
        .batches
        .iter()
        .filter_map(|b| b.texture)
        .filter_map(|t| h.assets.textures.get(t))
        .map(|i| i.name.clone())
        .collect();
    assert!(
        names.iter().any(|n| n.contains("data/art/")),
        "dialog art textures: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains(".pak#3")),
        "the first mission's map picture is drawn: {names:?}"
    );
    let edit = dialog.screen.edits[0].entry();
    eprintln!(
        "edit colour {:08x} highlight {:08x} rows {rows:?}",
        dialog.screen.edits[0].text_color, dialog.screen.edits[0].highlight_color
    );
    assert_eq!(
        dialog.list.item_count(),
        2,
        "both campaigns are listed: {rows:?}"
    );
    assert_eq!(edit, rows[0].name, "the edit box holds the first campaign");
    assert_eq!(dialog.selected().map(|r| r.stem.as_str()), Some("campaign"));
    let info = dialog.screen.texts[2].text.clone();
    assert!(
        !info.is_empty(),
        "mission info text for {}",
        rows[0].first_mission
    );

    // the PNG
    match Gpu::headless(&RenderConfig::from_env()) {
        Ok(gpu) => {
            let mut renderer = UiRenderer::new(&gpu);
            let target = Offscreen::new(&gpu, 800, 600);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            target.clear(&mut enc, [0.1, 0.1, 0.12, 1.0]);
            let layout = UiLayout::new((800, 600), (800, 600), ScaleMode::Fit);
            renderer.draw(
                &gpu,
                &mut enc,
                &target.target(),
                &layout,
                &settled,
                &h.assets,
            );
            let px = target.read_rgba(&gpu, enc).expect("readback");
            let dir = visual_dir();
            std::fs::create_dir_all(&dir).expect("docs/visual");
            mc2_render::gpu::write_png(&dir.join("new-campaign-dialog.png"), &px, 800, 600)
                .expect("PNG");
        }
        Err(e)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1")
                && e.to_string().contains("adapter") =>
        {
            eprintln!("SKIPPED PNG: no GPU ({e})");
        }
        Err(e) => panic!("GPU: {e}"),
    }

    // Escape leaves with No; the panel then slides out while the caller has the answer
    let press = |h: &mut Harness, dialog: &mut CampaignDialog| {
        let mut answer = None;
        for pressed in [true, false] {
            let ev = events::key(KEY_ESCAPE, pressed, None);
            h.frame(std::slice::from_ref(&ev), |ctx| {
                if let Some(s) = dialog.update(ctx) {
                    answer = Some(s);
                }
            });
        }
        answer
    };
    assert_eq!(press(&mut h, &mut dialog), Some(Status::No));
    assert!(!dialog.is_open());
    assert!(dialog.anim.leaving(), "the leave animation plays");
    let leaving = h.render(|ctx| dialog.render(ctx));
    assert!(
        leaving.quad_count() > 0,
        "the panel is still drawn while it leaves"
    );
    h.idle(90, |ctx| {
        dialog.update(ctx);
    });
    let gone = h.render(|ctx| dialog.render(ctx));
    assert_eq!(gone.quad_count(), 0, "nothing is drawn once it has left");

    // reopened, the Load button confirms the selected campaign
    {
        let mut ld = Loader::new(&mut vfs, &mut h.assets);
        dialog.begin(&mut ld, rows.clone());
    }
    h.idle(60, |ctx| {
        dialog.update(ctx);
    });
    let yes = dialog.screen.button(7).expect("Load button");
    let at = (
        yes.global_x() as f32 + yes.width() / 2.0,
        yes.global_y() as f32 + yes.height() / 2.0,
    );
    let mut answer = None;
    for ev in [
        events::move_to(at.0, at.1),
        events::left(true),
        events::left(false),
    ] {
        h.frame(std::slice::from_ref(&ev), |ctx| {
            if let Some(s) = dialog.update(ctx) {
                answer = Some(s);
            }
        });
    }
    assert_eq!(answer, Some(Status::Yes));
    assert_eq!(dialog.chosen, "campaign");
}

/// PNG output dir: `MC2_VISUAL_DIR` (e.g. rust/docs/visual) or the cargo test tmp dir, so a plain
/// `cargo test` never writes into the source tree.
fn visual_dir() -> std::path::PathBuf {
    std::env::var_os("MC2_VISUAL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("visual"))
}

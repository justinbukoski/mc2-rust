//! Dev helper: renders the logistics screens (mech lab, mech purchase, mission results) headless
//! to PNG, each in the campaign state it shows during play. Never opens a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_lab_purchase_results -- <out-dir> [w h ...]`
//! writes `<out-dir>/<screen>-<w>x<h>.png` for each size given (default 1024x768 and 3840x2160).

use std::path::Path;
use std::sync::Arc;

use mc2_campaign::interface::{MechSummary, MissionResult, MoverReport, Outcome};
use mc2_campaign::pilot::PilotReport;
use mc2_campaign::{GameData, Logistics};
use mc2_gfx::config::RenderConfig;
use mc2_platform::input::InputEvent;
use mc2_render::gpu::{write_png, Gpu};
use mc2_ui::draw::DrawList;
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::mech_lab::MechLab;
use mc2_ui::mech_purchase::MechPurchase;
use mc2_ui::mission_results::{MissionResults, SALVAGE_DONE};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::widgets::AnimButton;
use mc2_ui::{Loader, RenderCtx};

/// A press and release on a point, one frame each, after the double-click clock has run out.
/// `step` runs one frame of the screen with the given events.
fn click(h: &mut Harness, (x, y): (f32, f32), mut step: impl FnMut(&mut Harness, &[InputEvent])) {
    for _ in 0..8 {
        step(h, &[]);
    }
    step(h, &[move_to(x, y)]);
    step(h, &[left(true)]);
    step(h, &[left(false)]);
    step(h, &[]);
}

fn centre(b: &AnimButton) -> (f32, f32) {
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

/// Renders a draw list into a `w x h` PNG at `path`.
fn save(gpu: &Gpu, h: &Harness, dl: &DrawList, (w, hgt): (u32, u32), path: &Path) {
    let mut renderer = UiRenderer::new(gpu);
    let target = Offscreen::new(gpu, w, hgt);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((800, 600), (w, hgt), ScaleMode::Fit);
    renderer.draw(gpu, &mut enc, &target.target(), &layout, dl, &h.assets);
    let px = target.read_rgba(gpu, enc).expect("readback");
    write_png(path, &px, w, hgt).expect("png");
    println!("{} quads -> {}", dl.quad_count(), path.display());
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = Path::new(
        a.get(1)
            .expect("usage: render_lab_purchase_results <out-dir> [w h ...]"),
    );
    let mut sizes: Vec<(u32, u32)> = a
        .get(2..)
        .unwrap_or(&[])
        .chunks(2)
        .filter_map(|p| Some((p.first()?.parse().ok()?, p.get(1)?.parse().ok()?)))
        .collect();
    if sizes.is_empty() {
        sizes = vec![(1024, 768), (3840, 2160)];
    }
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("gpu");

    let mut logistics = Logistics::with_campaign(data, "campaign").expect("campaign");
    logistics.add_c_bills(10_000_000);
    let mut h = Harness::new();

    // ---- mech lab: the first design for sale, bought and opened for editing
    let design = logistics.purchasable_mechs()[0];
    let mech = logistics.purchase_mech(design).expect("a mech to edit");
    logistics.set_mech_to_modify(mech).expect("lab");
    let mut lab = MechLab::load(&mut Loader::new(&mut vfs, &mut h.assets)).expect("mech lab");
    h.frame(&[], |ctx| lab.begin(ctx, &logistics));
    // one update so the panel's numbers are the campaign's
    h.frame(&[], |ctx| lab.update(ctx, &mut logistics));
    for &size in &sizes {
        let dl = h.render(|ctx: &mut RenderCtx<'_>| lab.render(ctx, &logistics));
        save(
            &gpu,
            &h,
            &dl,
            size,
            &dir.join(format!("mech-lab-{}x{}.png", size.0, size.1)),
        );
    }
    logistics.cancel_mech_modifications().expect("cancel");

    // ---- mech purchase: an owned mech and the mechs for sale
    let owned = logistics.purchasable_mechs()[0];
    logistics.purchase_mech(owned).expect("an owned mech");
    let mut purchase =
        MechPurchase::load(&mut Loader::new(&mut vfs, &mut h.assets)).expect("purchase");
    h.frame(&[], |ctx| purchase.begin(ctx, &logistics));
    h.frame(&[], |ctx| purchase.update(ctx, &mut logistics));
    for &size in &sizes {
        let dl = h.render(|ctx: &mut RenderCtx<'_>| purchase.render(ctx));
        save(
            &gpu,
            &h,
            &dl,
            size,
            &dir.join(format!("mech-purchase-{}x{}.png", size.0, size.1)),
        );
    }

    // ---- mission results: the campaign's second mission, with a salvage and a promotion
    let pilot = logistics
        .pilot_index("pmwdagger")
        .expect("the pilot is in the roster");
    let wrecked = logistics.purchasable_mechs()[1];
    let name = |l: &Logistics, v: u32| l.variant(v).expect("variant").name.clone();
    let (carried_name, wrecked_name) = (name(&logistics, owned), name(&logistics, wrecked));
    logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let force = logistics.purchase_mech(owned).expect("purchase");
    logistics.set_mech_pilot(force, Some(pilot));
    logistics
        .add_mech_to_force_group(force, 1)
        .expect("force group");
    let won = Outcome {
        result: MissionResult::PlayerWinBig,
        movers: vec![MoverReport {
            on_gui: true,
            is_battle_mech: true,
            home_commander: true,
            is_air: false,
            destroyed: false,
            disabled: false,
            variant_name: carried_name,
            pilot_file: Some("pmwdagger".into()),
            paint_scheme: [0; 3],
            pilot: PilotReport {
                gunnery_rank: 50.0,
                piloting_rank: 48.0,
                gunnery_points: 100.0,
                piloting_points: 400.0,
                ..PilotReport::default()
            },
        }],
        mechs: vec![MechSummary::wreck(&wrecked_name, 50.0)],
        ..Outcome::won()
    };
    logistics.apply_outcome(&won).expect("apply_outcome");
    let mut results =
        MissionResults::load(&mut Loader::new(&mut vfs, &mut h.assets)).expect("results");
    h.frame(&[], |ctx| results.begin(ctx, &mut logistics, &won));
    for &size in &sizes {
        let dl = h.render(|ctx: &mut RenderCtx<'_>| results.render(ctx));
        save(
            &gpu,
            &h,
            &dl,
            size,
            &dir.join(format!("mission-results-salvage-{}x{}.png", size.0, size.1)),
        );
    }
    let done = centre(results.salvage.screen.button(SALVAGE_DONE).expect("Done"));
    click(&mut h, done, |h, ev| {
        h.frame(ev, |ctx| results.update(ctx, &mut logistics))
    });
    for &size in &sizes {
        let dl = h.render(|ctx: &mut RenderCtx<'_>| results.render(ctx));
        save(
            &gpu,
            &h,
            &dl,
            size,
            &dir.join(format!("mission-results-review-{}x{}.png", size.0, size.1)),
        );
    }
}

//! Dev helper: renders the in-mission objectives panel for mc2_01 to an offscreen PNG. Never
//! opens a window.
//!
//! The objectives are mc2_01's team 0 list (`data/missions/mc2_01.fit`), with the resource string
//! ids and the flags the mission file gives. The statuses are synthetic: the first objective
//! successful, the second failed, the rest undetermined. The panel uses the 1024x768 button
//! layout, `buttonlayout1024.fit`.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_objectives -- out.png`

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_sim_data::mission::Mission;
use mc2_ui::harness::Harness;
use mc2_ui::objectives::{
    ObjectiveStatus, ObjectiveText, ObjectiveView, ObjectivesPanel, ObjectivesView,
};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, UiAssets};

const W: u32 = 1024;
const H: u32 = 768;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "objectives.png".to_string());
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let mission = Mission::load_named(&mut vfs, "data/missions/mc2_01.fit").expect("mc2_01");
    let team = mission
        .teams
        .iter()
        .find(|t| t.team == 0)
        .expect("team 0 objectives");

    // The sim's objectives, as the panel reads them. Statuses are synthetic.
    let mut objectives = Vec::new();
    for (i, o) in team.objectives.iter().enumerate() {
        let text = |use_res: bool, id: u32, literal: &str| {
            if use_res {
                ObjectiveText::Resource(id)
            } else {
                ObjectiveText::Literal(literal.to_string())
            }
        };
        let status = match i {
            0 => ObjectiveStatus::Successful,
            1 => ObjectiveStatus::Failed,
            _ => ObjectiveStatus::Undetermined,
        };
        // the flag-activated objectives count as activated at 0 s, so their 20 s flash is over
        objectives.push(ObjectiveView {
            title: text(
                o.title_use_resource_string,
                o.title_resource_string_id,
                &o.title,
            ),
            description: text(
                o.description_use_resource_string,
                o.description_resource_string_id,
                &o.description,
            ),
            status,
            priority: o.priority,
            resource_points: o.resource_points,
            is_active: true,
            is_hidden_trigger: o.is_hidden_trigger,
            activate_on_flag: o.activate_on_flag,
            activation_time: 0.0,
        });
    }
    let view = ObjectivesView {
        objectives,
        mission_time: 60.0,
        render_status_info: false,
        results_time: 0.0,
        player_status: None,
    };

    let mut assets = UiAssets {
        screen: (W as i32, H as i32),
        ..UiAssets::default()
    };
    let mut panel = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut fit = ld
            .art_fit("buttonlayout1024")
            .expect("buttonlayout1024.fit");
        ObjectivesPanel::from_fit(&mut ld, &mut fit).expect("objectives block")
    };
    panel.start(true, false);

    let mut h = Harness::with_assets(assets);
    h.dt = 1.0 / 30.0;
    h.frame(&[], |ctx| panel.update(ctx, &view));
    let dl = h.render(|ctx| panel.render(ctx, &view).expect("single-player panel"));

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("gpu");
    let mut r = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, W, H);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((W, H), (W, H), ScaleMode::Fit);
    r.draw(&gpu, &mut enc, &target.target(), &layout, &dl, &h.assets);
    let px = target.read_rgba(&gpu, enc).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(&out), &px, W, H).expect("png");
    println!(
        "{} quads, {} batches -> {out}",
        dl.quad_count(),
        dl.batches.len()
    );
}

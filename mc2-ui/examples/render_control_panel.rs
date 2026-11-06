//! Headless sample of the in-mission orders panel at 1024x768.
//!
//! Run with `MC2_DATA=... cargo run -p mc2-ui --example render_control_panel -- out.png`.

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::control::{ControlPanel, ControlPanelInput, UnitCapabilities, VehicleAvailability};
use mc2_ui::draw::DrawList;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, RenderCtx, UiAssets};

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rust/mc2-ui/docs/control-panel-1024x768.png".to_string());
    let width = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1024);
    let height = 768;
    let root = std::env::var("MC2_DATA").expect("MC2_DATA must name the game data directory");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("game VFS");
    let mut assets = UiAssets {
        screen: (width as i32, height),
        ..UiAssets::default()
    };
    let mut panel = {
        let mut loader = Loader::new(&mut vfs, &mut assets);
        ControlPanel::load(&mut loader, width, height as u32).expect("button layout FIT")
    };
    let selected = [UnitCapabilities {
        can_jump: true,
        can_lay_mines: false,
        can_salvage: false,
        can_repair: false,
        attack_range: Some(mc2_ui::control::FireRange::Long),
        has_queued_orders: true,
        ..UnitCapabilities::default()
    }];
    panel.update(ControlPanelInput {
        selected_units: &selected,
        mouse: (980, 625),
        paused: false,
        vtol_in_flight: false,
        resource_points: 18750,
        vehicle_availability: VehicleAvailability {
            can_have_airstrike: true,
            can_have_guard_tower: true,
            can_have_sensor_probe: true,
            can_have_repair_truck: true,
            can_have_scout: true,
            can_have_minelayer: true,
            can_have_recovery_team: true,
            unit_capacity_available: true,
            pilots_remaining: true,
            player_destroyed: false,
        },
        click: Some(mc2_ui::control::ControlId::VehicleTab),
        vehicle_click: None,
    });
    panel.update(ControlPanelInput {
        selected_units: &selected,
        mouse: (980, 625),
        paused: false,
        vtol_in_flight: false,
        resource_points: 18750,
        vehicle_availability: VehicleAvailability {
            can_have_airstrike: true,
            can_have_guard_tower: true,
            can_have_sensor_probe: true,
            can_have_repair_truck: true,
            can_have_scout: true,
            can_have_minelayer: true,
            can_have_recovery_team: true,
            unit_capacity_available: true,
            pilots_remaining: true,
            player_destroyed: false,
        },
        click: None,
        vehicle_click: None,
    });
    let mut dl = DrawList::new(width as i32, height);
    let mut ctx = RenderCtx {
        dl: &mut dl,
        assets: &assets,
        dt: 0.0,
    };
    panel.render(&mut ctx);

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height as u32);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.02, 0.035, 0.045, 1.0]);
    let layout = UiLayout::new(
        (width, height as u32),
        (width, height as u32),
        ScaleMode::Fit,
    );
    renderer.draw(&gpu, &mut encoder, &target.target(), &layout, &dl, &assets);
    let rgba = target.read_rgba(&gpu, encoder).expect("offscreen readback");
    let path = std::path::Path::new(&output);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("output directory");
    }
    mc2_render::gpu::write_png(path, &rgba, width, height as u32).expect("PNG write");
    println!("{} quads -> {}", dl.quad_count(), path.display());
}

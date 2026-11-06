//! Dev helper: renders the logistics screens of a campaign started at mission 1 to offscreen PNGs
//! (`<out_dir>/<screen>_<width>x<height>.png`). Never opens a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_logistics -- <out_dir> [screen ...]`
//! Screens: `select`, `briefing`, `mechbay`, `pilot` (default: all four).

use std::path::Path;
use std::sync::Arc;

use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::input::UiInput;
use mc2_ui::logistics::briefing::Briefing;
use mc2_ui::logistics::mech_bay::MechBay;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::pilot_ready::PilotReady;
use mc2_ui::logistics::Page;
use mc2_ui::preview::ModelPreviews;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, RenderCtx, UiAssets, UiOutput, UpdateCtx};

/// One frame of update (the first frame a screen shows), then its draw list. A model picture the
/// screen asks for on its first draw is rendered and drawn in the second.
fn settle(
    page: &mut dyn Page,
    ld: &mut Loader<'_>,
    lg: &mut Logistics,
    previews: &mut ModelPreviews,
) -> DrawList {
    let dt = 1.0 / 30.0;
    let mut input = UiInput::new(800, 600);
    input.begin_frame(dt);
    let mut out = UiOutput::default();
    {
        let assets: &UiAssets = &*ld.assets;
        let mut ctx = UpdateCtx {
            input: &mut input,
            assets,
            dt,
            out: &mut out,
        };
        page.update(&mut ctx, lg);
    }
    input.end_frame();
    let draw = |page: &mut dyn Page, ld: &Loader<'_>| {
        let mut dl = DrawList::new(800, 600);
        let mut ctx = RenderCtx {
            dl: &mut dl,
            assets: &*ld.assets,
            dt: 0.0,
        };
        page.render(&mut ctx);
        dl
    };
    draw(page, ld);
    previews.resolve(ld.assets);
    draw(page, ld)
}

/// The screen as a small PNG for the committed evidence: opaque (the alpha channel is dropped), and
/// quantised to a fixed 256-colour palette (8 red, 8 green, 4 blue levels) and written indexed at
/// the best deflate level. The screens draw more colours than that, so the previews are approximate;
/// no test compares pixels.
fn write_png_small(path: &Path, rgba: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    const R: u32 = 8;
    const G: u32 = 8;
    const B: u32 = 4;
    let level = |v: u8, n: u32| (u32::from(v) * (n - 1) + 127) / 255;
    let scale = |l: u32, n: u32| (l * 255 / (n - 1)) as u8;
    let palette: Vec<u8> = (0..R * G * B)
        .flat_map(|i| {
            let (r, g, b) = (i / (G * B), (i / B) % G, i % B);
            [scale(r, R), scale(g, G), scale(b, B)]
        })
        .collect();
    let (pixels, _) = rgba.as_chunks::<4>();
    let indexed: Vec<u8> = pixels
        .iter()
        .map(|p| {
            let (r, g, b) = (level(p[0], R), level(p[1], G), level(p[2], B));
            (r * G * B + g * B + b) as u8
        })
        .collect();
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_palette(palette);
    enc.set_compression(png::Compression::High);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(&indexed).map_err(std::io::Error::other)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = Path::new(args.first().map_or("target/logistics", String::as_str));
    std::fs::create_dir_all(out_dir).expect("out dir");
    let wanted: Vec<&str> = if args.len() > 1 {
        args[1..].iter().map(String::as_str).collect()
    } else {
        vec!["select", "briefing", "mechbay", "pilot"]
    };

    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(&root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("game data"));
    let mut lg = Logistics::with_campaign(Arc::clone(&data), "campaign").expect("campaign");
    let mut assets = UiAssets::default();
    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("gpu");
    let mut renderer = UiRenderer::new(&gpu);
    let mut previews = ModelPreviews::new(
        Gpu::headless(&RenderConfig::from_env()).expect("preview gpu"),
        mc2_render::game::GameData::open(&root).expect("render data"),
    )
    .expect("previews");

    for name in wanted {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let dl = match name {
            "select" => {
                let mut page = MissionSelect::load(&mut ld).expect("selection");
                Page::begin(&mut page, &mut ld, &mut lg);
                settle(&mut page, &mut ld, &mut lg, &mut previews)
            }
            "briefing" => {
                let mut page = Briefing::load(&mut ld).expect("briefing");
                Page::begin(&mut page, &mut ld, &mut lg);
                settle(&mut page, &mut ld, &mut lg, &mut previews)
            }
            "mechbay" => {
                // two of the cheapest mech in storage, one of them in the force group
                let mut variants = lg.purchasable_mechs();
                variants.sort_by_key(|v| lg.variant_cost(*v));
                lg.add_c_bills(200_000);
                for _ in 0..2 {
                    lg.purchase_mech(variants[0]).expect("purchase");
                }
                let first = lg.inventory()[0].key;
                lg.add_mech_to_force_group(first, 1).expect("force group");
                let mut page = MechBay::load(&mut ld).expect("mech bay");
                Page::begin(&mut page, &mut ld, &mut lg);
                settle(&mut page, &mut ld, &mut lg, &mut previews)
            }
            "pilot" => {
                // the mech bay's setup, then the pilot room with the first mech in slot 1
                let mut variants = lg.purchasable_mechs();
                variants.sort_by_key(|v| lg.variant_cost(*v));
                lg.add_c_bills(200_000);
                for _ in 0..2 {
                    lg.purchase_mech(variants[0]).expect("purchase");
                }
                let first = lg.inventory()[0].key;
                lg.add_mech_to_force_group(first, 1).expect("force group");
                let mut page = PilotReady::load(&mut ld).expect("pilot room");
                Page::begin(&mut page, &mut ld, &mut lg);
                // the player picks the slot's mech: its statistics and model show in the room
                page.set_mech(&*ld.assets, &lg, Some(first));
                settle(&mut page, &mut ld, &mut lg, &mut previews)
            }
            other => {
                eprintln!("no screen named {other}");
                continue;
            }
        };
        for (w, h) in [(1024u32, 768u32), (3840, 2160)] {
            let target = Offscreen::new(&gpu, w, h);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
            let layout = UiLayout::new((800, 600), (w, h), ScaleMode::Fit);
            renderer.draw(&gpu, &mut enc, &target.target(), &layout, &dl, &assets);
            let px = target.read_rgba(&gpu, enc).expect("readback");
            let out = out_dir.join(format!("{name}_{w}x{h}.png"));
            write_png_small(&out, &px, w, h).expect("png");
            println!("{} quads -> {}", dl.quad_count(), out.display());
        }
    }
}

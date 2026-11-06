//! The mech lab's pictures on the shipped data: the spinning component model in the `rects[4]`
//! panel, the attribute meters and the heat count-up, the component icon art, and the ammo row
//! under `useUnlimitedAmmo`; and the options gameplay tab's spinning 'Mech. The GPU tests honour
//! `MC2_SKIP_GPU_TESTS=1` and write PNGs to `$CARGO_TARGET_TMPDIR/visual` (never committed).
//! Skips cleanly without `MC2_DATA`.

use std::path::PathBuf;
use std::sync::Arc;

use mc2_campaign::{GameData, Logistics, MechKey};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::logistics_ids::{button, form};
use mc2_ui::mech_lab::{CountUp, MechLab, ATTRIBUTE_METER_COUNT};
use mc2_ui::preview::{ModelPreviews, SPIN_DEGREES_PER_SECOND};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screens::{OptionsEnv, OptionsScreen, Prefs};
use mc2_ui::widgets::AnimButton;
use mc2_ui::{Loader, UiAssets};

struct Setup {
    h: Harness,
    vfs: Vfs,
    logistics: Logistics,
    root: std::ffi::OsString,
}

fn setup() -> Option<Setup> {
    let root = std::env::var_os("MC2_DATA")?;
    let mut vfs = Vfs::from_system_cfg(&root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let mut logistics = Logistics::with_campaign(data, "campaign").expect("campaign");
    logistics.add_c_bills(10_000_000);
    Some(Setup {
        h: Harness::new(),
        vfs,
        logistics,
        root,
    })
}

fn gpu() -> Option<Gpu> {
    match Gpu::headless(&mc2_gfx::config::RenderConfig::from_env()) {
        Ok(gpu) => Some(gpu),
        Err(e)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1")
                && e.to_string().contains("adapter") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("gpu: {e}"),
    }
}

fn previews(root: &std::ffi::OsStr) -> Option<ModelPreviews> {
    let gpu = gpu()?;
    let data = mc2_render::game::GameData::open(root).expect("render data");
    Some(ModelPreviews::new(gpu, data).expect("previews"))
}

fn visual_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&dir).expect("visual dir");
    dir
}

fn write_png(name: &str, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(visual_dir().join(name)).expect("png file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}

/// Draws a draw list at the UI's own size to RGBA8.
struct Screen {
    gpu: Gpu,
    renderer: UiRenderer,
    target: Offscreen,
}

impl Screen {
    fn new(gpu: Gpu) -> Screen {
        let renderer = UiRenderer::new(&gpu);
        let target = Offscreen::new(&gpu, 800, 600);
        Screen {
            gpu,
            renderer,
            target,
        }
    }

    fn draw(&mut self, assets: &UiAssets, dl: &DrawList) -> Vec<u8> {
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        self.target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
        let layout = UiLayout::new((800, 600), (800, 600), ScaleMode::Fit);
        self.renderer.draw(
            &self.gpu,
            &mut enc,
            &self.target.target(),
            &layout,
            dl,
            assets,
        );
        self.target.read_rgba(&self.gpu, enc).expect("readback")
    }
}

fn crop(img: &[u8], (l, t, r, b): (u32, u32, u32, u32)) -> Vec<u8> {
    let mut out = Vec::new();
    for y in t..b {
        let o = ((y * 800 + l) * 4) as usize;
        out.extend_from_slice(&img[o..o + ((r - l) * 4) as usize]);
    }
    out
}

/// How many pixels of two equally sized crops differ by more than a little.
fn differing(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 12))
        .count()
}

fn frame(s: &mut Setup, lab: &mut MechLab, events: &[InputEvent]) {
    s.h.frame(events, |ctx| lab.update(ctx, &mut s.logistics));
}

fn centre(b: &AnimButton) -> (f32, f32) {
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

fn click_at(s: &mut Setup, lab: &mut MechLab, (x, y): (f32, f32)) {
    for _ in 0..8 {
        frame(s, lab, &[]);
    }
    frame(s, lab, &[move_to(x, y)]);
    frame(s, lab, &[left(true)]);
    frame(s, lab, &[left(false)]);
    frame(s, lab, &[]);
}

fn press(s: &mut Setup, lab: &mut MechLab, id: i32) {
    let at = centre(lab.screen.button(id).expect("button"));
    click_at(s, lab, at);
}

/// A purchased mech with a lab open on it, its icons loaded.
fn open_lab(s: &mut Setup) -> (MechLab, MechKey) {
    let variant = s
        .logistics
        .purchasable_mechs()
        .into_iter()
        .find(|&v| s.logistics.purchase_mech(v).is_ok())
        .expect("a purchasable mech");
    let mech = s
        .logistics
        .inventory()
        .iter()
        .map(|m| m.key)
        .find(|&k| s.logistics.mech(k).is_some_and(|m| m.variant == variant))
        .expect("the purchased mech");
    s.logistics.set_mech_to_modify(mech).expect("modify");
    let mut ld = Loader::new(&mut s.vfs, &mut s.h.assets);
    let mut lab = MechLab::load(&mut ld).expect("mech lab");
    lab.load_art(&mut ld, &s.logistics);
    s.h.frame(&[], |ctx| lab.begin(ctx, &s.logistics));
    (lab, mech)
}

fn text(lab: &MechLab, slot: usize) -> String {
    lab.screen.texts[slot].text.clone()
}

/// The panel of the spinning model on the layout (`rects[4]`) as `(l, t, r, b)` whole pixels.
fn camera_rect(lab: &MechLab) -> (u32, u32, u32, u32) {
    let r = lab.screen.rect(4).expect("rects[4]");
    (
        r.obj.left() as u32,
        r.obj.top() as u32,
        r.obj.right() as u32,
        r.obj.bottom() as u32,
    )
}

#[test]
fn the_lab_draws_a_component_model_that_turns_and_writes_pngs() {
    let Some(mut s) = setup() else { return };
    let Some(mut previews) = previews(&s.root) else {
        return;
    };
    let gpu = gpu().expect("gpu");
    let mut screen = Screen::new(gpu);
    let (mut lab, _) = open_lab(&mut s);
    frame(&mut s, &mut lab, &[]);
    assert!(lab.model.is_some(), "the picked component has a model");
    let rect = camera_rect(&lab);

    // the same frames without a model are the baseline for the panel
    let mut crops = Vec::new();
    let mut frames = Vec::new();
    for n in 0..45 {
        frame(&mut s, &mut lab, &[]);
        let mut dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
        previews.resolve(&mut s.h.assets);
        // the picture rendered after this frame shows on the next
        let img = screen.draw(&s.h.assets, &dl);
        if n % 15 == 14 {
            write_png(&format!("mech_lab_frame{n:02}.png"), 800, 600, &img);
        }
        crops.push(crop(&img, rect));
        frames.push(lab.spin.yaw());
        dl.vertices.clear();
    }
    let mut bare = Vec::new();
    {
        let saved = lab.model.take();
        for _ in 0..3 {
            frame(&mut s, &mut lab, &[]);
            let dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
            bare = crop(&screen.draw(&s.h.assets, &dl), rect);
        }
        lab.model = saved;
    }
    let area = crops[0].len() / 4;
    let drawn = differing(crops.last().expect("a frame"), &bare);
    assert!(
        drawn > area / 50,
        "the model is drawn in its panel: {drawn} of {area} pixels differ from the bare panel"
    );
    // a quarter turn a second: the frames a second apart show the model at a different angle
    let turned = differing(&crops[14], &crops[44]);
    assert!(
        turned > area / 100,
        "the model turns between frames: {turned} of {area} pixels differ (yaw {} then {})",
        frames[14],
        frames[44]
    );
    assert!(
        (frames[44] - frames[14]).rem_euclid(360.0) > 0.9 * SPIN_DEGREES_PER_SECOND,
        "90 degrees a second"
    );
}

#[test]
fn the_diagram_draws_icon_art_not_flat_blocks() {
    let Some(mut s) = setup() else { return };
    let (mut lab, mech) = open_lab(&mut s);
    frame(&mut s, &mut lab, &[]);
    let dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    let v = s.logistics.mech(mech).expect("mech").variant;
    let placed = s.logistics.variant(v).expect("variant").components.len();
    assert!(placed > 0);
    // each placed component is one textured quad of its icon (plus the layout's own art)
    let textured: usize = dl
        .batches
        .iter()
        .filter(|b| b.texture.is_some())
        .map(|b| b.quad_count as usize)
        .sum();
    let icon_quads = {
        let mut n = 0;
        let vctx_catalog = &s.logistics.data().catalog;
        for p in &s.logistics.variant(v).expect("variant").components {
            let c = vctx_catalog.get(p.component).expect("component");
            let art =
                s.h.assets
                    .textures
                    .find(&mc2_ui::textures::art_file_path(&c.icon_file))
                    .expect("the icon art is loaded");
            n += dl
                .batches
                .iter()
                .filter(|b| b.texture == Some(art))
                .map(|b| b.quad_count as usize)
                .sum::<usize>()
                .min(1);
        }
        n
    };
    assert!(textured >= placed);
    assert!(
        icon_quads >= placed,
        "every placed component draws its icon texture ({icon_quads} of {placed})"
    );
}

#[test]
fn the_diagram_icons_are_pixels_of_art_in_the_png() {
    let Some(mut s) = setup() else { return };
    let Some(gpu) = gpu() else { return };
    let mut screen = Screen::new(gpu);
    let (mut lab, mech) = open_lab(&mut s);
    frame(&mut s, &mut lab, &[]);
    let dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    let img = screen.draw(&s.h.assets, &dl);
    write_png("mech_lab_icons.png", 800, 600, &img);
    let v = s.logistics.mech(mech).expect("mech").variant;
    let first = s.logistics.variant(v).expect("variant").components[0];
    let (cx, cy) = lab
        .cell_centre(&s.logistics, first.x, first.y)
        .expect("cell");
    let c = s
        .logistics
        .data()
        .catalog
        .get(first.component)
        .expect("component");
    // a flat block would be one colour: the icon over its cells has many
    let (w, h) = (c.width.max(1) as u32 * 48, c.height.max(1) as u32 * 32);
    let (l, t) = (cx as u32 - 24, cy as u32 - 16);
    let area = crop(&img, (l, t, l + w, t + h));
    let mut colours = std::collections::HashSet::new();
    for p in area.as_chunks::<4>().0 {
        colours.insert([p[0] >> 3, p[1] >> 3, p[2] >> 3]);
    }
    assert!(
        colours.len() > 12,
        "icon art has more than a block's colour: {} colours",
        colours.len()
    );
}

#[test]
fn the_component_list_rows_carry_icon_cost_and_heat() {
    use mc2_ui::component_row::ComponentRow;
    let Some(mut s) = setup() else { return };
    let (mut lab, _) = open_lab(&mut s);
    press(&mut s, &mut lab, form::WEAPON_ENERGY);
    frame(&mut s, &mut lab, &[]);
    assert!(lab.components.item_count() > 1);
    let catalog = &s.logistics.data().catalog;
    let mut seen = 0;
    for i in 0..lab.components.item_count() {
        let row = lab
            .components
            .item(i)
            .and_then(|it| it.as_any().downcast_ref::<ComponentRow>())
            .expect("a component row");
        let c = catalog.get(row.index).expect("component");
        assert!(row.has_icon(), "row {} has its icon art", row.name());
        assert_eq!(row.cost_text(), c.cost.to_string());
        assert_eq!(row.heat_text(), format!("{:.0}", c.heat));
        seen += 1;
    }
    assert!(seen > 1);
    // drawn: each row's icon is a textured quad of its icon file
    let dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    let first = lab
        .components
        .item(0)
        .and_then(|it| it.as_any().downcast_ref::<ComponentRow>())
        .expect("row")
        .index;
    let art =
        s.h.assets
            .textures
            .find(&mc2_ui::textures::art_file_path(
                &catalog.get(first).expect("component").icon_file,
            ))
            .expect("art");
    assert!(dl.batches.iter().any(|b| b.texture == Some(art)));
}

#[test]
fn the_ammo_row_follows_the_unlimited_ammo_rule() {
    let Some(mut s) = setup() else { return };
    let (mut lab, mech) = open_lab(&mut s);
    // pick the mech's own ammunition weapon in the diagram
    let v = s.logistics.mech(mech).expect("mech").variant;
    let placed = s.logistics.variant(v).expect("variant").components.clone();
    let weapon = placed
        .iter()
        .find(|p| {
            s.logistics
                .data()
                .catalog
                .get(p.component)
                .is_some_and(|c| c.ammo > 0)
        })
        .expect("a purchasable mech carries an ammunition weapon");
    let at = lab
        .cell_centre(&s.logistics, weapon.x, weapon.y)
        .expect("cell");
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);

    // the shipped `UnlimitedAmmo` is TRUE: no ammo count, label or meter
    assert!(lab.unlimited_ammo());
    assert_eq!(text(&lab, 13), "", "no ammo count under unlimited ammo");
    assert!(!lab.screen.statics[34].is_showing(), "the ammo label hides");
    assert!(!lab.meters[3].show, "the ammo meter hides");

    lab.set_unlimited_ammo(false);
    frame(&mut s, &mut lab, &[]);
    let ammo: f32 = text(&lab, 13).parse().expect("an ammo count");
    assert!(ammo > 0.0, "the weapon carries ammo");
    assert!(lab.screen.statics[34].is_showing(), "the ammo label shows");
    assert!(lab.meters[3].show, "the ammo meter shows");
    assert!((lab.meters[3].percent - ammo / 164.0).abs() < 1e-4);

    // an energy weapon has none either way, but the row is the same rule
    lab.set_unlimited_ammo(true);
    frame(&mut s, &mut lab, &[]);
    assert_eq!(text(&lab, 13), "");
    assert!(!lab.meters[3].show);
}

#[test]
fn the_heat_counts_up_across_frames_when_a_part_is_added() {
    let Some(mut s) = setup() else { return };
    let (mut lab, mech) = open_lab(&mut s);
    // make room: remove the first part, then add an energy weapon
    let v = s.logistics.mech(mech).expect("mech").variant;
    let first = s.logistics.variant(v).expect("variant").components[0];
    let at = lab
        .cell_centre(&s.logistics, first.x, first.y)
        .expect("cell");
    click_at(&mut s, &mut lab, at);
    press(&mut s, &mut lab, button::MB_MSG_REMOVE);
    for _ in 0..60 {
        frame(&mut s, &mut lab, &[]);
    }
    press(&mut s, &mut lab, form::WEAPON_ENERGY);
    let before = lab.meters[ATTRIBUTE_METER_COUNT - 4].percent;
    let before_text = text(&lab, 12);
    // Add: the heat starts counting from its old value
    let mut samples = Vec::new();
    let add = centre(lab.screen.button(button::MB_MSG_ADD).expect("add"));
    for _ in 0..8 {
        frame(&mut s, &mut lab, &[]);
    }
    frame(&mut s, &mut lab, &[move_to(add.0, add.1)]);
    frame(&mut s, &mut lab, &[left(true)]);
    frame(&mut s, &mut lab, &[left(false)]);
    for _ in 0..60 {
        frame(&mut s, &mut lab, &[]);
        samples.push((
            lab.meters[ATTRIBUTE_METER_COUNT - 4].percent,
            text(&lab, 12),
        ));
    }
    let (settled, settled_text) = samples.last().cloned().expect("frames");
    let first_text = &samples[0].1;
    assert_ne!(
        settled_text, before_text,
        "the weapon changed the heat ({before_text} -> {settled_text})"
    );
    let distinct: std::collections::BTreeSet<String> =
        samples.iter().map(|(_, t)| t.clone()).collect();
    assert!(
        distinct.len() > 2,
        "the heat text counts through values, not at once: {distinct:?}"
    );
    assert_ne!(first_text, &settled_text, "the first frame is not the end");
    let moving: std::collections::BTreeSet<u32> =
        samples.iter().map(|(p, _)| p.to_bits()).collect();
    assert!(moving.len() > 2, "the heat bar moves across frames");
    assert!(settled > before, "the settled bar is longer than before");
    // the settled figures are the campaign's
    let ctx_heat = {
        let key = lab.edited().expect("editing");
        let v = s.logistics.variant(key).expect("variant");
        let vctx = s.logistics.ctx_for(v);
        (v.heat(&vctx), v.max_heat(&vctx))
    };
    assert_eq!(settled_text, format!("{}/{}", ctx_heat.0, ctx_heat.1));
    assert!((settled - ctx_heat.0 as f32 / ctx_heat.1 as f32).abs() < 1e-4);
}

#[test]
fn count_up_runs_up_then_flashes_and_ends_on_the_real_value() {
    // `updateHeatMeter`: a part that adds heat counts from `old * t / .6`, then flashes the old bar
    let mut a = CountUp::default();
    assert!(!a.running());
    a.start(10, false);
    assert!(a.running());
    let dt = 1.0 / 30.0;
    let mut numbers = Vec::new();
    let mut meters = Vec::new();
    for _ in 0..60 {
        let shown = a.step(16, dt, true);
        numbers.push(shown.number);
        meters.push(shown.meter);
    }
    assert!(numbers[0] < numbers[8], "it counts up first: {numbers:?}");
    assert_eq!(*numbers.last().expect("frames"), 16, "it ends on the value");
    assert!(!a.running(), "the animation ends");
    assert!(meters.contains(&10.0), "the flash shows the old bar");
    // a falling figure counts down from the old value to the new one
    let mut b = CountUp::default();
    b.start(30, false);
    let first = b.step(20, dt, false);
    let mut last = first;
    for _ in 0..30 {
        last = b.step(20, dt, false);
    }
    assert!(first.number > 20 && last.number >= 20 && last.number < first.number);
    // armor plating: the same shape with the armor's flash
    let mut c = CountUp::default();
    c.start(100, false);
    let mut flashed = false;
    for _ in 0..60 {
        let shown = c.step(120, dt, false);
        flashed |= shown.meter == 100.0;
        assert!(shown.show_denominator);
    }
    assert!(flashed);
    // a heat sink flashes the denominator instead
    let mut d = CountUp::default();
    d.start(9, true);
    let mut hid = false;
    for _ in 0..60 {
        hid |= !d.step(10, dt, true).show_denominator;
    }
    // (the heat sink lowers the heat, so the flash is skipped when old >= cur; here it is a rise)
    assert!(hid);
}

#[test]
fn the_options_gameplay_tab_spins_a_mech() {
    let Some(mut s) = setup() else { return };
    let Some(mut previews) = previews(&s.root) else {
        return;
    };
    let gpu = gpu().expect("gpu");
    let mut screen = Screen::new(gpu);
    let prefs = Prefs::default();
    let mut ld = Loader::new(&mut s.vfs, &mut s.h.assets);
    let mut options = OptionsScreen::load(&mut ld, prefs.clone(), prefs, &OptionsEnv::default())
        .expect("options FITs");
    options.select_tab(2);
    let panel = {
        let req = options.gameplay_preview().expect("a camera");
        assert_eq!(req.shape, "bushwacker");
        req
    };
    let mut crops = Vec::new();
    let mut yaws = Vec::new();
    // where the panel is: found from the request's size and the screen's non-black area
    for n in 0..45 {
        s.h.frame(&[], |ctx| {
            options.update(ctx);
        });
        let dl = s.h.render(|ctx| options.render(ctx));
        previews.resolve(&mut s.h.assets);
        let img = screen.draw(&s.h.assets, &dl);
        if n == 44 {
            write_png("options_gameplay.png", 800, 600, &img);
        }
        crops.push(img);
        yaws.push(options.gameplay_preview().expect("camera").yaw);
    }
    assert!(panel.width > 8 && panel.height > 8);
    // the screen differs between frames a second apart, only where the model is
    let turned = differing(&crops[14], &crops[44]);
    assert!(turned > 200, "the mech turns: {turned} pixels differ");
    assert_ne!(yaws[14], yaws[44]);
    // and a picture of it was rendered for the tab: the store holds the panel-sized pictures
    let pictures =
        s.h.assets
            .textures
            .iter()
            .filter(|(_, i)| i.name.starts_with("preview:bushwacker"))
            .count();
    assert!(
        pictures >= 3,
        "{pictures} pictures of the mech were rendered"
    );
}

/// The component list draws first (`MechLabScreen::render`), so the black covers it puts over the
/// rows hanging past its bottom edge never end up over the ACCEPT button: the last quad that
/// reaches the button is not an opaque black fill.
#[test]
fn no_black_cover_is_drawn_over_the_accept_button() {
    let Some(mut s) = setup() else { return };
    let (mut lab, _) = open_lab(&mut s);
    frame(&mut s, &mut lab, &[]);
    let dl = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    let b = lab
        .screen
        .button(button::MB_MSG_NEXT)
        .expect("accept button");
    let (l, t, r, bt) = (
        b.global_x() as f32,
        b.global_y() as f32,
        b.global_x() as f32 + b.width(),
        b.global_y() as f32 + b.height(),
    );
    let mut covered = Vec::new();
    for batch in &dl.batches {
        for q in batch.first_quad..batch.first_quad + batch.quad_count {
            let v = &dl.vertices[q as usize * 4..q as usize * 4 + 4];
            let (x0, y0, x1, y1) = (v[0].x, v[0].y, v[2].x, v[2].y);
            if x1 > l && x0 < r && y1 > t && y0 < bt {
                covered.push((batch.texture.is_none(), v[0].argb));
            }
        }
    }
    let last = covered.last().expect("the button is drawn");
    assert!(
        !(last.0 && last.1 == 0xff00_0000),
        "the last thing over ACCEPT is an opaque black fill"
    );
    // and the list is not what draws the black cover now: the button's art comes after it
    let first_black = covered.iter().position(|c| c.0 && c.1 == 0xff00_0000);
    if let Some(i) = first_black {
        assert!(
            covered[i + 1..].iter().any(|c| !c.0),
            "button art over the cover"
        );
    }
}

//! Tests over the shipped game data. Set `MC2_DATA` to the data root; without it every test
//! returns early.

use mc2_formats::vfs::Vfs;
use mc2_ui::ctx::msg;
use mc2_ui::cursor::{Cursors, M_STATE_LOGISTICS};
use mc2_ui::font::text::{string_size, TextAttrs};
use mc2_ui::font::{Font, FontFormat};
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::screen::{LogisticsScreen, Prefixes};
use mc2_ui::strings::StringTable;
use mc2_ui::widgets::{ListBox, Object, TextListItem};
use mc2_ui::{Loader, UiAssets};

fn vfs() -> Option<Vfs> {
    match std::env::var_os("MC2_DATA") {
        Some(root) => Some(Vfs::from_system_cfg(root).expect("vfs")),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            None
        }
    }
}

#[test]
fn every_shipped_font_decodes_with_its_metrics() {
    let Some(mut vfs) = vfs() else { return };
    let expect = [
        ("agencyfb8", 11, 9, 8),
        ("agencyfb11", 11, 9, 8),
        ("agencyfb14", 14, 13, 13),
        ("agencyfb17", 14, 13, 13),
        ("agencyfb20", 14, 13, 13),
        ("arial8", 11, 9, 8),
        ("arial9chat", 11, 9, 8),
        ("arial9game", 11, 9, 8),
        ("arialblack9", 11, 9, 8),
        ("arialnarrow6", 6, 5, 6),
        ("arialnarrow8", 11, 9, 8),
        ("arialnarrow9", 11, 9, 8),
    ];
    for (name, line, ascent, cell) in expect {
        let f = Font::load(&mut vfs, &format!("assets/graphics/{name}.d3f")).unwrap();
        assert_eq!(f.format, FontFormat::GlyphBmp);
        assert_eq!(f.name, format!("assets/graphics/{name}.glyph"));
        assert_eq!((f.line_height, f.ascent), (line, ascent), "{name}");
        let a = f.glyph(b'A');
        assert_eq!((a.w as u32, a.h as u32), (cell, line), "{name} cell");
        // every printable ASCII character is present
        for c in 33u8..127 {
            assert!(f.glyph(c).present, "{name} lacks {}", c as char);
        }
        // the atlas has ink exactly where the glyph cells are
        assert!(f.atlas.iter().skip(3).step_by(4).any(|&a| a == 255));
    }
}

#[test]
fn font_metrics_of_the_menu_font() {
    let Some(mut vfs) = vfs() else { return };
    let f = Font::load(&mut vfs, "assets/graphics/agencyfb17.d3f").unwrap();
    let adv = |c: u8| i32::from(f.glyph(c).advance);
    // from agencyfb17.glyph: '!' 3, 'A' 6, 'W' 8, 'i' 3, space 4
    assert_eq!(
        (adv(b'!'), adv(b'A'), adv(b'W'), adv(b'i'), adv(b' ')),
        (3, 6, 8, 3, 4)
    );
    let a = TextAttrs::default();
    let label = "NEW CAMPAIGN";
    let w: i32 = label.bytes().map(adv).sum();
    assert_eq!(string_size(&f, label, &a), (w as u32, 14));
    assert_eq!(string_size(&f, "A\nB\nC", &a).1, 42);
    // the soft hyphen (173) is absent and advances nothing
    assert!(!f.glyph(173).present);
    // a .tga name finds the same font family's glyph file
    let chat = Font::load(&mut vfs, "assets/graphics/arial9chat.tga").unwrap();
    assert_eq!(chat.name, "assets/graphics/arial9chat.glyph");
}

#[test]
fn fonts_load_by_resource_id() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let f = ld.font(27616); // IDS "AgencyFB17.d3f"
    let g = ld.font(27630); // "AgencyFB11.d3f"
    assert_eq!(f.size, 1);
    let (fi, gi) = (f.id.unwrap(), g.id.unwrap());
    assert_ne!(fi, gi);
    assert_eq!(assets.fonts.get(fi).line_height, 14);
    assert!(assets.fonts.get(fi).texture.is_some());
    // loading again reuses the font
    let mut ld = Loader::new(&mut vfs, &mut assets);
    assert_eq!(ld.font(27621).id, Some(fi));
}

#[test]
fn string_table_matches_the_shipped_resource_library() {
    let Some(vfs) = vfs() else { return };
    let Ok(so) = std::fs::read(vfs.root().join("libmc2res_64.so")) else {
        eprintln!("libmc2res_64.so not present; skipping");
        return;
    };
    let t = StringTable::builtin();
    // every NUL-terminated byte run of the library, and every suffix of it (the linker may merge
    // a string into the tail of a longer one)
    let mut runs = std::collections::HashSet::new();
    for run in so.split(|&b| b == 0) {
        for i in 0..run.len() {
            runs.insert(&run[i..]);
        }
    }
    let mut missing = Vec::new();
    for (id, s) in t.iter() {
        let bytes = mc2_ui::cp1252::encode(s);
        if !bytes.is_empty() && !runs.contains(bytes.as_slice()) {
            missing.push(id);
        }
    }
    assert!(
        missing.is_empty(),
        "{} strings not in libmc2res_64.so, e.g. {:?}",
        missing.len(),
        &missing[..missing.len().min(5)]
    );
}

#[test]
fn main_menu_layout_matches_mcl_mm_fit() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let s = LogisticsScreen::load(&mut ld, "mcl_mm", Prefixes::default()).unwrap();
    assert_eq!(
        (
            s.statics.len(),
            s.rects.len(),
            s.buttons.len(),
            s.texts.len()
        ),
        (14, 3, 10, 2)
    );
    let ids: Vec<i32> = s.buttons.iter().map(|b| b.id()).collect();
    assert_eq!(ids, vec![90, 91, 92, 93, 94, 95, 96, 97, 98, 99]);
    let ys: Vec<i32> = s.buttons[..9].iter().map(|b| b.global_y()).collect();
    assert_eq!(ys, vec![121, 163, 205, 247, 416, 331, 373, 458, 289]);
    let new_campaign = s.button(90).unwrap();
    let new_campaign_tex = new_campaign.obj.texture.unwrap();
    assert_eq!(
        (
            new_campaign.global_x(),
            new_campaign.width(),
            new_campaign.height()
        ),
        (274, 256.0, 32.0)
    );
    assert_eq!(new_campaign.data.text_id, 27600);
    assert_eq!(new_campaign.data.state_coords[4], [0.0, 64.0]);
    // UVs: the normal cell (0, 0)-(256, 32) of the 256-wide texture, plus the original's bias
    let l = &new_campaign.obj.loc;
    assert!((l[0].u - 0.1 / 256.0).abs() < 1e-6 && (l[2].u - (1.0 + 0.1 / 256.0)).abs() < 1e-6);
    assert!((l[2].v - (32.0 / 256.0 + 0.1 / 256.0)).abs() < 1e-6);
    let legal = s.button(99).unwrap();
    assert_eq!((legal.global_x(), legal.global_y()), (723, 223));
    assert_eq!(s.statics[3].global_rect().left, 768);
    assert_eq!(s.rects[2].color(), 0xff43_311c);
    assert!(s.rects[2].outline);
    assert_eq!(s.texts[0].alignment, 3);
    assert_eq!(s.texts[1].obj.global_x(), 37);
    let t = assets.textures.get(s.statics[0].texture.unwrap()).unwrap();
    assert_eq!(
        (t.name.as_str(), t.width, t.height),
        ("data/art/mcl_mm_1.tga", 256, 256)
    );
    // statics without `Alpha` are keyed: one bit of alpha
    assert!(t
        .rgba
        .iter()
        .skip(3)
        .step_by(4)
        .all(|&a| a == 0 || a == 255));
    // buttons load the alpha version of their art
    let bt = assets.textures.get(new_campaign_tex).unwrap();
    assert_eq!(bt.name, "data/art/mcl_mm_12.tga");
    assert_ne!(Some(new_campaign_tex), s.statics[3].texture);
}

#[test]
fn cursors_load_both_sizes() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let small = Cursors::load(&mut vfs, &mut assets.textures, "cursors", 800).unwrap();
    let large = Cursors::load(&mut vfs, &mut assets.textures, "cursors", 1024).unwrap();
    assert_eq!(small.infos.len(), 243);
    assert_eq!(large.infos.len(), 243);
    let c = &large.infos[0];
    assert_eq!((c.hot_x, c.hot_y, c.frames), (13, 10, 1));
    assert!(large.infos[M_STATE_LOGISTICS as usize].texture.is_some());
    let h = Harness::with_assets(assets);
    let dl = h.render(|ctx| large.draw(ctx.dl, 0, (100, 100)));
    assert_eq!(dl.vertices[0].x, 87.0);
    assert_eq!(dl.vertices[0].y, 90.0);
}

#[test]
fn list_box_scroll_bar_scrolls_the_items() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let mut l = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        ListBox::new(&mut ld, 100.0, 100.0, 200.0, 100.0)
    };
    for _ in 0..20 {
        let it = TextListItem {
            obj: Object::new(0.0, 0.0, 150.0, 20.0),
            ..TextListItem::default()
        };
        l.add_item(Box::new(it)).unwrap();
    }
    let sb = l.scroll_bar.as_ref().unwrap();
    // 20 items of 20 + 1 pixel: 419 + 1 = 420 high, the bar is 96 high
    assert_eq!(sb.scroll_max, 420.0 - 96.0);
    assert_eq!(sb.top.id(), msg::SCROLLUP as i32);
    let (bx, by) = (
        sb.bottom.global_x() + 3,
        sb.bottom.global_y() + (sb.bottom.height() as i32) / 2,
    );
    let mut h = Harness::with_assets(assets);
    for _ in 0..10 {
        h.frame(&[], |ctx| l.update(ctx));
    }
    let y0 = l.items[0].obj().global_y();
    // click and hold the down arrow: one step per frame
    // the drag origin is the pointer position of the previous frame: move, then press
    h.frame(&[move_to(bx as f32, by as f32)], |ctx| l.update(ctx));
    h.frame(&[left(true)], |ctx| l.update(ctx));
    for _ in 0..4 {
        h.frame(&[], |ctx| l.update(ctx));
    }
    h.frame(&[left(false)], |ctx| l.update(ctx));
    let y1 = l.items[0].obj().global_y();
    assert!(y1 < y0, "items moved up ({y0} -> {y1})");
    assert_eq!(l.scroll_pos() as i32, y0 - y1);
    // selecting the last item scrolls it into view
    l.select_item(19);
    let last = l.items[19].obj();
    assert!(last.global_y() as f32 + last.height() <= 200.0 + 1.0);
}

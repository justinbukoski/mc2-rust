// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission objectives panel (`ControlGui::RenderObjectives`, `renderObjective`,
//! `renderPlayerStatus`, `startObjectives` in `code/controlgui.cpp`).
//!
//! Expected values are hand-derived from the C++ (file:line cited beside each one) and from
//! `data/art/buttonlayout1024.fit` (line numbers cited), never read back from the port. The tests
//! that need the shipped data skip cleanly when `MC2_DATA` is unset.

use mc2_formats::vfs::Vfs;
use mc2_ui::draw::{DrawList, GuiRect, Vertex};
use mc2_ui::font::text::{self, TextAttrs};
use mc2_ui::font::FontRef;
use mc2_ui::harness::Harness;
use mc2_ui::objectives::{
    dot_leader, icon_for, objective_color, player_status_layout, status_color, Icon, MoveInfo,
    ObjectiveStatus, ObjectiveText, ObjectiveView, ObjectivesPanel, ObjectivesView, PanelLayout,
    PlayerRow, PlayerStatusUnavailable, PlayerStatusView, StaticInfo, LOG_KILLMARKER,
    OBJECTIVE_MOVE,
};
use mc2_ui::widgets::basic::{font_height, font_width};
use mc2_ui::{Loader, UiAssets};

/// `[Objectives]` and `[PrimaryObj*]` of `data/art/buttonlayout1024.fit` (lines 3059-3159):
/// `Objectiveleft` 45 (3061), `Objectiveskip` 15 (3063), `SecondObjskip` 25 (3065),
/// `totalright` 475 (3067), `Headerleft` 30 and `Headertop` 87 (`[PrimaryObjHeader]`),
/// `objBoxSkip` -2 in `[PrimaryObjBox]` (3122) and -1 in `[PrimaryObjCheck]` (3141).
fn fit_layout() -> PanelLayout {
    PanelLayout {
        left: 45,
        top: 87,
        header_left: 30,
        item_skip: 15,
        secondary_skip: 25,
        total_right: 475,
        box_skip: -2,
        check_skip: -1,
        font: FontRef::default(),
        edges: Vec::new(),
        box_frame: StaticInfo::default(),
        check: StaticInfo::default(),
        cross: StaticInfo::default(),
    }
}

fn primary(description: u32, status: ObjectiveStatus) -> ObjectiveView {
    ObjectiveView {
        description: ObjectiveText::Resource(description),
        status,
        ..ObjectiveView::default()
    }
}

fn secondary(description: u32, status: ObjectiveStatus) -> ObjectiveView {
    ObjectiveView {
        priority: 2,
        ..primary(description, status)
    }
}

const U: ObjectiveStatus = ObjectiveStatus::Undetermined;
const S: ObjectiveStatus = ObjectiveStatus::Successful;
const F: ObjectiveStatus = ObjectiveStatus::Failed;

// ---------------------------------------------------------------------------------------------
// 1. layout positions

/// `pos = OBJECTIVESTOP + OBJECTIVESSKIP` then `+= OBJECTIVESSKIP` per primary row
/// (controlgui.cpp:678-703); the secondary header is `pos - SKIP + OBJEECTIVESHEADERSKIP`
/// (707-712), so for n primaries it is `87 + 15n + 25`.
#[test]
fn primary_rows_stack_from_the_header_for_one_to_eight_objectives() {
    // 87 + 15 * n for n = 1..=8: the last primary row, from 102 (n=1) to 207 (n=8)
    const LAST_ROW_Y: [i32; 8] = [102, 117, 132, 147, 162, 177, 192, 207];
    // 87 + 15 * n + 25 for n = 1..=8
    const SECONDARY_HEADER_Y: [i32; 8] = [127, 142, 157, 172, 187, 202, 217, 232];
    let panel = ObjectivesPanel::new(fit_layout());
    for n in 1..=8usize {
        let objs: Vec<ObjectiveView> = (0..n).map(|i| primary(50001 + i as u32, U)).collect();
        let l = panel.listing(&objs);
        let ys: Vec<i32> = l.rows.iter().map(|r| r.y).collect();
        let want: Vec<i32> = (0..n as i32).map(|k| 102 + 15 * k).collect();
        assert_eq!(ys, want, "n={n}");
        assert_eq!(*ys.last().unwrap(), LAST_ROW_Y[n - 1], "n={n}");
        assert_eq!(l.primary_header_y, 87, "n={n}");
        assert_eq!(l.secondary_header_y, SECONDARY_HEADER_Y[n - 1], "n={n}");
        // with no secondary rows the total line sits one skip below the secondary header
        assert_eq!(l.total_y, SECONDARY_HEADER_Y[n - 1] + 15, "n={n}");
    }
}

/// Five primaries and three secondaries, in list order [P, S, P, P, S, P, S, P]: primaries start
/// at 102; the secondary header is 87 + 75 + 25 = 187 (controlgui.cpp:707-712); secondary row j
/// is at header + 15 * (j + 1) (716-723); the total is at header + 15 * (3 + 1) = 247 (745-746).
#[test]
fn mixed_primary_and_secondary_rows_keep_list_order_and_stack_below_the_primary_block() {
    let panel = ObjectivesPanel::new(fit_layout());
    let objs = vec![
        primary(50001, U),
        secondary(50002, U),
        primary(50003, U),
        primary(50004, U),
        secondary(50005, U),
        primary(50006, U),
        secondary(50007, U),
        primary(50008, U),
    ];
    let l = panel.listing(&objs);
    let rows: Vec<(usize, usize, bool, i32)> = l
        .rows
        .iter()
        .map(|r| (r.objective, r.order, r.primary, r.y))
        .collect();
    assert_eq!(
        rows,
        vec![
            (0, 0, true, 102),
            (2, 1, true, 117),
            (3, 2, true, 132),
            (5, 3, true, 147),
            (7, 4, true, 162),
            (1, 5, false, 202),
            (4, 6, false, 217),
            (6, 7, false, 232),
        ]
    );
    assert_eq!(l.primary_header_y, 87);
    assert_eq!(l.secondary_header_y, 187);
    assert_eq!(l.total_y, 247);
}

/// Hidden triggers, inactive objectives, and inactive-or-hidden secondaries are not listed
/// (controlgui.cpp:685 and 718: `IsActive() && !IsHiddenTrigger()`); a priority other than 1 is
/// a secondary (`!(1 == Priority())`).
#[test]
fn hidden_and_inactive_objectives_are_not_listed() {
    let panel = ObjectivesPanel::new(fit_layout());
    let objs = vec![
        primary(50001, U), // 0: listed, primary
        ObjectiveView {
            is_active: false,
            ..primary(50002, U)
        }, // 1: inactive
        ObjectiveView {
            is_hidden_trigger: true,
            ..primary(50003, U)
        }, // 2: hidden trigger
        ObjectiveView {
            priority: 3,
            ..secondary(50004, U)
        }, // 3: listed, secondary
        ObjectiveView {
            is_hidden_trigger: true,
            ..secondary(50005, U)
        }, // 4: hidden trigger
        primary(50006, U), // 5: listed, primary
    ];
    let l = panel.listing(&objs);
    let rows: Vec<(usize, usize, bool, i32)> = l
        .rows
        .iter()
        .map(|r| (r.objective, r.order, r.primary, r.y))
        .collect();
    // two primaries: secondary header 87 + 30 + 25 = 142, secondary row at 142 + 15 = 157,
    // total at 142 + 2 * 15 = 172
    assert_eq!(
        rows,
        vec![(0, 0, true, 102), (5, 1, true, 117), (3, 2, false, 157)]
    );
    assert_eq!(l.secondary_header_y, 142);
    assert_eq!(l.total_y, 172);
}

// ---------------------------------------------------------------------------------------------
// 2. icon and colour per status

/// `renderObjective` colour: failed `0xffc61c00`, successful `0xff41c700`, otherwise
/// `0xffffc600` (controlgui.cpp:830-831).
#[test]
fn status_colour_matches_the_original() {
    assert_eq!(status_color(F), 0xffc6_1c00);
    assert_eq!(status_color(S), 0xff41_c700);
    assert_eq!(status_color(U), 0xffff_c600);
}

/// Successful draws the check and failed the cross; undetermined draws neither
/// (controlgui.cpp:907-930).
#[test]
fn icon_follows_the_status() {
    assert_eq!(icon_for(U), None);
    assert_eq!(icon_for(S), Some(Icon::Check));
    assert_eq!(icon_for(F), Some(Icon::Cross));
}

/// The box is drawn at `yPos + objBoxSkip` (-2) and the check or cross at `yPos + objBoxSkip`
/// of its own block (-1), each keeping its fit x, width and height (controlgui.cpp:897-930;
/// fit lines 3118-3159: box 33,12x12; check and cross 34,10x10).
#[test]
fn box_and_icon_are_placed_relative_to_the_row() {
    let mut layout = fit_layout();
    layout.box_frame = StaticInfo::at(33.0, 0.0, 12.0, 12.0);
    layout.check = StaticInfo::at(34.0, 0.0, 10.0, 10.0);
    layout.cross = StaticInfo::at(34.0, 0.0, 10.0, 10.0);
    let panel = ObjectivesPanel::new(layout);
    assert_eq!(panel.box_quad(117).rect(), GuiRect::new(33, 115, 45, 127));
    let (kind, check) = panel.icon_quad(S, 117).expect("check");
    assert_eq!(kind, Icon::Check);
    assert_eq!(check.rect(), GuiRect::new(34, 116, 44, 126));
    let (kind, cross) = panel.icon_quad(F, 117).expect("cross");
    assert_eq!(kind, Icon::Cross);
    assert_eq!(cross.rect(), GuiRect::new(34, 116, 44, 126));
    assert!(panel.icon_quad(U, 117).is_none());
}

// ---------------------------------------------------------------------------------------------
// 3. animation timing

/// `startObjectives(true)` from closed resets `objectiveTime` to 0 (controlgui.cpp:471-472),
/// and each open frame adds `frameLength` (618). Closing is `renderObjectives = 0` (the button
/// handler), which stops the advance and draws nothing.
#[test]
fn open_and_close_advance_the_timer_like_render_objectives() {
    let mut panel = ObjectivesPanel::new(fit_layout());
    let view = ObjectivesView::default();
    let mut h = Harness::with_assets(UiAssets::default());
    h.dt = 0.5;
    panel.start(true, false);
    assert!(panel.is_open());
    for _ in 0..3 {
        h.frame(&[], |ctx| panel.update(ctx, &view));
    }
    assert_eq!(panel.objective_time(), 1.5);
    panel.close();
    assert!(!panel.is_open());
    h.frame(&[], |ctx| panel.update(ctx, &view));
    assert_eq!(panel.objective_time(), 1.5);
}

/// `startObjectives` while the results are on screen sets the timer to
/// `objectiveMoveInfo[LAST].time` (controlgui.cpp:473-474), which is 0.0 in the table
/// (controlgui.cpp:130-133).
#[test]
fn restarting_during_results_resets_to_the_last_move_time() {
    let mut panel = ObjectivesPanel::new(fit_layout());
    let view = ObjectivesView {
        render_status_info: true,
        results_time: 1.0,
        ..ObjectivesView::default()
    };
    let mut h = Harness::with_assets(UiAssets::default());
    panel.start(true, false);
    h.dt = 2.0;
    h.frame(&[], |ctx| panel.update(ctx, &view));
    assert_eq!(panel.objective_time(), 2.0);
    panel.start(true, true);
    assert_eq!(panel.objective_time(), OBJECTIVE_MOVE[1].time);
    assert_eq!(panel.objective_time(), 0.0);
}

/// Mission results gate the objectives: while `resultsTime < .24` nothing advances or draws
/// (controlgui.cpp:611-615), and from `.24` on the timer runs.
#[test]
fn results_gate_holds_the_objectives_until_0_24_seconds() {
    let mut panel = ObjectivesPanel::new(fit_layout());
    panel.start(true, false);
    let mut h = Harness::with_assets(UiAssets::default());
    h.dt = 0.1;
    let early = ObjectivesView {
        render_status_info: true,
        results_time: 0.1,
        ..ObjectivesView::default()
    };
    h.frame(&[], |ctx| panel.update(ctx, &early));
    assert_eq!(panel.objective_time(), 0.0);
    let late = ObjectivesView {
        results_time: 0.24,
        ..early
    };
    h.frame(&[], |ctx| panel.update(ctx, &late));
    assert_eq!(panel.objective_time(), 0.1);
}

/// The shipped `objectiveMoveInfo` has both times at 0.0 (controlgui.cpp:130-133), so the
/// segment search never matches (626-633) and the panel's edge moves to the table's last
/// position, 0, in one step (648): `delta = 0 - edgeX`.
#[test]
fn shipped_slide_table_snaps_the_edge_to_zero() {
    let panel = ObjectivesPanel::new(fit_layout());
    assert_eq!(panel.slide_delta(0.0, 1024.0, 0.0), 0.0);
    assert_eq!(panel.slide_delta(5.0, 1024.0, 0.0), 0.0);
    assert_eq!(panel.slide_delta(0.0, 1024.0, -30.0), 30.0);
}

/// A synthetic two-point table: from (0 s, -640 px) to (1 s, 0 px), scaled by `screenWidth/640`
/// (controlgui.cpp:632-633), so p0 = -1024 and p1 = 0 at 1024 wide. At t = 0.5 the position is
/// -1024 + 0.5 * (1024 / 1) = -512 (642-644); the delta is -512 - edgeX. Past the last time the
/// else branch uses the unscaled last position (648).
#[test]
fn interpolated_slide_scales_the_positions_by_screen_width() {
    let panel = ObjectivesPanel::new(fit_layout()).with_moves([
        MoveInfo {
            time: 0.0,
            position: -640.0,
        },
        MoveInfo {
            time: 1.0,
            position: 0.0,
        },
    ]);
    assert_eq!(panel.slide_delta(0.5, 1024.0, -1024.0), 512.0);
    assert_eq!(panel.slide_delta(0.0, 1024.0, 0.0), -1024.0);
    assert_eq!(panel.slide_delta(1.0, 1024.0, -1024.0), 1024.0);
}

/// One kill-marker click per successful objective, the first frame its total is shown:
/// `bTotal = renderStatusInfo && objectiveCount < objectiveTime` (controlgui.cpp:689, 722), with
/// `objectiveCount` the row's list index (681, 734), and the sound once per row (694-698). The
/// sound id is `LOG_KILLMARKER` (mclib/sounds.h:115).
#[test]
fn kill_marker_sound_plays_once_per_successful_row_as_its_total_is_revealed() {
    let mut panel = ObjectivesPanel::new(fit_layout());
    panel.start(true, false);
    let view = ObjectivesView {
        objectives: vec![primary(50001, S), primary(50002, U), primary(50003, S)],
        render_status_info: true,
        results_time: 1.0,
        ..ObjectivesView::default()
    };
    let mut h = Harness::with_assets(UiAssets::default());
    h.dt = 0.5;
    let mut played = Vec::new();
    for _ in 0..6 {
        h.frame(&[], |ctx| panel.update(ctx, &view));
        played.push(h.out.sounds.clone());
    }
    // times 0.5, 1.0, 1.5, 2.0, 2.5, 3.0; row 0 shows at 0.5 (0 < t), row 2 at 2.5 (2 < t)
    assert_eq!(
        played,
        vec![
            vec![LOG_KILLMARKER],
            vec![],
            vec![],
            vec![],
            vec![LOG_KILLMARKER],
            vec![],
        ]
    );
    assert_eq!(LOG_KILLMARKER, 103);
}

// ---------------------------------------------------------------------------------------------
// 4. flashing and the leader dots

/// A newly activated objective flashes for 20 s (controlgui.cpp:834-835): `tempI = (int)(2 Hz *
/// elapsed)` and the colour is white when `tempI` is even (837-843). Otherwise the status colour.
#[test]
fn activation_flash_alternates_white_for_twenty_seconds() {
    let failed = ObjectiveView {
        status: F,
        activate_on_flag: true,
        activation_time: 10.0,
        ..ObjectiveView::default()
    };
    let red = 0xffc6_1c00;
    assert_eq!(objective_color(&failed, 10.0), 0xffff_ffff); // tempI 0
    assert_eq!(objective_color(&failed, 10.5), red); // tempI 1
    assert_eq!(objective_color(&failed, 12.0), 0xffff_ffff); // tempI 4
    assert_eq!(objective_color(&failed, 29.99), red); // 19.99 < 20, tempI 39
    assert_eq!(objective_color(&failed, 30.0), red); // 20 > 20 is false: no flash
    let plain = ObjectiveView {
        activate_on_flag: false,
        ..failed
    };
    assert_eq!(objective_color(&plain, 10.0), red);
}

/// `dotWidth = width("....................")`, `dotRealWidth = dotWidth / 20`,
/// `numberOfDots = (long)(totalLength / dotRealWidth)` with a floor of 3, and `numberOfDots - 2`
/// dots are drawn (controlgui.cpp:870-900). Worked: totalRight 475, amount width 40, x 45,
/// description 300, dot width 100 gives totalLength 90, 90 / 5 = 18, so 16 dots, 80 px.
#[test]
fn leader_dots_fill_the_gap_before_the_total() {
    assert_eq!(dot_leader(475, 40, 45, 300, 100), Some((16, 80.0)));
    // totalLength 92 -> (long)(92 / 5) = 18: the fraction is dropped
    assert_eq!(dot_leader(475, 40, 45, 298, 100), Some((16, 80.0)));
    // totalLength -30 -> -6 -> floor 3 -> one dot (controlgui.cpp:878-880)
    assert_eq!(dot_leader(475, 40, 45, 420, 100), Some((1, 5.0)));
}

// ---------------------------------------------------------------------------------------------
// 5. multiplayer player status panel

/// `renderPlayerStatus` backdrop: header rect grown by 5 on each side, then `+ height + 4` per
/// shown row (controlgui.cpp:787-796); rows at `i * (height + 4)` (800). The score is shown
/// on the first row of each team (499-503). Teams [0, 0, 1] give [true, false, true].
#[test]
fn player_status_backdrop_grows_per_row_and_shows_one_score_per_team() {
    let view = PlayerStatusView {
        header: GuiRect::new(100, 200, 300, 220),
        rows: vec![
            PlayerRow { team: 0 },
            PlayerRow { team: 0 },
            PlayerRow { team: 1 },
        ],
    };
    let l = player_status_layout(&view, 0.0);
    // left 100 - 5, top 200 - 5, right 300 + 5, bottom 220 + 5 + 3 * 24
    assert_eq!(l.backdrop, GuiRect::new(95, 195, 305, 297));
    let slots: Vec<(usize, i32, bool)> = l
        .rows
        .iter()
        .map(|r| (r.player, r.y, r.show_score))
        .collect();
    assert_eq!(slots, vec![(0, 24, true), (1, 48, false), (2, 72, true)]);
}

/// The x offset is added before the `(long)` cast (controlgui.cpp:787-792): 95 - 12.7 = 82.3
/// gives 82, and 305 - 12.7 = 292.3 gives 292. Only eight rows are shown (`MAX_MC_PLAYERS`,
/// multplyr.h:101; the loop at 496 runs i = 1..=8).
#[test]
fn player_status_truncates_the_offset_and_shows_at_most_eight_rows() {
    let view = PlayerStatusView {
        header: GuiRect::new(100, 200, 300, 220),
        rows: (0..9).map(|t| PlayerRow { team: t }).collect(),
    };
    let l = player_status_layout(&view, -12.7);
    assert_eq!(l.backdrop.left, 82);
    assert_eq!(l.backdrop.right, 292);
    assert_eq!(l.rows.len(), 8);
    assert_eq!(l.backdrop.bottom, 225 + 8 * 24);
}

// ---------------------------------------------------------------------------------------------
// 6. drawing (with a stand-in texture, so the quads are visible)

fn textured(assets: &mut UiAssets, x: f32, y: f32, w: f32, h: f32) -> StaticInfo {
    let tex = assets
        .textures
        .insert("test", 64, 64, vec![255; 64 * 64 * 4]);
    StaticInfo {
        texture: Some(tex),
        ..StaticInfo::at(x, y, w, h)
    }
}

/// A successful primary row draws its box and the check; the cross is not drawn and a hidden
/// objective draws nothing (controlgui.cpp:897-930, 907).
#[test]
fn successful_row_draws_box_and_check_and_hidden_rows_draw_nothing() {
    let mut assets = UiAssets::default();
    let mut layout = fit_layout();
    layout.box_frame = textured(&mut assets, 33.0, 0.0, 12.0, 12.0);
    layout.check = textured(&mut assets, 34.0, 0.0, 10.0, 10.0);
    layout.cross = textured(&mut assets, 34.0, 0.0, 10.0, 10.0);
    let mut panel = ObjectivesPanel::new(layout);
    panel.start(true, false);
    let view = ObjectivesView {
        objectives: vec![
            primary(50001, S),
            ObjectiveView {
                is_hidden_trigger: true,
                ..primary(50002, F)
            },
        ],
        ..ObjectivesView::default()
    };
    let h = Harness::with_assets(assets);
    let dl = h.render(|ctx| panel.render(ctx, &view).expect("single-player panel"));
    // box then check for row 0 at y 102; nothing for the hidden row
    assert_eq!(dl.quad_count(), 2);
    let v = &dl.vertices;
    assert_eq!((v[0].x, v[0].y), (33.0, 100.0));
    assert_eq!((v[2].x, v[2].y), (45.0, 112.0));
    assert_eq!((v[4].x, v[4].y), (34.0, 101.0));
    assert_eq!((v[6].x, v[6].y), (44.0, 111.0));
}

/// `RenderObjectives` draws the scoreboard instead of the objectives when the game is
/// multiplayer (controlgui.cpp:664-667). The scoreboard rows are not ported, so the panel
/// reports that with an explicit error, after drawing the bars it shares with the single-player
/// panel, and draws no objective and no backdrop of its own.
#[test]
fn multiplayer_scoreboard_is_reported_unavailable_and_not_drawn_as_a_partial_panel() {
    let mut panel = ObjectivesPanel::new(fit_layout());
    panel.start(true, false);
    let view = ObjectivesView {
        objectives: vec![primary(50001, S)],
        player_status: Some(PlayerStatusView {
            header: GuiRect::new(100, 200, 300, 220),
            rows: vec![PlayerRow { team: 0 }],
        }),
        ..ObjectivesView::default()
    };
    let h = Harness::with_assets(UiAssets::default());
    let mut result = None;
    let dl = h.render(|ctx| result = Some(panel.render(ctx, &view)));
    assert_eq!(result, Some(Err(PlayerStatusUnavailable)));
    assert_eq!(dl.quad_count(), 0);
}

// ---------------------------------------------------------------------------------------------
// 7. the shipped layout (needs MC2_DATA)

fn vfs() -> Option<Vfs> {
    match std::env::var_os("MC2_DATA") {
        Some(root) => Some(Vfs::from_system_cfg(root).expect("vfs")),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            None
        }
    }
}

/// Reads the real `[Objectives]` block: every value below is a line of
/// `data/art/buttonlayout1024.fit` (cited), `ObjectiveFont` is `[Fonts]` line 8 (40025), and the
/// description of mc2_01's first objective is string 50001 (`data/missions/mc2_01.fit`, team 0).
#[test]
fn shipped_buttonlayout_1024_gives_the_objective_layout() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let mut fit = ld.art_fit("buttonlayout1024").expect("fit");
    let panel = ObjectivesPanel::from_fit(&mut ld, &mut fit).expect("objectives block");
    let l = panel.layout();
    assert_eq!(l.font.res_id, 40025);
    assert_eq!(
        (
            l.left,
            l.top,
            l.header_left,
            l.item_skip,
            l.secondary_skip,
            l.total_right
        ),
        (45, 87, 30, 15, 25, 475)
    );
    assert_eq!((l.box_skip, l.check_skip), (-2, -1));
    // [ObjStatic0] and [ObjStatic1] (lines 3076-3108): 14 wide, 118 high, at y 76 and 194
    let rects: Vec<GuiRect> = l.edges.iter().map(StaticInfo::rect).collect();
    assert_eq!(
        rects,
        vec![GuiRect::new(0, 76, 14, 194), GuiRect::new(0, 194, 14, 312)]
    );
    assert!(l.edges.iter().all(|e| e.texture.is_some()));
    // [PrimaryObjBox] (3118-3135): x 33, 12 x 12; the block has no YLocation, so y is 0
    assert_eq!(l.box_frame.rect(), GuiRect::new(33, 0, 45, 12));
    // [PrimaryObjCheck] and [PrimaryObjX] (3137-3170): x 34, 10 x 10
    assert_eq!(l.check.rect(), GuiRect::new(34, 0, 44, 10));
    assert_eq!(l.cross.rect(), GuiRect::new(34, 0, 44, 10));
    assert!(
        l.box_frame.texture.is_some() && l.check.texture.is_some() && l.cross.texture.is_some()
    );
    // mc2_01's first objective: IDS_OBJECTIVES_1_1 (strings.res.h:3520) is the C++ literal
    // " 1. Investigate Abandoned Airfield" with its leading space (test_scripts/res_conv/strings.res.cpp)
    assert_eq!(
        assets.strings.load(50001),
        " 1. Investigate Abandoned Airfield"
    );
}

/// `drawShadowText` passes word wrap `false` (mclib/utilities.cpp:381), so a description wider
/// than the screen stays on its own line and is clipped at the edge. Wrapping would put its
/// second line one font height lower, into the next objective's row. Every glyph of the
/// 400-character description sits on one line: its vertical extent is one font height plus the
/// shadow's 1 px (a wrapped second line would add another `font_height`).
#[test]
fn long_description_stays_on_one_line_and_is_clipped_not_wrapped() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets {
        screen: (1024, 768),
        ..UiAssets::default()
    };
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let mut fit = ld.art_fit("buttonlayout1024").expect("fit");
    let shipped = ObjectivesPanel::from_fit(&mut ld, &mut fit).expect("objectives block");
    let font = shipped.layout().font;
    let line_h = font_height(&assets, font);
    assert!(line_h > 0, "the objective font is loaded");
    // the headers (IDS_PRIMARY 40010, IDS_SECONDARY 40011) are drawn at y 87 and 127 whatever the
    // rows are, and the edge bars span y 76..312: blank and remove them so only the text is measured
    std::sync::Arc::make_mut(&mut assets.strings).set(40010, "");
    std::sync::Arc::make_mut(&mut assets.strings).set(40011, "");
    let mut layout = shipped.layout().clone();
    layout.edges.clear();
    layout.box_frame = StaticInfo::default();
    let mut panel = ObjectivesPanel::new(layout);
    panel.start(true, false);
    let view = ObjectivesView {
        objectives: vec![ObjectiveView {
            description: ObjectiveText::Literal("W".repeat(400)),
            ..ObjectiveView::default()
        }],
        ..ObjectivesView::default()
    };
    let h = Harness::with_assets(assets);
    let dl = h.render(|ctx| panel.render(ctx, &view).expect("single-player panel"));
    let ys: Vec<f32> = dl.vertices.iter().map(|v| v.y).collect();
    let top = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let bottom = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(!ys.is_empty(), "the description draws glyphs");
    assert!(
        bottom - top <= line_h as f32 + 1.0,
        "one line: vertical extent {} is one font height {line_h} plus the 1 px shadow",
        bottom - top
    );
}

/// The glyph vertices `drawShadowText` lays down for `s` at `(x, y)`: the text in the black shadow
/// one pixel left and down (mclib/utilities.cpp:377-389), then the text in `color`. Laid out by
/// the font module itself, so a panel draw that matches these vertices shows this text here.
fn shadowed_run(
    assets: &UiAssets,
    font: FontRef,
    s: &str,
    color: u32,
    proportional: bool,
    x: i32,
    y: i32,
) -> Vec<Vertex> {
    let f = assets.font(font).expect("the objective font is loaded");
    let region = GuiRect::new(0, 0, assets.screen.0, assets.screen.1);
    let attrs = |color| TextAttrs {
        color,
        size: font.size as f32,
        word_wrap: false,
        proportional,
        ..TextAttrs::default()
    };
    let mut dl = DrawList::new(assets.screen.0, assets.screen.1);
    text::draw(&mut dl, f, s, &attrs(BLACK), region, (x - 1, y + 1));
    text::draw(&mut dl, f, s, &attrs(color), region, (x, y));
    dl.vertices
}

fn has_run(hay: &[Vertex], run: &[Vertex]) -> bool {
    !run.is_empty() && hay.windows(run.len()).any(|w| w == run)
}

const BLACK: u32 = 0xff00_0000;
const WHITE: u32 = 0xffff_ffff;
/// `0xff41c700` (controlgui.cpp:831) and `0xffc61c00` (830): successful and failed descriptions.
const GREEN: u32 = 0xff41_c700;
const RED: u32 = 0xffc6_1c00;
const DOTS_20: &str = "....................";
/// `IDS_TOTAL` (strings.res.h:1109, the strings fixture, id 45034): `"Total:   %ld"`.
const IDS_TOTAL_STR: u32 = 45034;

/// `sprintf( total, total2, sum )` (controlgui.cpp:746).
fn format_total(assets: &UiAssets, sum: i64) -> String {
    assets
        .string(IDS_TOTAL_STR)
        .replace("%ld", &sum.to_string())
}

/// The results (`renderStatusInfo`): each objective's amount, its leader dots and the running
/// total appear as the timer passes each row (controlgui.cpp:689 `bTotal = renderStatusInfo &&
/// objectiveCount < objectiveTime`). The amount is the points of a successful objective and `0`
/// otherwise (controlgui.cpp:856-857); the total sums the successful revealed rows (693, 746).
///
/// The expected values are the C++ worked out by hand, not read back from the panel: the colours
/// are the literals of controlgui.cpp:830-831, and the dot count and first-dot x are the
/// arithmetic of controlgui.cpp:870-900 for the measured widths below. The only measured inputs
/// are the shipped font widths of the strings, asserted first so a font change fails here.
///
/// Mission: three primaries, rows at y 102, 117 and 132 (`87 + 15 * (k + 1)`, controlgui.cpp:678-
/// 703). Successful 100 points, failed 250 points (shown as 0), successful 40 points. The
/// secondary header is 157 (`87 + 45 + 25`) and the total line 172 (`157 + 15`, 741).
///
/// Hand derivation per row, with `OBJECTIVESTOTALRIGHT` 475, `xPos` 45, `dotWidth` 160 so
/// `dotRealWidth` 8 (870-880), `totalLength = 475 - amount - 45 - description` (872), dots
/// `= (long)(totalLength / 8)` with a floor of 3 (874-877), `dots - 2` drawn (882), and the first
/// dot at `475 - amount - 8 * (dots - 2)` (885-892):
/// - row 0: description 272, amount "100" 24: totalLength 134, 16.75 -> 16, 14 dots, x 475-24-112 = 339.
/// - row 1: description 144, amount "0" 8: totalLength 278, 34.75 -> 34, 32 dots, x 475-8-256 = 211.
/// - row 2: description 200, amount "40" 16: totalLength 214, 26.75 -> 26, 24 dots, x 475-16-192 = 267.
#[test]
fn results_reveal_each_amount_its_dots_and_the_running_total_in_turn() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets {
        screen: (1024, 768),
        ..UiAssets::default()
    };
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let mut fit = ld.art_fit("buttonlayout1024").expect("fit");
    let mut panel = ObjectivesPanel::from_fit(&mut ld, &mut fit).expect("objectives block");
    let l = panel.layout().clone();
    let scored = |desc: u32, status: ObjectiveStatus, points: u32| ObjectiveView {
        description: ObjectiveText::Resource(desc),
        status,
        resource_points: points,
        ..ObjectiveView::default()
    };
    let objectives = vec![
        scored(50001, S, 100),
        scored(50002, F, 250),
        scored(50003, S, 40),
    ];
    let view = ObjectivesView {
        objectives: objectives.clone(),
        render_status_info: true,
        results_time: 1.0,
        ..ObjectivesView::default()
    };
    // the total format string (strings.res.h:1109, the strings fixture, id 45034)
    let mut h = Harness::with_assets(assets);
    assert_eq!(h.assets.string(IDS_TOTAL_STR), "Total:   %ld");
    // the measured widths the derivation above starts from (shipped `ObjectiveFont`)
    let a = &h.assets;
    for (k, (id, desc_w)) in [(50001, 272), (50002, 144), (50003, 200)]
        .into_iter()
        .enumerate()
    {
        let desc_text = a.string(id);
        assert_eq!(
            font_width(a, l.font, desc_text),
            desc_w,
            "row {k} description width"
        );
    }
    for (amount, w) in [("100", 24), ("0", 8), ("40", 16)] {
        assert_eq!(font_width(a, l.font, amount), w, "amount {amount} width");
    }
    assert_eq!(font_width(a, l.font, DOTS_20), 160, "twenty dots width");
    // (description width, leader dots, first dot x, colour) per row: the values of the derivation
    // above, with the colours of controlgui.cpp:830-831 (successful, failed, successful)
    const ROWS: [(i32, usize, i32, u32); 3] = [
        (272, 14, 339, GREEN),
        (144, 32, 211, RED),
        (200, 24, 267, GREEN),
    ];
    panel.start(true, false);
    h.dt = 0.5;
    // (objective_time after this frame, rows revealed, running total)
    let steps: [(f32, usize, i64); 5] = [
        (0.5, 1, 100),
        (1.0, 1, 100),
        (1.5, 2, 100),
        (2.0, 2, 100),
        (2.5, 3, 140),
    ];
    let row_y = [102, 117, 132];
    let total_y = 172;
    for (time, revealed, total) in steps {
        h.frame(&[], |ctx| panel.update(ctx, &view));
        assert_eq!(panel.objective_time(), time);
        let dl = h.render(|ctx| panel.render(ctx, &view).expect("single-player panel"));
        let a = &h.assets;
        for (k, obj) in objectives.iter().enumerate() {
            let y = row_y[k];
            let (desc_w_expected, dots, dot_x, color) = ROWS[k];
            let desc = match &obj.description {
                ObjectiveText::Resource(id) => a.string(*id).to_string(),
                ObjectiveText::Literal(s) => s.clone(),
            };
            assert_eq!(font_width(a, l.font, &desc), desc_w_expected);
            let desc_run = shadowed_run(a, l.font, &desc, color, true, l.left, y);
            assert!(has_run(&dl.vertices, &desc_run), "row {k} description");
            let shown = if obj.status == S {
                obj.resource_points
            } else {
                0
            };
            let amount = shown.to_string();
            let amount_w = font_width(a, l.font, &amount);
            let amount_run = shadowed_run(
                a,
                l.font,
                &amount,
                WHITE,
                false,
                l.total_right - amount_w,
                y,
            );
            let dot_run = shadowed_run(a, l.font, &".".repeat(dots), color, false, dot_x, y);
            if k < revealed {
                assert!(
                    has_run(&dl.vertices, &amount_run),
                    "row {k} amount {amount}"
                );
                assert!(has_run(&dl.vertices, &dot_run), "row {k} dots");
            } else {
                assert!(
                    !has_run(&dl.vertices, &amount_run),
                    "row {k} is not revealed at {time} s"
                );
                assert!(
                    !has_run(&dl.vertices, &dot_run),
                    "row {k} dots are not revealed at {time} s"
                );
            }
        }
        let total_text = format_total(a, total);
        let total_x = l.total_right - font_width(a, l.font, &total_text);
        let total_run = shadowed_run(a, l.font, &total_text, WHITE, false, total_x, total_y);
        assert!(
            has_run(&dl.vertices, &total_run),
            "total {total_text} at {time} s"
        );
    }
}

/// The `from_fit` layout reproduces the table row positions above (1024-wide).
#[test]
fn shipped_layout_stacks_six_primaries_for_mc2_01() {
    let Some(mut vfs) = vfs() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let mut fit = ld.art_fit("buttonlayout1024").expect("fit");
    let panel = ObjectivesPanel::from_fit(&mut ld, &mut fit).expect("objectives block");
    // mc2_01 team 0: six primary objectives with ids 50001..50006 (mission file, team 0)
    let objs: Vec<ObjectiveView> = (0..6).map(|i| primary(50001 + i, U)).collect();
    let l = panel.listing(&objs);
    let ys: Vec<i32> = l.rows.iter().map(|r| r.y).collect();
    assert_eq!(ys, vec![102, 117, 132, 147, 162, 177]);
    assert_eq!(l.secondary_header_y, 202);
    assert_eq!(l.total_y, 217);
}

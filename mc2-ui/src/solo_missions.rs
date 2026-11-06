// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The solo mission list (`MPLoadMap::beginSingleMission`, `code/mploadmap.cpp`, over
//! `mcl_mp_loadmap.fit`): a dialog over the main menu that lists the single-player maps of
//! `data/missions` (`IsSinglePlayer`), shows the picked map's name, picture and blurb, and
//! returns the picked map's file name.
//!
//! The enter and leave animations of the dialog are not played: it opens and closes at once.

use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::logistics::briefing::load_map;
use crate::logistics_ids::button;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::widgets::{ListBox, LocalizedListItem};
use mc2_formats::ini::FitIni;
use mc2_platform::input::KeyStatus;
use mc2_platform::keys::{KEY_ESCAPE, KEY_RETURN};

/// `missionPath`.
const MISSION_PATH: &str = "data/missions/";
/// `IDS_MP_LM_MAP_LIST_MAP_NAME`: `"MAP NAME : %s"`.
const MAP_NAME_FORMAT: u32 = 26306;
/// The texts of `mcl_mp_loadmap.fit` the dialog fills (`textObjects[...]`).
const TEXT_MAX_PLAYERS: usize = 2;
const TEXT_MAP_NAME: usize = 3;
const TEXT_TYPE: usize = 4;
const TEXT_BLURB: usize = 5;
/// `statics[18]`: the picture of the picked map.
const MAP_PICTURE: usize = 18;

/// A single-player map: its file stem, the name the list shows and the blurb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoloMap {
    pub file: String,
    pub name: String,
    pub blurb: String,
}

/// The list dialog.
pub struct SoloMissions {
    pub screen: LogisticsScreen,
    pub list: ListBox,
    template: LocalizedListItem,
    /// One entry per list row (`setHiddenText(fileName)`).
    pub maps: Vec<SoloMap>,
    pub status: Status,
    open: bool,
    /// The row whose picture and texts are shown (`oldSel`).
    shown: i32,
}

/// What one update of the dialog did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoloUpdate {
    /// The picked row changed: the host loads its picture.
    pub picked: Option<usize>,
    /// The dialog closed with this answer.
    pub closed: Option<Status>,
}

/// `getMapNameFromFile`: the mission's display name.
fn display_name(assets: &UiAssets, fit: &mut FitIni, stem: &str) -> String {
    let name = if fit.seek_block("MissionSettings").is_ok() {
        if fit.boolean("MissionNameUseResourceString").unwrap_or(false) {
            fit.long("MissionNameResourceStringID")
                .map(|id| assets.string(id as u32).to_string())
        } else {
            fit.string("MissionName")
        }
    } else {
        None
    };
    name.filter(|n| !n.is_empty())
        .unwrap_or_else(|| stem.to_string())
}

/// The blurb of `updateMapInfo` (`Blurb2`, or its string resource).
fn blurb(assets: &UiAssets, fit: &mut FitIni) -> String {
    let _ = fit.seek_block("MissionSettings");
    if fit.boolean("Blurb2UseResourceString").unwrap_or(false) {
        if let Some(id) = fit.long("Blurb2ResourceStringID") {
            return assets.string(id as u32).to_string();
        }
    }
    fit.string("Blurb2").unwrap_or_default()
}

impl SoloMissions {
    pub fn load(ld: &mut Loader<'_>) -> Result<SoloMissions, String> {
        let mut screen = LogisticsScreen::load(ld, "mcl_mp_loadmap", Prefixes::default())?;
        for b in &mut screen.buttons {
            b.set_message_on_release();
        }
        let mut file = ld.art_fit("mcl_mp_loadmap_list0")?;
        file.seek_block("MapList").map_err(|e| e.to_string())?;
        let mut list = ListBox::from_current_block(ld, &file);
        list.set_orange();
        let template = LocalizedListItem::from_fit(ld, &mut file, "Text0");
        Ok(SoloMissions {
            screen,
            list,
            template,
            maps: Vec::new(),
            status: Status::Running,
            open: false,
            shown: -1,
        })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// `seedDialog(1)`: every mission file of `data/missions` whose `[MissionSettings]` says
    /// `IsSinglePlayer`, in file order; the first row is picked. (`seedFromCampaign` would add
    /// the campaign's played missions again, but the shipped campaign missions are all
    /// single-player maps already, so they would be listed twice.)
    pub fn begin(&mut self, ld: &mut Loader<'_>) {
        self.status = Status::Running;
        self.open = true;
        self.screen.begin();
        self.list.remove_all_items();
        self.maps.clear();
        let mut stems: Vec<String> = Vec::new();
        for name in ld.vfs.list_dir(MISSION_PATH.trim_end_matches('/')) {
            let Some(stem) = name
                .strip_prefix(MISSION_PATH)
                .and_then(|n| n.strip_suffix(".fit"))
            else {
                continue;
            };
            let Ok(Some(mut fit)) = ld.vfs.read_fit(&name) else {
                continue;
            };
            // `ul IsSinglePlayer` (`readIdULong`)
            let single = fit.seek_block("MissionSettings").is_ok()
                && fit.read_u32("IsSinglePlayer").is_ok_and(|v| v != 0);
            if single {
                stems.push(stem.to_string());
            }
        }
        for stem in stems {
            let path = format!("{MISSION_PATH}{stem}.fit");
            let Ok(Some(mut fit)) = ld.vfs.read_fit(&path) else {
                continue;
            };
            let name = display_name(ld.assets, &mut fit, &stem);
            let blurb = blurb(ld.assets, &mut fit);
            let mut row = self.template.clone();
            row.item.item.set_text(&name);
            row.item.item.size_to_text(ld.assets);
            row.item.item.obj.resize(
                self.list.obj.width() - self.list.scroll_bar_width() - 30.0,
                row.item.item.obj.height(),
            );
            row.hidden_text = stem.clone();
            self.list.add_item(Box::new(row));
            self.maps.push(SoloMap {
                file: stem,
                name,
                blurb,
            });
        }
        self.shown = -1;
        self.list.select_item(0);
        self.show_info(ld.assets);
    }

    /// `updateMapInfo`'s texts for the picked row (the picture comes from the host).
    fn show_info(&mut self, assets: &UiAssets) {
        let picked = usize::try_from(self.list.selected_item()).ok();
        let map = picked.and_then(|i| self.maps.get(i)).cloned();
        let set = |screen: &mut LogisticsScreen, i: usize, text: &str| {
            if let Some(t) = screen.texts.get_mut(i) {
                t.set_text(text);
            }
        };
        match &map {
            Some(map) => {
                let line = assets.string(MAP_NAME_FORMAT).replace("%s", &map.name);
                set(&mut self.screen, TEXT_MAP_NAME, &line);
                set(&mut self.screen, TEXT_TYPE, "");
                set(&mut self.screen, TEXT_MAX_PLAYERS, "");
                set(&mut self.screen, TEXT_BLURB, &map.blurb);
            }
            None => {
                for i in [TEXT_TYPE, TEXT_MAP_NAME, TEXT_MAX_PLAYERS, TEXT_BLURB] {
                    set(&mut self.screen, i, "");
                }
                if let Some(s) = self.screen.statics.get_mut(MAP_PICTURE) {
                    s.set_color(0, false);
                }
            }
        }
        self.shown = self.list.selected_item();
    }

    /// Gives the picked row's map picture to the panel (`statics[18].setTexture(getMissionTGA)`).
    pub fn show_picture(&mut self, ld: &mut Loader<'_>, row: usize) {
        let Some(map) = self.maps.get(row) else {
            return;
        };
        let picture = load_map(ld, &map.file);
        if let Some(s) = self.screen.statics.get_mut(MAP_PICTURE) {
            let (tex, width) = picture.map_or((None, 0), |(tex, width)| (Some(tex), width));
            s.set_texture(tex, width);
            s.set_uvs(0.0, 127.0, 127.0, 0.0);
            s.set_color(0xffff_ffff, true);
        }
    }

    /// The file stem of the picked map (`getMapFileName`).
    pub fn picked_file(&self) -> Option<&str> {
        let i = usize::try_from(self.list.selected_item()).ok()?;
        self.maps.get(i).map(|m| m.file.as_str())
    }

    /// `MPLoadMap::update`: the buttons (Yes picks, No cancels), the list, Enter and Escape.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> SoloUpdate {
        let mut out = SoloUpdate::default();
        if !self.open {
            return out;
        }
        let mut msgs = self.screen.update(ctx);
        self.list.update(ctx);
        if self.list.selected_item() != self.shown {
            self.show_info(ctx.assets);
            out.picked = usize::try_from(self.list.selected_item()).ok();
        }
        if ctx.input.key_status(KEY_RETURN) == KeyStatus::Released {
            msgs.push(crate::Msg::new(0, button::YES));
        }
        if ctx.input.key_status(KEY_ESCAPE) == KeyStatus::Released {
            msgs.push(crate::Msg::new(0, button::NO));
        }
        for m in msgs {
            let answer = match m.who {
                button::YES if self.picked_file().is_some() => Status::Yes,
                button::NO => Status::No,
                _ => continue,
            };
            self.status = answer;
            self.open = false;
            out.closed = Some(answer);
            break;
        }
        out
    }

    /// The dimmed backdrop, then the dialog (`MPLoadMap::render`).
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.open {
            return;
        }
        let (w, h) = ctx.screen();
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), 0x7f00_0000);
        self.list.render(ctx);
        self.screen.render(ctx);
    }
}

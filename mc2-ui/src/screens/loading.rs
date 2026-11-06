// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission loading screen (`code/loadscreen.cpp`): FIT background panels, loading bar and
//! progress/waiting state. The original draws its bar from two TGA files in `LoadingBar` and
//! moves the FIT animation objects through their directional intro/outro animations.

use crate::anim::Animation;
use crate::ctx::{Loader, RenderCtx, UpdateCtx};
use crate::draw::{Blend, GuiRect, Vertex};
use crate::fit::FitExt;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::textures::TexId;
use crate::widgets::Text;
use mc2_formats::ini::FitIni;

/// Loading progress and presentation state. `ReadyToLoad` means the entrance animation has
/// finished and the app may begin loading mission data; `Done` means progress reached 100%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadingStatus {
    #[default]
    Entering,
    ReadyToLoad,
    Loading,
    Done,
}

pub struct LoadingScreen {
    pub screen: LogisticsScreen,
    pub status: LoadingStatus,
    pub progress: f32,
    pub waiting_for_players: bool,
    pub progress_background: Option<TexId>,
    pub progress_fill: Option<TexId>,
    pub waiting_texture: Option<TexId>,
    progress_rect: GuiRect,
    wait_position: (i32, i32),
    loading_text: Text,
    incoming: [Animation; 5],
    outgoing: [Animation; 5],
    object_animation: Vec<usize>,
    outro_started: bool,
}

impl LoadingScreen {
    /// Loads `mcl_loadingscreen` and the bar textures named by its `LoadingBar` block.
    pub fn load(ld: &mut Loader<'_>) -> Result<Self, String> {
        let file = ld.art_fit("mcl_loadingscreen")?;
        Self::load_fit(ld, file)
    }

    /// Loads the resolution-specific FIT selected by `LoadScreenWrapper::changeRes`.
    pub fn load_for_resolution(ld: &mut Loader<'_>, width: i32) -> Result<Self, String> {
        let mut file = None;
        for candidate in [640, 1024, 1280, 1600, 1920] {
            if candidate < width {
                continue;
            }
            if let Ok(fit) = ld.art_fit(&format!("mcl_loadingscreen_{candidate}")) {
                file = Some(fit);
                break;
            }
        }
        let file = match file {
            Some(fit) => fit,
            None => ld
                .art_fit("mcl_loadingscreen_640")
                .or_else(|_| ld.art_fit("mcl_loadingscreen"))?,
        };
        Self::load_fit(ld, file)
    }

    fn load_fit(ld: &mut Loader<'_>, mut file: FitIni) -> Result<Self, String> {
        let screen = LogisticsScreen::from_fit(ld, &mut file, Prefixes::default());
        let loading_text = Text::from_fit(ld, &mut file, "AnimObject18");
        let incoming = animations(&mut file, "In");
        let outgoing = animations(&mut file, "Out");
        let mut progress_rect = GuiRect::default();
        let mut wait_position = (0, 0);
        let (mut fill, mut background, mut waiting) = (None, None, None);
        if file.seek_block("LoadingBar").is_ok() {
            let x = file.long_or("XLocation", 0) as i32;
            let y = file.long_or("YLocation", 0) as i32;
            let fill_name = file.string("FileName").unwrap_or_default();
            let background_name = file.string("BackgroundFileName").unwrap_or_default();
            fill = ld.art(&fill_name);
            background = ld.art(&background_name);
            let (w, h) = background
                .and_then(|id| ld.assets.textures.get(id))
                .map_or((0, 0), |t| (t.width as i32, t.height as i32));
            progress_rect = GuiRect::new(x, y, x + w, y + h);
        }
        if file.seek_block("WaitImage").is_ok() {
            let x = file.long_or("XLocation", 0) as i32;
            let y = file.long_or("YLocation", 0) as i32;
            let name = file.string("FileName").unwrap_or_default();
            waiting = ld.art(&name);
            wait_position = (x, y);
        }
        let mut object_animation = Vec::with_capacity(screen.anim_objects.len());
        for i in 0..screen.anim_objects.len() {
            let name = if file.seek_block(&format!("AnimObject{i}")).is_ok() {
                file.string("AnimationOut").unwrap_or_default()
            } else {
                String::new()
            };
            object_animation.push(animation_index(&name));
        }
        Ok(Self {
            screen,
            status: LoadingStatus::Entering,
            progress: 0.0,
            waiting_for_players: false,
            progress_background: background,
            progress_fill: fill,
            waiting_texture: waiting,
            progress_rect,
            wait_position,
            loading_text,
            incoming,
            outgoing,
            object_animation,
            outro_started: false,
        })
    }

    /// Starts the directional panel animations and resets progress as `LoadScreen::begin` does.
    /// The waiting art is selected by progress (`loadProgress == 1000`) in the original, so a
    /// reused screen must drop out of the waiting state here as well.
    pub fn begin(&mut self) {
        self.progress = 0.0;
        self.waiting_for_players = false;
        self.status = LoadingStatus::Entering;
        self.outro_started = false;
        for (object, &index) in self
            .screen
            .anim_objects
            .iter_mut()
            .zip(&self.object_animation)
        {
            object.anim = self.incoming[index].clone();
            object.anim.begin();
        }
    }

    pub fn set_progress(&mut self, percent: f32) {
        self.progress = percent.clamp(0.0, 100.0);
        // Progress may arrive while the entrance is still animating. Preserve that
        // state so update can still issue the READYTOLOAD handoff.
        if self.status == LoadingStatus::ReadyToLoad {
            self.status = LoadingStatus::Loading;
        }
    }

    pub fn set_waiting_for_players(&mut self, waiting: bool) {
        self.waiting_for_players = waiting;
    }

    /// Advances screen widgets and marks the load transition ready once the entrance panels end.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Status {
        self.screen.update(ctx);
        if self.status == LoadingStatus::Entering
            && self.screen.anim_objects.iter().all(|o| o.is_done())
        {
            self.status = LoadingStatus::ReadyToLoad;
        }
        if self.outro_started
            && self.progress >= 99.0
            && self.screen.anim_objects.iter().all(|o| o.is_done())
        {
            self.status = LoadingStatus::Done;
            Status::Next
        } else if self.status == LoadingStatus::ReadyToLoad {
            Status::ReadyToLoad
        } else {
            Status::Running
        }
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.screen.render(ctx);
        if let Some(object) = self.screen.anim_objects.first() {
            self.loading_text
                .render_at(ctx, object.anim.x_delta(), object.anim.y_delta());
        }
        if self.waiting_for_players {
            if let Some(id) = self.waiting_texture {
                let Some(image) = ctx.assets.textures.get(id) else {
                    return;
                };
                let (x, y) = self.wait_position;
                textured_image(
                    ctx,
                    id,
                    GuiRect::new(x, y, x + image.width as i32, y + image.height as i32),
                    1.0,
                );
            }
            return;
        }
        if let Some(id) = self.progress_background {
            textured_image(ctx, id, self.progress_rect, 1.0);
        }
        if let Some(id) = self.progress_fill {
            let width = (self.progress_rect.width() as f32 * self.progress / 100.0) as i32;
            if width > 0 {
                textured_image(
                    ctx,
                    id,
                    GuiRect::new(
                        self.progress_rect.left,
                        self.progress_rect.top,
                        self.progress_rect.left + width,
                        self.progress_rect.bottom,
                    ),
                    self.progress / 100.0,
                );
            }
        }
    }

    /// Starts the outgoing animation associated with each panel's `AnimationOut` in the FIT.
    pub fn begin_outro(&mut self) {
        self.outro_started = true;
        self.status = LoadingStatus::Loading;
        for (object, &index) in self
            .screen
            .anim_objects
            .iter_mut()
            .zip(&self.object_animation)
        {
            object.anim = self.outgoing[index].clone();
            object.anim.begin();
        }
    }
}

fn animations(file: &mut FitIni, direction: &str) -> [Animation; 5] {
    let names = [
        format!("AnimationTop{direction}"),
        format!("AnimationBottom{direction}"),
        format!("AnimationLeft{direction}"),
        format!("AnimationRight{direction}"),
        format!("AnimationTop2{direction}"),
    ];
    names.map(|name| Animation::from_block(file, &name))
}

fn animation_index(name: &str) -> usize {
    if name.contains('2') {
        4
    } else if name.contains("Top") {
        0
    } else if name.contains("Bottom") {
        1
    } else if name.contains("Left") {
        2
    } else {
        3
    }
}

fn textured_image(ctx: &mut RenderCtx<'_>, texture: TexId, rect: GuiRect, u_right: f32) {
    let (l, t, r, b) = (
        rect.left as f32,
        rect.top as f32,
        rect.right as f32,
        rect.bottom as f32,
    );
    let v = [
        Vertex {
            x: l,
            y: t,
            u: 0.0,
            v: 0.0,
            argb: u32::MAX,
        },
        Vertex {
            x: l,
            y: b,
            u: 0.0,
            v: 1.0,
            argb: u32::MAX,
        },
        Vertex {
            x: r,
            y: b,
            u: u_right,
            v: 1.0,
            argb: u32::MAX,
        },
        Vertex {
            x: r,
            y: t,
            u: u_right,
            v: 0.0,
            argb: u32::MAX,
        },
    ];
    ctx.dl.quad(Some(texture), Blend::Alpha, v);
}

// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Logistics screens between two missions (lane U2a): mission selection, briefing, mech bay,
//! pilot ready room, and the screen-sequence controller that moves between them.
//!
//! The screens keep their state in `mc2-campaign` ([`Logistics`]) and only read or change it
//! through that crate's API; each `begin`, `update` and `handle_message` takes the campaign as a
//! parameter, as the C++ reads the `LogisticsData::instance` global.
//!
//! Screens that other lanes own (purchase, mech lab, loading) plug in through [`Page`]; until
//! they land, [`Placeholder`] stands in for them.

use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::font::FontRef;
use crate::screen::{LogisticsScreen, Status};
use crate::widgets::TextListItem;
use mc2_campaign::{Logistics, MechKey, VariantKey};
use mc2_sim_data::campaign::DisplayText;

pub mod attribute_meter;
pub mod briefing;
pub mod callout;
pub mod loadout;
pub mod mech_bay;
pub mod mech_display;
pub mod mech_icon;
pub mod mech_list;
pub mod mission_select;
pub mod movie;
pub mod pilot_list;
pub mod pilot_ready;
pub mod script;
pub mod sequence;

/// Button and message ids (`mechbayscreen.h`, `missionbriefingscreen.h`, `missionselectionscreen.h`,
/// `pilotreadyscreen.cpp`). A widget sends its id to its screen's `handleMessage`.
pub mod ids {
    pub const MB_MSG_NEXT: i32 = 50;
    pub const MB_MSG_PREV: i32 = 51;
    pub const MB_MSG_ADD: i32 = 52;
    pub const MB_MSG_REMOVE: i32 = 53;
    pub const MB_MSG_BUY: i32 = 54;
    pub const MB_MSG_CHANGE_LOADOUT: i32 = 55;
    pub const MB_MSG_BUY_SELL: i32 = 56;
    pub const MB_MSG_MAINMENU: i32 = 57;
    pub const MN_MSG_PAUSE: i32 = 81;
    pub const MN_MSG_PLAY: i32 = 80;
    pub const MN_MSG_STOP: i32 = 82;
    /// `MSG_FIRST_MISSION`: the operation's mission buttons are `100 + index`.
    pub const MSG_FIRST_MISSION: i32 = 100;
    pub const PR_MSG_ADD: i32 = 62;
    pub const PR_MSG_REMOVE: i32 = 63;
    pub const PR_MSG_MAIN: i32 = 64;
}

/// The string ids the logistics screens print (`resource.h`, `test_scripts/res_conv`).
pub mod strings {
    pub const MN_LB_FONT: u32 = 27517;
    pub const MN_DIVIDER: u32 = 27516;
    pub const MN_MISSION_OBJECTIVES: u32 = 27518;
    pub const NOPILOT: u32 = 45032;
    /// `IDS_GREEN`: the ranks are `GREEN + rank` for ranks 0 to 3; `ACE` is rank 4.
    pub const GREEN: u32 = 30112;
    pub const ACE: u32 = 30192;
    /// `IDS_SPECIALTY`: the specialty skills are `SPECIALTY + index`.
    pub const SPECIALTY: u32 = 41001;
    /// `IDS_SPECIALTY_HELP1`: the skill help texts are `SPECIALTY_HELP1 + index`.
    pub const SPECIALTY_HELP1: u32 = 41030;
    /// `IDS_HELP_MEDAL1`: the medal help texts are `HELP_MEDAL1 + index`.
    pub const HELP_MEDAL1: u32 = 40101;
    /// `IDS_HELP_DEPLOYTEAMMW`: the help text of the deployment slots.
    pub const HELP_DEPLOY_TEAM: u32 = 29021;
    /// `IDS_MB_MECH_WEIGHT`, `IDS_MB_WEIGHT`.
    pub const MB_MECH_WEIGHT: u32 = 27132;
    pub const MB_WEIGHT: u32 = 27131;
}

/// Sound ids (`mclib/sounds.h`) the screens use besides those in [`crate::ctx::sfx`].
pub const LOG_NEXTBACKBUTTONS: i32 = 93;

/// A screen of the logistics sequence: what the controller drives (`LogisticsScreen` in the C++).
pub trait Page {
    /// `begin`: called when the sequence moves onto the screen.
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics);
    /// `end`: called when the sequence moves off the screen.
    fn end(&mut self, lg: &mut Logistics);
    /// `update`: one frame. The screen's status says whether the sequence should move on.
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics);
    /// `render`: one frame, at rest (the sequence's slide is not drawn).
    fn render(&mut self, ctx: &mut RenderCtx<'_>);
    /// `beginFadeIn(t)`: the screen fades in over `t` seconds at its next render.
    fn begin_fade_in(&mut self, t: f32);
    /// `getStatus`.
    fn status(&self) -> Status;
    /// Lets the owner of the sequence set the status (the purchase and lab screens report back
    /// through it).
    fn set_status(&mut self, status: Status);
    /// The page's layout (its buttons and parts).
    fn base(&self) -> &LogisticsScreen;
    /// The page's layout, for the tutorial's callout (`getButton`, `getRect`).
    fn base_mut(&mut self) -> &mut LogisticsScreen;
    /// The page as its concrete type, for the app and the tests.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Whether the screen is wired up. A screen that is not is never begun: the sequence reports
    /// it as [`sequence::Flow::Unavailable`] instead.
    fn is_available(&self) -> bool {
        true
    }
    /// The error that stopped the screen from loading (a missing layout or mission file). The
    /// sequence stops on it, rather than moving on as if the screen had worked.
    fn fault(&self) -> Option<&str> {
        None
    }
    /// The app's movie host now drains [`Page::take_movie_requests`]: the movie controls send
    /// requests from here on. Until it is connected a page with a movie reports the movie as
    /// unavailable, as before the movie host existed. Called once, before the first `begin`.
    fn connect_movie_host(&mut self) {}
    /// The movie requests the page made since the last call, in order (`begin` opens the mission's
    /// movie; the movie controls play, pause and stop it). The app's movie host plays them. Only a
    /// page whose host is connected makes requests.
    fn take_movie_requests(&mut self) -> Vec<movie::MovieRequest> {
        Vec::new()
    }
    /// A part of the screen the player used that is not wired up. The sequence reports it once as
    /// [`sequence::Flow::Unavailable`]; the screen itself carries on. The mission selection reports
    /// its movie this way while no movie host is connected.
    fn take_unavailable(&mut self) -> Option<&'static str> {
        None
    }
}

/// Stands in for a screen another lane owns. [`Placeholder::unavailable`] is the screen the
/// sequence reports as unavailable; [`Placeholder::new`] runs until its status is set, so the
/// sequence can be driven past it.
pub struct Placeholder {
    pub base: LogisticsScreen,
    pub name: &'static str,
    available: bool,
    fault: Option<String>,
}

impl Placeholder {
    /// A stand-in that runs until its status is set (a test double for a screen).
    pub fn new(name: &'static str) -> Placeholder {
        Placeholder {
            base: LogisticsScreen::default(),
            name,
            available: true,
            fault: None,
        }
    }

    /// A stand-in whose layout failed to load: it reports `fault` from the start.
    pub fn failing(name: &'static str, fault: &str) -> Placeholder {
        Placeholder {
            fault: Some(fault.to_string()),
            ..Placeholder::new(name)
        }
    }

    /// Makes the stand-in fail from now on (a fault raised while it runs).
    pub fn fail(&mut self, fault: &str) {
        self.fault = Some(fault.to_string());
    }

    /// A screen that is not wired up yet: the sequence never begins it.
    pub fn unavailable(name: &'static str) -> Placeholder {
        Placeholder {
            available: false,
            ..Placeholder::new(name)
        }
    }
}

impl Page for Placeholder {
    fn begin(&mut self, _ld: &mut Loader<'_>, _lg: &mut Logistics) {
        self.base.begin();
    }
    fn end(&mut self, _lg: &mut Logistics) {}
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _lg: &mut Logistics) {
        self.base.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.base.render(ctx);
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
    fn begin_fade_in(&mut self, t: f32) {
        self.base.begin_fade_in(t);
    }
    fn status(&self) -> Status {
        self.base.get_status()
    }
    fn set_status(&mut self, status: Status) {
        self.base.status = status;
    }
    fn base(&self) -> &LogisticsScreen {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.base
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The text of a campaign display name: a literal, or the string-table entry it names.
pub fn display_text(assets: &UiAssets, text: Option<&DisplayText>) -> String {
    match text {
        None => String::new(),
        Some(DisplayText::Literal(s)) => s.clone(),
        Some(DisplayText::Resource(id)) => assets.string(*id).to_string(),
    }
}

/// `addItem`/`addLBItem` of the briefing and selection list boxes: a text entry `width` pixels
/// wide (the list box less its scroll bar and a margin), in `font`.
pub fn text_item(
    assets: &UiAssets,
    font: FontRef,
    width: f32,
    text: &str,
    color: u32,
) -> TextListItem {
    let mut item = TextListItem::new(assets, font);
    let h = item.obj.height();
    item.obj.resize(width, h);
    item.set_text(text);
    item.set_color(color);
    item
}

/// The variant of a mech in the inventory.
pub(crate) fn variant_of(lg: &Logistics, mech: MechKey) -> VariantKey {
    lg.mech(mech).map_or(0, |m| m.variant)
}

/// The chassis picture index of a variant (the mech icon's cell).
pub(crate) fn chassis_icon(lg: &Logistics, variant: VariantKey) -> Option<i64> {
    lg.chassis_of(variant).map(|c| c.icon_picture_index)
}

/// Whether two rectangles overlap (`aObject::rectIntersect`).
pub(crate) fn rect_overlaps(a: GuiRect, b: GuiRect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// `sprintf` for the screens' strings: fills each `%...` conversion with the next argument.
pub(crate) fn fill(fmt: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // skip the conversion: flags, width, precision, length, then the letter
        while let Some(&n) = chars.peek() {
            chars.next();
            if n.is_ascii_alphabetic() && n != 'l' {
                break;
            }
        }
        if let Some(a) = args.next() {
            out.push_str(a);
        }
    }
    out
}

/// `IDS_MB_WEIGHT`: the drop weight now and at most.
pub(crate) fn format_pair(fmt: &str, now: i64, max: i64) -> String {
    fill(fmt, &[now.to_string(), max.to_string()])
}

/// `IDS_MB_MECH_WEIGHT`: the mech's tons and its weight class.
pub(crate) fn format_weight_class(fmt: &str, tons: f32, class: &str) -> String {
    fill(fmt, &[format!("{tons:.0}"), class.to_string()])
}

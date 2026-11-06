// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GameBindings` for the simulation: the ABL natives of `code/ablmc2.cpp` over the [`World`](crate::World). The 230
//! registrations fold into 222 callbacks. 216 have real bodies (`ai::natives::IMPLEMENTED`), including `getfixed` and
//! `orderdeployelementals` (a faithful silent no-op: its C++ body is under `#ifdef USE_ELEMENTALS`), and some return the C++'s
//! fixed values. 3 logistics natives (`logistics_screen_id`, `logistics_animation_callout`, `logistics_in_callout`) are served by
//! mc2-ui's `BrainBindings` (`mc2-ui/src/logistics/script.rs:33`). The other 11 keep the trait default (0 or false) and report
//! [`AiUnavailable::Native`](crate::ai::AiUnavailable::Native) when called: `break`, `calcpartid`, `damageobject`,
//! `getrepairstate`, `isserver`, `ordercapture`, `orderloadelementals`, `orderrefit`, `repair`, `setsalvagestatus`,
//! `wasevercapturable`.
//!
//! Group ownership (counts from the C++ survey): mission globals/timers/objectives/triggers/strikes (31, lane E), spawn
//! lifecycle (6, E+A), object queries/setters (31, A), terrain (4, A), capture (5, A/D), movers/paths (9, B), weapons (10, C),
//! pilot/brain state (22, D), contacts/targets/sensors (23, D), tactical orders (25, D/B), stubs that return fixed values
//! because their `#ifdef` bodies are not compiled (9), sound/speech (11, which push [`SimEvent`](crate::events::SimEvent)
//! requests from `natives_audio.rs`), camera (`SetCamera*`/`GetCamera*`, see below), and movie, tutorial, callout, and GUI
//! presentation (`natives_presentation`). Presentation output uses [`SimEvent`](crate::events::SimEvent) requests, while the sim
//! retains the state scripts query (`ForceMovieEnd`, `InCallout`, and `GUIIsAOEStyle`).
//!
//! ABL runs inside the update (pilot brains inside `pilot_update`, the mission brain after collisions). The `Env` is taken out
//! of the world for the duration of a call (`Option::take`) so the bindings can borrow the world mutably.

use super::natives_presentation;
use super::MAX_GLOBAL_MISSION_VALUES;
use crate::events::CameraRequest;

/// The bindings object handed to `mc2_abl::Env::execute`.
pub struct SimBindings<'w> {
    /// The world the scripts act on.
    pub world: &'w mut crate::World,
    /// `CurObject`, `CurWarrior`, `CurContact`, `CurGroup`, `CurAlarm`, `CurObjectClass` (ablmc2.cpp:111-118): set by the caller
    /// before each `execute`; natives given id -1 use `cur_object`.
    pub cur_object: crate::ids::WatchId,
    /// `CurWarrior` (appended by lane D): the pool slot of the pilot whose brain is running, `None` for the mission script.
    pub cur_warrior: Option<u16>,
    /// `CurContact` (appended by lane D): the contact `selectContact` picked.
    pub cur_contact: Option<crate::ids::ObjectHandle>,
    /// `CurAlarm` (appended by lane D): the alarm whose handler is running.
    pub cur_alarm: i32,
}

impl<'w> SimBindings<'w> {
    /// Bindings for the mission script (no pilot context).
    pub fn mission(world: &'w mut crate::World) -> SimBindings<'w> {
        SimBindings {
            world,
            cur_object: crate::ids::WatchId::NONE,
            cur_warrior: None,
            cur_contact: None,
            cur_alarm: 0,
        }
    }

    /// Bindings for the brain of the pilot in `slot` (`runBrain` / `checkAlarms` set `CurObject = getVehicle()`,
    /// `CurWarrior = this`, `CurContact = NULL`).
    pub fn pilot(world: &'w mut crate::World, slot: u16) -> SimBindings<'w> {
        let cur_object = world
            .pilots
            .get(slot)
            .map_or(crate::ids::WatchId::NONE, |p| p.mover);
        SimBindings {
            world,
            cur_object,
            cur_warrior: Some(slot),
            cur_contact: None,
            cur_alarm: 0,
        }
    }
}

/// `execCallStrike[Ex]` and `IfaceCallStrike` (ablmc2.cpp:3993, artlry.cpp:159).
fn script_strike(
    b: &mut SimBindings<'_>,
    strike: i32,
    target: i32,
    mut at: crate::math::Vec3,
    clan: bool,
    seconds: f32,
) {
    let handle = if target == -1 {
        b.world.objects.by_watch_id(b.cur_object)
    } else {
        b.world.objects.find_by_part_id(crate::ids::PartId(target))
    };
    if let Some(position) = handle.and_then(|h| b.world.objects.get(h).map(|o| o.c.position)) {
        at = position;
    } else {
        at.z = b.world.terrain.elevation(&at);
    }
    let random = clan && !crate::object::hooks::team_line_of_sight(b.world, 1, at, 0.0);
    let Some(strike) = crate::combat::artillery_math::normalize_strike(strike, random) else {
        return;
    };
    // C++ uses commander -1 for non-clan scripted strikes, then dereferences it in
    // CallArtillery. Use the home commander instead of reproducing that invalid access.
    let commander = if clan { 1 } else { 0 };
    crate::combat::artillery::call_strike_seconds(
        b.world,
        commander,
        strike,
        at,
        crate::combat::artillery_math::seconds_from_time_to_impact(seconds),
    );
}

impl mc2_abl::GameBindings for SimBindings<'_> {
    fn set_revealed(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _team_id: i32,
        _distance: f32,
        _coord_list: mc2_abl::ArrayRef,
    ) {
        // Terrain::markRadiusSeen immediately returns in the shipped C++
        // (terrain.cpp:1226-1229); the native still forces every team's sensors to scan.
        crate::ai::sensors::force_scan_all(self.world);
    }
    fn set_salvage(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _object_id: i32,
        _component: i32,
    ) -> bool {
        // USE_SALVAGE is disabled (ablmc2.cpp:3585-3608).
        false
    }
    fn get_salvage(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        _object_id: i32,
        item_count: i32,
        items: mc2_abl::ArrayRef,
        quantities: mc2_abl::ArrayRef,
    ) {
        // USE_SALVAGE is disabled in the shipped C++ build (ablmc2.cpp:3729-3754).
        let count = (item_count.max(0) as usize)
            .min(ctx.len_bytes(items).max(ctx.len_bytes(quantities)) / 4);
        for i in 0..count {
            ctx.set_i32(items, i, -1);
            ctx.set_i32(quantities, i, -1);
        }
    }

    fn path_exists(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _mover_id: i32,
        start_row: i32,
        start_col: i32,
        goal_row: i32,
        goal_col: i32,
    ) -> i32 {
        let gates = crate::object::GateSnapshot::from_store(&self.world.objects);
        let global = &mut self.world.terrain.move_data_mut().global[0];
        let start = global.calc_area(start_row, start_col);
        let goal = global.calc_area(goal_row, goal_col);
        global.get_path_cost(
            &mut mc2_path::PathContext::new(),
            start,
            goal,
            false,
            &mut 0,
            true,
            &gates,
        )
    }

    fn object_change_sides(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, team: i32) {
        crate::mission::natives_object::change_sides(self, id, team)
    }
    fn object_suicide(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) {
        crate::mission::natives_object::object_suicide(self, id)
    }
    fn object_create(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::object_create(self, id)
    }
    fn object_remove(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::object_remove(self, id)
    }
    fn object_commander(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::object_commander(self, id)
    }
    fn object_visible(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, from: i32, to: i32) -> i32 {
        crate::mission::natives_object::object_visible(self, from, to)
    }
    fn in_area(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        id: i32,
        center: mc2_abl::ArrayRef,
        radius: f32,
        min_in: i32,
    ) -> bool {
        let center = crate::math::Vec3 {
            x: ctx.get_f32(center, 0),
            y: ctx.get_f32(center, 1),
            z: ctx.get_f32(center, 2),
        };
        crate::mission::natives_object::in_area(self, id, center, radius, min_in)
    }
    fn set_sensor_range(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, range: f32) {
        crate::mission::natives_object::set_sensor_range(self, id, range)
    }
    fn create_infantry(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        team: i32,
        commander: i32,
        position: mc2_abl::ArrayRef,
        building_part_id: i32,
    ) -> i32 {
        crate::mission::natives_object::create_infantry(
            self,
            ctx,
            team,
            commander,
            position,
            building_part_id,
        )
    }
    fn set_object_damage(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        id: i32,
        percentage: i32,
    ) -> i32 {
        crate::mission::natives_object::set_object_damage(self, id, percentage)
    }
    fn get_home_team(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::mission::natives_object::get_home_team(self)
    }
    fn set_animation(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        state: i32,
        sub_state: i32,
    ) {
        crate::mission::natives_object::set_animation(self, object_id, state, sub_state)
    }
    fn set_building_name(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        new_name: i32,
    ) {
        crate::mission::natives_object::set_building_name(self, object_id, new_name)
    }
    fn add_prisoner(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, _prison_id: i32) -> i32 {
        crate::mission::natives_object::add_prisoner()
    }
    fn end_movie_mode(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) {
        natives_presentation::end_movie_mode(self.world)
    }
    fn play_video(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        file_name: mc2_abl::ArrayRef,
    ) -> i32 {
        natives_presentation::play_video(self.world, ctx.string(file_name))
    }
    fn set_movie_mode(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) {
        natives_presentation::set_movie_mode(self.world)
    }
    fn fade_to_color(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        fade_color: i32,
        fade_time: f32,
    ) {
        natives_presentation::fade_to_color(self.world, fade_color, fade_time)
    }
    fn force_movie_end(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        natives_presentation::force_movie_end(self.world)
    }
    fn animation_callout(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        button_id: i32,
        is_button: bool,
        is_pressed: bool,
        time_to_scroll: f32,
        num_flashes: i32,
    ) -> bool {
        natives_presentation::animation_callout(
            self.world,
            button_id,
            is_button,
            is_pressed,
            time_to_scroll,
            num_flashes,
        )
    }
    fn tutorial_text(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, text_id: i32) {
        natives_presentation::tutorial_text(self.world, text_id)
    }
    fn gui_is_aoe_style(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        natives_presentation::gui_is_aoe_style(self.world)
    }
    fn freeze_gui(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, gui_flag: bool) {
        natives_presentation::freeze_gui(self.world, gui_flag)
    }
    fn convert_coords(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        kind: i32,
        world: mc2_abl::ArrayRef,
        cell: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::mission::natives_object::convert_coords(ctx, self, kind, world, cell)
    }
    fn get_relative_position_to_point(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        point: mc2_abl::ArrayRef,
        angle: f32,
        distance: f32,
        flags: i32,
        output: mc2_abl::ArrayRef,
    ) {
        crate::mission::natives_object::relative_position_to_point(
            self, ctx, point, angle, distance, flags, output,
        )
    }
    fn get_relative_position_to_object(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        angle: f32,
        distance: f32,
        flags: i32,
        output: mc2_abl::ArrayRef,
    ) {
        crate::mission::natives_object::relative_position_to_object(
            self, ctx, object_id, angle, distance, flags, output,
        )
    }
    fn get_unit_status(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> f32 {
        crate::mission::natives_object::unit_status(self, object_id)
    }
    fn lock_gate_open(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) {
        crate::mission::natives_object::lock_gate(self, id, Some(true))
    }
    fn lock_gate_closed(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) {
        crate::mission::natives_object::lock_gate(self, id, Some(false))
    }
    fn release_gate_lock(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) {
        crate::mission::natives_object::lock_gate(self, id, None)
    }
    fn is_gate_open(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, _id: i32) -> bool {
        let _ = ctx;
        crate::mission::natives_object::is_gate_open(self, _id)
    }
    fn get_general_alarm(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::mission::natives_object::get_general_alarm(self)
    }
    fn set_general_alarm(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, alarm: i32) {
        crate::mission::natives_object::set_general_alarm(self, alarm)
    }
    fn get_sensors_working(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::sensors_working(self, id)
    }
    fn set_tonnage(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, tonnage: f32) {
        crate::mission::natives_object::set_tonnage(self, id, tonnage)
    }
    fn set_explosion_damage(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, damage: f32) {
        crate::mission::natives_object::set_explosion_damage(self, id, damage)
    }
    fn set_explosion_radius(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, radius: f32) {
        crate::mission::natives_object::set_explosion_radius(self, id, radius)
    }
    fn get_object_dmg_pts(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::object_damage_points(self, id)
    }
    fn get_object_max_dmg(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::object_max_damage(self, id)
    }
    fn get_pilot_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::pilot_id(self, id)
    }
    fn get_pilot_wounds(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> f32 {
        crate::mission::natives_object::pilot_wounds(self, id)
    }
    fn set_pilot_wounds(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, wounds: i32) {
        crate::mission::natives_object::set_pilot_wounds(self, id, wounds)
    }
    fn get_armor_pts(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::armor_points(self, id, false)
    }
    fn get_max_armor(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32) -> i32 {
        crate::mission::natives_object::armor_points(self, id, true)
    }
    fn set_current_br_value(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, id: i32, value: i32) {
        crate::mission::natives_object::set_current_br_value(self, id, value)
    }
    fn set_object_active(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        id: i32,
        active: bool,
    ) -> i32 {
        crate::mission::natives_object::set_object_active(self, id, active)
    }
    /// `execPlayDigitalMusic` (ablmc2.cpp:2606-2620), delegated to the mission audio native.
    fn play_digital_music(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, sound_num: i32) -> i32 {
        crate::mission::natives_audio::play_digital_music(self, sound_num)
    }

    fn stop_music(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::mission::natives_audio::stop_music(self)
    }
    fn play_sound_effect(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, sound_num: i32) -> i32 {
        crate::mission::natives_audio::play_sound_effect(self, sound_num)
    }
    fn set_radio(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        warrior_index: i32,
        on: bool,
    ) -> i32 {
        crate::mission::natives_audio::set_radio(self, warrior_index, on)
    }
    fn play_speech(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        warrior_index: i32,
        message: i32,
    ) -> i32 {
        crate::mission::natives_audio::play_speech(self, warrior_index, message)
    }
    fn play_betty(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, message_index: i32) -> i32 {
        crate::mission::natives_audio::play_betty(self, message_index)
    }
    fn get_current_music_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::mission::natives_audio::get_current_music_id(self)
    }
    fn get_mission_tune_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::mission::natives_audio::get_mission_tune_id(self)
    }
    fn is_playing_voice_over(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        crate::mission::natives_audio::is_playing_voice_over(self)
    }
    fn stop_voice_over(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) {
        crate::mission::natives_audio::stop_voice_over(self)
    }
    fn in_callout(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        natives_presentation::in_callout(self.world)
    }
    fn player_in_combat(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        crate::mission::natives_audio::get_player_in_combat(self)
    }

    fn call_strike(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        strike_type: i32,
        target_id: i32,
        x: f32,
        y: f32,
        z: f32,
        clan_strike: bool,
    ) {
        let target_exists = if target_id == -1 {
            self.world.objects.by_watch_id(self.cur_object).is_some()
        } else {
            self.world
                .objects
                .find_by_part_id(crate::ids::PartId(target_id))
                .is_some()
        };
        script_strike(
            self,
            strike_type,
            target_id,
            crate::math::Vec3 { x, y, z },
            clan_strike && !target_exists,
            0.0,
        );
    }

    fn call_strike_ex(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        strike_type: i32,
        target_id: i32,
        x: f32,
        y: f32,
        z: f32,
        clan_strike: bool,
        time_to_impact: f32,
    ) {
        script_strike(
            self,
            strike_type,
            target_id,
            crate::math::Vec3 { x, y, z },
            clan_strike,
            time_to_impact.max(0.0),
        );
    }

    // ---- lane D natives (src/ai/natives.rs) -------------------------------------------------------------------------------

    fn after_native(&mut self, env: &mut mc2_abl::Env) {
        crate::ai::brain::run_nested(self.world, env);
    }

    fn native_called(&mut self, id: u16) {
        let mover = self
            .world
            .objects
            .by_watch_id(self.cur_object)
            .filter(|&h| {
                self.world.objects.get(h).is_some_and(|o| {
                    matches!(
                        o.kind,
                        crate::object::Kind::Mech(_) | crate::object::Kind::Vehicle(_)
                    )
                })
            });
        let part_id = mover
            .and_then(|h| self.world.objects.get(h).map(|o| o.c.part_id.0))
            .unwrap_or(0);
        if !self.world.ai_native_calls_frame.contains(&(part_id, id)) {
            self.world.ai_native_calls_frame.push((part_id, id));
        }
        if !crate::ai::natives::IMPLEMENTED.contains(&id) {
            let what = crate::ai::AiUnavailable::Native { id };
            match mover {
                Some(h) => crate::ai::report_unavailable(self.world, h, what),
                None => crate::ai::report_unavailable_unowned(self.world, what),
            }
        }
    }
    fn get_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::get_id(self)
    }
    fn select_object(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::select_object(self, object_id)
    }
    fn select_warrior(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, warrior_index: i32) -> i32 {
        crate::ai::natives::select_warrior(self, warrior_index)
    }
    fn get_warrior_status(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, warrior_id: i32) -> i32 {
        crate::ai::natives::get_warrior_status(self, warrior_id)
    }
    fn get_contacts(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        contact_list: mc2_abl::ArrayRef,
        contact_criteria: i32,
        sort_criteria: i32,
    ) -> i32 {
        crate::ai::natives::get_contacts(self, ctx, contact_list, contact_criteria, sort_criteria)
    }
    fn get_enemy_count(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::get_enemy_count(self, object_id)
    }
    fn select_contact(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, type_: i32, id: i32) -> i32 {
        crate::ai::natives::select_contact(self, type_, id)
    }
    fn get_contact_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::get_contact_id(self)
    }
    fn is_contact(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        criteria: i32,
        select: i32,
    ) -> i32 {
        crate::ai::natives::is_contact(self, object_id, criteria, select)
    }
    fn get_contact_status(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        contact_tagged: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_contact_status(self, ctx, contact_tagged)
    }
    fn get_contact_relative_position(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _range: f32,
        _angle: f32,
    ) -> i32 {
        crate::ai::natives::get_contact_relative_position(self)
    }
    fn set_target(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, attacker_id: i32, target_id: i32) {
        crate::ai::natives::set_target(self, attacker_id, target_id)
    }
    fn get_target(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::get_target(self, object_id)
    }
    fn get_weapons_ready(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
        list_size: i32,
    ) -> i32 {
        crate::ai::natives::get_weapons_ready(self, ctx, weapon_list, list_size)
    }
    fn get_weapons_locked(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
        list_size: i32,
    ) -> i32 {
        crate::ai::natives::get_weapons_locked(self, ctx, weapon_list, list_size)
    }
    fn get_weapons_in_range(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
        list_size: i32,
    ) -> i32 {
        crate::ai::natives::get_weapons_in_range(self, ctx, weapon_list, list_size)
    }
    fn get_weapon_shots(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, weapon_index: i32) -> i32 {
        crate::ai::natives::get_weapon_shots(self, weapon_index)
    }
    fn get_weapon_ranges(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        range_list: mc2_abl::ArrayRef,
    ) {
        crate::ai::natives::get_weapon_ranges(self, ctx, object_id, range_list)
    }
    fn get_object_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        coord_list: mc2_abl::ArrayRef,
    ) {
        crate::ai::natives::get_object_position(self, ctx, object_id, coord_list)
    }
    fn get_integer_memory(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, mem_index: i32) -> i32 {
        crate::ai::natives::get_integer_memory(self, mem_index)
    }
    fn get_real_memory(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, mem_index: i32) -> f32 {
        crate::ai::natives::get_real_memory(self, mem_index)
    }
    fn get_alarm_triggers(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        trigger_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_alarm_triggers(self, ctx, trigger_list)
    }
    fn get_challenger(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::get_challenger(self, object_id)
    }
    fn get_time_without_orders(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        crate::ai::natives::get_time_without_orders(self)
    }
    fn get_fire_ranges(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, ranges: mc2_abl::ArrayRef) {
        crate::ai::natives::get_fire_ranges(self, ctx, ranges)
    }
    fn get_attackers(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        attackers: mc2_abl::ArrayRef,
        seconds: f32,
    ) -> i32 {
        crate::ai::natives::get_attackers(self, ctx, attackers, seconds)
    }
    fn get_attacker_info(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, attacker_id: i32) -> f32 {
        crate::ai::natives::get_attacker_info(self, attacker_id)
    }
    fn set_challenger(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        victim_id: i32,
        challenger_id: i32,
    ) -> i32 {
        crate::ai::natives::set_challenger(self, victim_id, challenger_id)
    }
    fn set_integer_memory(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        mem_index: i32,
        mem_value: i32,
    ) {
        crate::ai::natives::set_integer_memory(self, mem_index, mem_value)
    }
    fn set_real_memory(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        mem_index: i32,
        mem_value: f32,
    ) {
        crate::ai::natives::set_real_memory(self, mem_index, mem_value)
    }
    fn has_move_goal(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        crate::ai::natives::has_move_goal(self)
    }
    fn has_move_path(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        crate::ai::natives::has_move_path(self)
    }
    fn sort_weapons(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
        list_size: i32,
        sort_type: i32,
    ) {
        crate::ai::natives::sort_weapons(self, ctx, weapon_list, list_size, sort_type)
    }
    fn get_visual_range(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> f32 {
        crate::ai::natives::get_visual_range(self, object_id)
    }
    fn get_unit_mates(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        mate_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_unit_mates(self, ctx, object_id, mate_list)
    }
    fn get_tac_order(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        _time: f32,
        param_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_tac_order(self, ctx, object_id, param_list)
    }
    fn get_last_tac_order(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        _time: f32,
        param_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_last_tac_order(self, ctx, object_id, param_list)
    }
    fn get_objects(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        criteria: i32,
        obj_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_objects(self, ctx, criteria, obj_list)
    }
    fn order_wait(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        seconds: f32,
        clear_last_target: bool,
    ) -> i32 {
        crate::ai::natives::order_wait(self, ctx, seconds, clear_last_target)
    }
    fn order_move_to(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        coord_list: mc2_abl::ArrayRef,
        run: bool,
    ) -> i32 {
        crate::ai::natives::order_move_to(self, ctx, coord_list, run)
    }
    fn order_move_to_object(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        run: bool,
    ) -> i32 {
        crate::ai::natives::order_move_to_object(self, ctx, object_id, run)
    }
    fn order_move_to_contact(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, run: bool) -> i32 {
        crate::ai::natives::order_move_to_contact(self, ctx, run)
    }
    fn order_power_down(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::order_power_down(self, ctx)
    }
    fn order_power_up(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::order_power_up(self, ctx)
    }
    fn order_attack_object(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        attack_type: i32,
        attack_method: i32,
        attack_range: i32,
        pursue: bool,
    ) -> i32 {
        crate::ai::natives::order_attack_object(
            self,
            ctx,
            object_id,
            attack_type,
            attack_method,
            attack_range,
            pursue,
        )
    }
    fn order_attack_contact(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        attack_type: i32,
        attack_method: i32,
        attack_range: i32,
        pursue: bool,
    ) -> i32 {
        crate::ai::natives::order_attack_contact(
            self,
            ctx,
            attack_type,
            attack_method,
            attack_range,
            pursue,
        )
    }
    fn order_withdraw(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::order_withdraw(self, ctx)
    }
    fn get_fixed(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        fixee_id: i32,
        bay_id: i32,
        params: i32,
    ) -> i32 {
        crate::ai::natives::get_fixed(self, fixee_id, bay_id, params)
    }
    /// `execOrderDeployElementals`: the C++ body is entirely under `#ifdef USE_ELEMENTALS`, which the shipped game never defines, so the
    /// native pops its argument and does nothing (the dispatcher pops it); no unavailable report.
    fn order_deploy_elementals(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, _params: i32) {}
    fn object_in_withdrawal(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id1: i32) -> i32 {
        crate::ai::natives::object_in_withdrawal(self, object_id1)
    }
    fn set_attack_radius(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, radius: f32) -> f32 {
        crate::ai::natives::set_attack_radius(self, radius)
    }
    fn distance_to_object(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id1: i32,
        object_id2: i32,
    ) -> f32 {
        crate::ai::natives::distance_to_object(self, object_id1, object_id2)
    }
    fn distance_to_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        coord_list: mc2_abl::ArrayRef,
    ) -> f32 {
        crate::ai::natives::distance_to_position(self, ctx, object_id, coord_list)
    }
    fn object_exists(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::object_exists(self, object_id)
    }
    fn object_status(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::object_status(self, object_id)
    }
    fn object_team(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::object_team(self, object_id)
    }
    fn object_class(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::object_class(self, object_id)
    }
    fn get_terrain_object_part_id(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        row: i32,
        col: i32,
    ) -> i32 {
        crate::ai::natives::get_terrain_object_part_id(row, col)
    }
    fn get_current_br_value(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_num: i32) -> i32 {
        crate::ai::natives::get_current_br_value(self, object_num)
    }
    fn object_type_id(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id1: i32) -> i32 {
        crate::ai::natives::object_type_id(self, object_id1)
    }
    fn object_status_count(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        tally_list: mc2_abl::ArrayRef,
    ) {
        crate::ai::natives::object_status_count(self, ctx, object_id, tally_list)
    }
    fn get_object_damage(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_num: i32) -> i32 {
        crate::ai::natives::get_object_damage(self, object_num)
    }
    fn get_object_active(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::ai::natives::get_object_active(self, object_id)
    }
    fn core_move_to(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        location: mc2_abl::ArrayRef,
        params: i32,
    ) -> i32 {
        crate::ai::natives::core_move_to(self, ctx, location, params)
    }
    fn core_move_to_object(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        params: i32,
    ) -> i32 {
        crate::ai::natives::core_move_to_object(self, ctx, object_id, params)
    }
    fn core_power(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, up: bool) -> i32 {
        crate::ai::natives::core_power(self, ctx, up)
    }
    fn core_attack(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        params: i32,
    ) -> i32 {
        crate::ai::natives::core_attack(self, ctx, object_id, params)
    }
    fn core_capture(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        params: i32,
    ) -> i32 {
        crate::ai::natives::core_capture(self, ctx, object_id, params)
    }
    fn set_captured(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) {
        crate::mission::natives_capture::set_captured(self, object_id)
    }
    fn set_capturable(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        capturable: bool,
    ) {
        crate::mission::natives_capture::set_capturable(self, object_id, capturable)
    }
    fn is_captured(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) -> i32 {
        crate::mission::natives_capture::is_captured(self, object_id)
    }
    fn is_capturable(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        capturing_team_id: i32,
    ) -> bool {
        crate::mission::natives_capture::is_capturable(self, object_id, capturing_team_id)
    }
    fn is_team_targeting(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        team_id: i32,
        target_id: i32,
        except_id: i32,
    ) -> bool {
        crate::mission::natives_capture::is_team_targeting(self, team_id, target_id, except_id)
    }
    fn is_team_capturing(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        team_id: i32,
        target_id: i32,
        except_id: i32,
    ) -> bool {
        crate::mission::natives_capture::is_team_capturing(self, team_id, target_id, except_id)
    }
    fn core_scan(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32, params: i32) -> i32 {
        crate::ai::natives::core_scan(self, ctx, object_id, params)
    }
    fn core_control(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        object_id: i32,
        params: i32,
    ) -> i32 {
        crate::ai::natives::core_control(self, ctx, object_id, params)
    }
    fn core_eject(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::core_eject(self, ctx)
    }
    fn set_pilot_state(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, new_state_handle: i32) -> i32 {
        crate::ai::natives::set_pilot_state(self, ctx, new_state_handle)
    }
    fn get_pilot_state(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::get_pilot_state(self)
    }
    fn get_next_pilot_event(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        param_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_next_pilot_event(self, ctx, param_list)
    }
    fn set_target_priority(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        index: i32,
        type_: i32,
        param1: i32,
        param2: i32,
        param3: i32,
    ) -> i32 {
        crate::ai::natives::set_target_priority(self, index, type_, param1, param2, param3)
    }
    fn set_debug_window(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _window_index: i32,
        _object_id: i32,
    ) -> i32 {
        // debug windows are a GUI feature (ablmc2.cpp:5161): the C++ pushes 0
        0
    }
    fn set_debug_string(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _object_id: i32,
        _string_num: i32,
        _debug_string: mc2_abl::ArrayRef,
    ) {
    }
    fn mc_print(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, _value: mc2_abl::AnyValue) {}
    fn request_help(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        enemy_part_id: i32,
        friendly_center: mc2_abl::ArrayRef,
        friendly_radius: f32,
        enemy_center: mc2_abl::ArrayRef,
        enemy_radius: f32,
        priority: i32,
    ) -> f32 {
        crate::ai::natives::request_help(
            self,
            ctx,
            enemy_part_id,
            friendly_center,
            friendly_radius,
            enemy_center,
            enemy_radius,
            priority,
        )
    }
    fn request_target(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        center: mc2_abl::ArrayRef,
        radius: f32,
    ) -> i32 {
        crate::ai::natives::request_target(self, ctx, center, radius)
    }
    fn request_shelter(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, range: f32) -> i32 {
        crate::ai::natives::request_shelter(self, range)
    }
    fn get_weapons(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
        info_type: i32,
    ) -> i32 {
        crate::ai::natives::get_weapons(self, ctx, weapon_list, info_type)
    }
    fn set_move_area(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        center: mc2_abl::ArrayRef,
        radius: f32,
    ) {
        crate::ai::natives::set_move_area(self, ctx, center, radius)
    }
    fn get_weapons_status(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        weapon_list: mc2_abl::ArrayRef,
    ) -> i32 {
        crate::ai::natives::get_weapons_status(self, ctx, weapon_list)
    }
    fn clear_tac_order(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) {
        crate::ai::natives::clear_tac_order(self)
    }
    fn set_will_help(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, will_help: bool) -> bool {
        crate::ai::natives::set_will_help(self, will_help)
    }
    fn get_last_scan(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        crate::ai::natives::get_last_scan(self)
    }
    fn get_map_info(&mut self, ctx: &mut mc2_abl::NativeCtx<'_>, map_info: mc2_abl::ArrayRef) {
        crate::ai::natives::get_map_info(self, ctx, map_info)
    }
    fn is_off_map(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        world_pos: mc2_abl::ArrayRef,
    ) -> bool {
        crate::ai::natives::is_off_map(self, ctx, world_pos)
    }
    fn set_goal_planning(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, setting: bool) -> bool {
        crate::ai::natives::set_goal_planning(self, setting)
    }
    fn set_eject(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, setting: bool) -> bool {
        crate::ai::natives::set_eject(self, setting)
    }
    fn set_keep_moving(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, setting: bool) -> bool {
        crate::ai::natives::set_keep_moving(self, setting)
    }
    fn get_time(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        self.world.mission.actual_time
    }

    fn get_time_left(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        let remaining = self.world.mission.time_limit - self.world.mission.actual_time;
        if self.world.mission.time_limit > -1.0 {
            remaining.max(0.0)
        } else {
            -1.0
        }
    }

    fn set_timer(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        timer_number: i32,
        duration: f32,
    ) -> i32 {
        let id = (timer_number as i16) as i32;
        if !(7..=14).contains(&id) {
            return 0;
        }
        if let Some(timer) = self.world.mission.timers.get(id) {
            timer.set(duration);
            id
        } else {
            0
        }
    }

    fn check_timer(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, timer_number: i32) -> f32 {
        self.world
            .mission
            .timers
            .get(timer_number)
            .map_or(0.0, |timer| timer.current().max(0.0))
    }

    fn end_timer(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, _timer_id: i32) {}

    fn set_objective_status(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _objective_num: i32,
        _status: i32,
    ) -> i32 {
        // The C++ method addresses the legacy `Mission::objectives` list, which is NULL in the new objective system.
        0
    }

    fn check_objective_status(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        objective_num: i32,
    ) -> i32 {
        check_objective_status(self.world, objective_num)
    }

    fn get_global_value(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, variable_num: i32) -> f32 {
        get_global_value(&self.world.mission, variable_num)
    }

    fn set_global_value(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        variable_num: i32,
        value: f32,
    ) {
        set_global_value(&mut self.world.mission, variable_num, value);
    }

    fn set_objective_pos(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        _variable_num: i32,
        _x: f32,
        _y: f32,
        _z: f32,
    ) {
        // The original mutates the legacy NULL objectives list; loaded CObjectives are immutable mission data.
    }

    fn send_message(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        multiplay_code: i32,
        multiplay_param: i32,
    ) {
        self.world.mission.current_message = (multiplay_code, multiplay_param);
    }

    fn get_message(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, _message_param: i32) -> i32 {
        self.world.mission.current_message.0
    }

    fn get_logistics_time(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        // `execGetLogisticsTime`: `timeGetTime()`, the virtual millisecond clock of the frame.
        self.world.clock.now_ms().unwrap_or(0) as f32
    }

    /// `execPlayWave` (ablmc2.cpp:6259-6268): support samples are identified by filename.
    fn play_wave(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        file_name: mc2_abl::ArrayRef,
        _type_: i32,
    ) -> i32 {
        crate::mission::natives_audio::play_wave(self, ctx, file_name)
    }

    /// `execStopVoiceOver` (ablmc2.cpp:5975-5983).
    /// `execSetInvulnerable` (ablmc2.cpp:5861-5869): combat damage reads this
    /// exact mission flag for the home team.
    fn set_invulnerable(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, value: bool) {
        crate::mission::natives_object::set_invulnerable(self, value);
    }

    fn get_mission_won(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        self.world.mission.term.result != -1
            && self.world.mission.term.result >= super::result::WIN_SMALL as i32
    }

    fn get_mission_lost(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        matches!(
            self.world.mission.term.result,
            x if x == super::result::LOST_BIG as i32 || x == super::result::LOST_SMALL as i32
        )
    }

    fn get_objective_success(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        objective_status_changed(self.world, true)
    }

    fn get_objective_failed(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        objective_status_changed(self.world, false)
    }

    fn get_enemy_destroyed(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        let value = self.world.mission.destroyed_latches.1;
        self.world.mission.destroyed_latches.1 = false;
        value
    }

    /// `execGetSensorsActive` (ablmc2.cpp:2793): `SensorSystemManager::enemyInLOS`, which the sensor scan sets when the home team
    /// has any contact on an enemy (contact.cpp:342-347).
    fn get_sensors_active(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        self.world.sensors.enemy_in_los
    }

    fn get_friendly_destroyed(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> bool {
        let value = self.world.mission.destroyed_latches.0;
        self.world.mission.destroyed_latches.0 = false;
        value
    }

    fn get_mission_status(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        if self.world.mission.term.counter_started {
            self.world.mission.term.result
        } else {
            super::result::PLAYING as i32
        }
    }

    fn add_trigger_area(
        &mut self,
        _ctx: &mut mc2_abl::NativeCtx<'_>,
        ul_row: i32,
        ul_col: i32,
        lr_row: i32,
        lr_col: i32,
        kind: i32,
        param: i32,
    ) -> i32 {
        self.world
            .mission
            .triggers
            .add(ul_row, ul_col, lr_row, lr_col, kind, param)
    }

    fn is_trigger_area_hit(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, handle: i32) -> bool {
        self.world.mission.triggers.is_hit(handle)
    }

    fn reset_trigger_area(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, handle: i32) {
        self.world.mission.triggers.reset(handle);
    }

    fn remove_trigger_area(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, handle: i32) {
        self.world.mission.triggers.remove(handle);
    }

    fn set_camera_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_pos: mc2_abl::ArrayRef,
    ) {
        let v = camera_vec3(ctx, cam_pos);
        // `Camera::setPosition(pos, false)` applies at once: x/y clamped away from the map edge
        // (`drawTerrainGrid` off), z snapped to the terrain (never below the water).  The request
        // carries the script's raw value; the application's camera applies the same rule.
        let p = camera_set_position(&self.world.terrain, v);
        self.world.camera.position = p;
        self.world.events.push(CameraRequest::Position(v).into());
    }

    fn set_camera_goal_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_pos: mc2_abl::ArrayRef,
        time: f32,
    ) {
        let v = camera_vec3(ctx, cam_pos);
        self.world.camera.goal_position = v;
        self.world
            .events
            .push(CameraRequest::GoalPosition(v, time).into());
    }

    fn set_camera_rotation(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_rot: mc2_abl::ArrayRef,
    ) {
        let v = camera_vec3(ctx, cam_rot);
        // `Camera::setRotation` -> `setCameraRotation` wraps the yaw into -180..180 at once, so a
        // read in the same script execution sees the wrapped value.
        self.world.camera.rotation = crate::math::Vec3 {
            x: v.x,
            y: wrap_camera_yaw(v.y),
            z: 0.0,
        };
        self.world.events.push(CameraRequest::Rotation(v).into());
    }

    fn set_camera_goal_rotation(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_rot: mc2_abl::ArrayRef,
        time: f32,
    ) {
        let v = camera_vec3(ctx, cam_rot);
        self.world.camera.goal_rotation = v;
        self.world
            .events
            .push(CameraRequest::GoalRotation(v, time).into());
    }

    fn set_camera_zoom(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, cam_fov: f32) {
        self.world.camera.fov = cam_fov;
        self.world.events.push(CameraRequest::Zoom(cam_fov).into());
    }

    fn set_camera_goal_zoom(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, cam_fov: f32, time: f32) {
        self.world.camera.goal_fov = cam_fov;
        self.world
            .events
            .push(CameraRequest::GoalZoom(cam_fov, time).into());
    }

    fn set_camera_velocity(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_vel: mc2_abl::ArrayRef,
    ) {
        let v = camera_vec3(ctx, cam_vel);
        self.world.camera.velocity = v;
        self.world.events.push(CameraRequest::Velocity(v).into());
    }

    fn set_camera_goal_velocity(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_vel: mc2_abl::ArrayRef,
        time: f32,
    ) {
        let v = camera_vec3(ctx, cam_vel);
        self.world.camera.goal_velocity = v;
        self.world
            .events
            .push(CameraRequest::GoalVelocity(v, time).into());
    }

    fn set_camera_look_object(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>, object_id: i32) {
        self.world.camera.look_object = object_id;
        self.world
            .events
            .push(CameraRequest::LookObject(object_id).into());
    }

    fn get_camera_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_pos: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.position;
        put_vec3(ctx, cam_pos, v);
    }

    fn get_camera_goal_position(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_pos: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.goal_position;
        put_vec3(ctx, cam_pos, v);
    }

    fn get_camera_rotation(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_rot: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.rotation;
        put_vec3(ctx, cam_rot, v);
    }

    fn get_camera_goal_rotation(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_rot: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.goal_rotation;
        put_vec3(ctx, cam_rot, v);
    }

    fn get_camera_zoom(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        self.world.camera.fov
    }

    fn get_camera_goal_zoom(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        self.world.camera.goal_fov
    }

    fn get_camera_velocity(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_vel: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.velocity;
        put_vec3(ctx, cam_vel, v);
    }

    fn get_camera_goal_velocity(
        &mut self,
        ctx: &mut mc2_abl::NativeCtx<'_>,
        cam_vel: mc2_abl::ArrayRef,
    ) {
        let v = self.world.camera.goal_velocity;
        put_vec3(ctx, cam_vel, v);
    }

    fn get_camera_look_object(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> i32 {
        self.world.camera.look_object
    }

    fn get_camera_frame_length(&mut self, _ctx: &mut mc2_abl::NativeCtx<'_>) -> f32 {
        self.world.clock.frame_length
    }
}

/// Writes `real[3]` of a `GetCamera*` native.
fn put_vec3(ctx: &mut mc2_abl::NativeCtx<'_>, a: mc2_abl::ArrayRef, v: crate::math::Vec3) {
    ctx.set_f32(a, 0, v.x);
    ctx.set_f32(a, 1, v.y);
    ctx.set_f32(a, 2, v.z);
}

/// `real[3]` of a camera native.
/// The position `Camera::setPosition(pos, false)` leaves (camera.cpp:2205): x/y clamped away from the
/// map edge (`drawTerrainGrid` off), z the terrain elevation, never below the water.
/// `Camera::setCameraRotation`: a single wrap into -180..180 (not a modulo).
fn wrap_camera_yaw(mut a: f32) -> f32 {
    if a > 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

fn camera_set_position(t: &crate::terrain::Terrain, v: crate::math::Vec3) -> crate::math::Vec3 {
    let max_visual = t.world_units_map_side / 2.0
        - crate::terrain::WORLD_UNITS_PER_VERTEX * 2.0
        - crate::terrain::WORLD_UNITS_PER_VERTEX * 6.0;
    let mut p = v;
    p.x = p.x.min(max_visual).max(-max_visual);
    p.y = p.y.min(max_visual).max(-max_visual);
    p.z = t.elevation(&p).max(t.water_elevation);
    p
}

fn camera_vec3(ctx: &mc2_abl::NativeCtx<'_>, a: mc2_abl::ArrayRef) -> crate::math::Vec3 {
    crate::math::Vec3 {
        x: ctx.get_f32(a, 0),
        y: ctx.get_f32(a, 1),
        z: ctx.get_f32(a, 2),
    }
}

/// `StatusChangedSuccess` / `StatusChangedFailed` over the home team's active objectives (objective.cpp:2001-2035): a change
/// is reported once, by whichever query asks for it first, and it is cleared either way.
pub fn objective_status_changed(w: &mut crate::World, success: bool) -> bool {
    let Some(team) = w.mission.objectives.get_mut(w.teams.home) else {
        return false;
    };
    let mut changed = false;
    for entry in &mut team.entries {
        if entry.runtime.active && entry.runtime.changed {
            entry.runtime.changed = false;
            changed |= entry.runtime.status
                == if success {
                    super::objective::ObjectiveStatus::Successful
                } else {
                    super::objective::ObjectiveStatus::Failed
                };
        }
    }
    changed
}

/// `if (missionBrain) { missionBrain->execute(); missionResult = getInteger(); }` (mission.cpp:521-541): runs the mission script after
/// sensors and collisions and returns its result (9999 = quit, -1 = do not evaluate objectives, otherwise `terminationResult`).
/// `None` when the mission has no brain (or no loaded script environment): the C++ block does not run, so `terminationResult` keeps
/// its value (the force-death path sets it, mission.cpp:553-559).
pub fn run_mission_brain(w: &mut crate::World) -> Option<i32> {
    let inst = w.mission.brain?;
    // No ABL environment means no script ran, so there is no script result to report. Returning `Some(0)` here would overwrite
    // `terminationResult` (the force-death LOST_BIG) with 0, the bug the brain-less path exists to prevent.
    let mut env = crate::ai::brain::take_env(w)?;
    let result = {
        let mut bindings = SimBindings::mission(w);
        if env.execute(inst, &mut bindings).is_ok() {
            env.get_integer(inst)
        } else {
            0
        }
    };
    crate::ai::brain::restore_env(w, env);
    Some(result)
}

/// `checkobjectivestatus` (ablmc2.cpp) over `Mission::checkObjectiveStatus` (mission.cpp:3498-3513): the status code of objective
/// `objective_num` of the home team, from `Status(objectives)` of that one objective with its qualifiers, evaluated now. Objectives
/// after it are not evaluated; the answer is this call's, not the cached runtime status.
pub fn check_objective_status(w: &mut crate::World, objective_num: i32) -> i32 {
    let Ok(index) = usize::try_from(objective_num) else {
        return 0;
    };
    let team = w.teams.home;
    let status = super::objective::objective_status_at(w, team, index);
    super::objective_eval::status_code(status)
}

/// `getglobalvalue` (ablmc2.cpp:3416): indices outside `globalMissionValues` read 0.
pub fn get_global_value(m: &super::MissionState, variable_num: i32) -> f32 {
    match usize::try_from(variable_num) {
        Ok(i) if i < MAX_GLOBAL_MISSION_VALUES => m.global_values[i],
        _ => 0.0,
    }
}

/// `setglobalvalue` (ablmc2.cpp:3435): indices outside `globalMissionValues` are ignored.
pub fn set_global_value(m: &mut super::MissionState, variable_num: i32, value: f32) {
    if let Ok(i) = usize::try_from(variable_num) {
        if i < MAX_GLOBAL_MISSION_VALUES {
            m.global_values[i] = value;
        }
    }
}

#[cfg(test)]
mod camera_tests {
    use super::{camera_set_position, wrap_camera_yaw};
    use crate::math::Vec3;
    use crate::terrain::Terrain;

    #[test]
    fn set_rotation_readback_wraps_the_yaw() {
        assert_eq!(wrap_camera_yaw(190.0), -170.0);
        assert_eq!(wrap_camera_yaw(-190.0), 170.0);
        assert_eq!(wrap_camera_yaw(90.0), 90.0);
    }

    #[test]
    fn set_position_readback_clamps_to_the_map_and_snaps_to_the_ground() {
        let side = 20;
        let mut t = Terrain::from_elevations(side, &vec![100.0; (side * side) as usize]);
        t.water_elevation = 10.0;
        let max = t.world_units_map_side / 2.0 - 128.0 * 8.0;
        // inside: only z changes (to the terrain)
        let p = camera_set_position(
            &t,
            Vec3 {
                x: 5.0,
                y: -7.0,
                z: 999.0,
            },
        );
        assert_eq!((p.x, p.y), (5.0, -7.0));
        assert_eq!(p.z, 100.0);
        // outside: x/y clamp to the edge margin
        let p = camera_set_position(
            &t,
            Vec3 {
                x: 1e9,
                y: -1e9,
                z: 0.0,
            },
        );
        assert_eq!((p.x, p.y), (max, -max));
        // below the water: the water elevation
        t.water_elevation = 150.0;
        let p = camera_set_position(
            &t,
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        );
        assert_eq!(p.z, 150.0);
    }
}

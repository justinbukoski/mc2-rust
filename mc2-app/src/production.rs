// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! What a production mission registers with [`GameCore`]: the effects sink and its scene pass
//! ([`crate::fx_sink`]) and the battle sound sink ([`crate::sim_audio`]).
//!
//! Both front ends (`mc2 --headless` and the window) call [`install_sinks`] once, right after
//! building the core for the live simulation; tests and the viewer that want no effects or battle
//! sound simply do not.  The sinks see every simulation frame's events in the frame that produced
//! them (`mission_tick` hands them over before the render), so a weapon that fires in a frame has
//! its effect queued and its sample started in that same frame.

use crate::fx_sink::FxHub;
use crate::game::GameCore;
use crate::sim::SimDriver;
use crate::sim_audio::{AudioSink, SimAudioBridge};
use mc2_sim::{SimEvent, World};
use mc2_ui::screens::Prefs;

/// Registers the production effects sink (and its [`crate::layers::SceneEffects`] draw hookup in
/// `core.effects`) and, when the core has game sound, the battle sound sink on the core's own
/// `GameAudio`.  A core without sound (no device, no sound data) stays silent: the sink is not
/// installed and the core says so through [`GameCore::battle_sound`] being false.
pub fn install_sinks<D>(core: &mut GameCore<D>) -> Result<(), String>
where
    D: SimDriver<Event = SimEvent, World = World> + 'static,
{
    // `useNonWeaponEffects` (`code/prefs.cpp:250`): off when `options.cfg` does not say.
    let prefs = match core.game.fit("options.cfg") {
        Ok(mut f) => Prefs::from_fit(&mut f),
        Err(_) => Prefs::default(),
    };
    core.non_weapon_effects = prefs.use_non_weapon_effects;
    let hub = FxHub::new(
        &mut core.scene,
        &mut core.game.vfs,
        core.config.sim_seed,
        prefs.use_non_weapon_effects,
    )?;
    core.sinks.push(Box::new(hub.sink()));
    core.effects = Some(Box::new(hub.scene()));
    core.fx_hub = Some(hub);
    if let Some(audio) = &core.audio {
        let sink = AudioSink::<D>::shared(audio.shared_game(), Box::new(SimAudioBridge));
        core.sinks.push(Box::new(sink));
        core.battle_sound = true;
    }
    Ok(())
}

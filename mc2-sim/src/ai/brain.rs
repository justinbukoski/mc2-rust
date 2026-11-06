// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Running a pilot's ABL brain: `MechWarrior::runBrain` (warrior.cpp:2121) and the alarm handler call of `checkAlarms` /
//! `handleAlarm` (warrior.cpp:4720-4730, 4675-4688).
//!
//! The `Env` is taken out of the world for the call (`Option::take`) so the bindings can borrow the world mutably. In the C++ a
//! native that raises an alarm (or otherwise runs a brain) executes the inner module inside the outer script's native call. Here the
//! native only queues the request in [`World::deferred_brain_calls`]; [`run_nested`] (the bindings' `after_native` hook) runs the
//! queue, on the same environment, right after that native returns and before its result reaches the outer script, which is the C++
//! order. A request still queued when the outermost script returns (beyond the nesting depth or the shared run budget) is reported `NestedBrain`, not run;
//! `DisableRest` continuations always run.

use super::pilot::goal_action;
use super::warrior::{
    clear_cur_tac_order, commander_is_home, set_general_tac_order, set_main_goal,
    set_player_tac_order, vehicle,
};
use super::{report_unavailable, AiUnavailable};
use crate::command::{OrderCode, TacticalOrder};
use crate::World;
use mc2_abl::Env;

/// A brain call requested while another script was running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeferredBrainCall {
    /// Attach the brain of a script-created infantry pilot once the native returns.
    AttachInfantry(u16),
    /// `runBrain` of the pilot slot.
    Run(u16),
    /// The alarm handler `code` of the pilot slot.
    Alarm(u16, usize),
    /// Not a brain call: the part of `Mover::disable` that follows the incapacitation alarm, queued behind the handler so the handler
    /// sees the mover as the C++ does (not yet disabled). Always runs; never counted against the nesting budget.
    DisableRest(crate::ObjectHandle, u32),
    /// Not a brain call: the part of `handleEjection` that follows `eject` (queued behind the incapacitation handler and `DisableRest`).
    EjectRest(crate::ObjectHandle),
    /// Not a brain call: the `Eject` general order that `coreEject`/`orderEject` install after `handleEjection` returns (`unit_order`).
    EjectOrder(u16, bool),
}

/// Most brain calls run nested inside natives during one outermost script; later requests stay queued and are reported `NestedBrain`.
const MAX_NESTED_RUNS: u32 = 64;

/// Take the ABL environment for a script run. `None` when it is not loaded or a script is already running.
pub(crate) fn take_env(w: &mut World) -> Option<Env> {
    let env = w.abl.take()?;
    w.abl_running = true;
    w.nested_runs = 0;
    Some(env)
}

/// Deepest nesting of scripts inside natives; deeper requests stay queued (a handler that raises its own alarm cannot recurse forever).
const MAX_NESTING: u32 = 8;

/// Run the brain calls queued by the native that just returned, oldest first, as nested scripts on `env` (the C++ runs them inside
/// the native call). The handler/brain gets its own bindings: its `CurObject`, `CurWarrior`, `CurAlarm`; the outer script's context is
/// untouched (the C++ nulls the globals after the inner call, clobbering the outer script's; not reproduced).
pub(crate) fn run_nested(w: &mut World, env: &mut Env) {
    // The queue is taken out while its calls run: a native inside a handler flushes only what that handler itself raised, so a
    // queued `DisableRest` waits for the handler before it (the C++ finishes `handleAlarm` before `setStatus`).
    let mut pending: std::collections::VecDeque<_> =
        std::mem::take(&mut w.deferred_brain_calls).into();
    let mut left = Vec::new();
    while let Some(call) = pending.pop_front() {
        match call {
            DeferredBrainCall::AttachInfantry(slot) => {
                if crate::mission::abl_load::make_pilot_brain_in_env(
                    w,
                    env,
                    slot as usize,
                    "infantry001",
                )
                .is_err()
                {
                    w.note_combat_unavailable("createInfantry: infantry brain failed to load");
                }
            }
            DeferredBrainCall::EjectRest(h) => {
                crate::combat::damage::eject_after_disable(w, h);
                // what the rest of `handleEjection` raised runs before whatever was queued after it
                for c in std::mem::take(&mut w.deferred_brain_calls)
                    .into_iter()
                    .rev()
                {
                    pending.push_front(c);
                }
            }
            DeferredBrainCall::DisableRest(h, cause) => {
                crate::combat::damage::disable_after_alarm(w, h, cause);
            }
            DeferredBrainCall::EjectOrder(slot, unit) => {
                super::orders::finish_eject_order(w, slot, unit)
            }
            // One budget for the whole outermost script, so handlers that keep raising alarms cannot run forever.
            _ if env.exec_depth() >= MAX_NESTING || w.nested_runs >= MAX_NESTED_RUNS => {
                left.push(call);
            }
            DeferredBrainCall::Run(slot) => {
                w.nested_runs += 1;
                if run_brain_script(w, slot, env).is_some() {
                    finish_brain(w, slot);
                }
                left.append(&mut w.deferred_brain_calls);
            }
            DeferredBrainCall::Alarm(slot, code) => {
                w.nested_runs += 1;
                run_alarm_script(w, slot, code, env);
                left.append(&mut w.deferred_brain_calls);
            }
        }
    }
    w.deferred_brain_calls = left;
}

/// Put the environment back after a script run. The C++ has no queue: a brain call made inside a native runs inside that native
/// ([`run_nested`]) and one made outside any script runs at once, so nothing may be left to run here. Requests still queued are
/// those [`run_nested`] refused beyond the nesting bound; they are reported, never run late and never dropped silently.
pub(crate) fn restore_env(w: &mut World, env: Env) {
    w.abl = Some(env);
    w.abl_running = false;
    let mut refused = false;
    for call in std::mem::take(&mut w.deferred_brain_calls) {
        match call {
            // state changes are never dropped
            DeferredBrainCall::DisableRest(h, cause) => {
                crate::combat::damage::disable_after_alarm(w, h, cause)
            }
            DeferredBrainCall::EjectRest(h) => crate::combat::damage::eject_after_disable(w, h),
            DeferredBrainCall::EjectOrder(slot, unit) => {
                super::orders::finish_eject_order(w, slot, unit)
            }
            _ => refused = true,
        }
    }
    if refused {
        super::report_unavailable_unowned(w, AiUnavailable::NestedBrain);
    }
}

/// `MechWarrior::runBrain()` (warrior.cpp:2121-2208). Returns the brain's error code (`brain->getInteger()`).
pub fn run_brain(w: &mut World, slot: u16) -> i32 {
    let Some(mut env) = take_env_for_brain(w, slot) else {
        return 0;
    };
    let brain_err = run_brain_script(w, slot, &mut env);
    restore_env(w, env);
    match brain_err {
        Some(err) => {
            finish_brain(w, slot);
            err
        }
        None => 0,
    }
}

/// The environment for a `runBrain` of `slot`; `None` (queueing the call when a script is running) otherwise.
fn take_env_for_brain(w: &mut World, slot: u16) -> Option<Env> {
    w.pilots.get(slot).and_then(|p| p.brain)?;
    vehicle(w, slot)?;
    match take_env(w) {
        Some(env) => Some(env),
        None => {
            if w.abl_running {
                // curEventID = 0; curEventTrigger = 0; are set when the queued call runs
                w.deferred_brain_calls.push(DeferredBrainCall::Run(slot));
            }
            None
        }
    }
}

/// The script half of `runBrain` on an environment that is already out of the world: `Some(brain error code)`, `None` when the pilot
/// has no brain or vehicle.
fn run_brain_script(w: &mut World, slot: u16, env: &mut Env) -> Option<i32> {
    let inst = w.pilots.get(slot).and_then(|p| p.brain)?;
    let me = vehicle(w, slot)?;
    // curEventID = 0; curEventTrigger = 0;
    w.pilots.cur_event_id = 0;
    w.pilots.cur_event_trigger = 0;
    let executed = {
        let mut bindings = crate::mission::bindings::SimBindings::pilot(w, slot);
        env.execute(inst, &mut bindings)
    };
    let brain_err = env.get_integer(inst);
    if let (Ok(n), Some(p)) = (&executed, w.pilots.get_mut(slot)) {
        p.brain_statements = *n;
    }
    if executed.is_err() {
        report_unavailable(w, me, AiUnavailable::AblRuntimeError);
    }
    Some(brain_err)
}

/// The rest of `runBrain` after the script ran: the goal plan and the alarm history reset.
fn finish_brain(w: &mut World, slot: u16) {
    // Well, we'll just set it every frame so it doesn't screw up :)
    let use_goal_plan = !commander_is_home(w, slot);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.use_goal_plan = use_goal_plan;
    }
    if use_goal_plan {
        let (action, wid, location, radius) = {
            let g = &w.pilots.get(slot).expect("pilot").main_goal;
            (g.action, g.object, g.location, g.control_radius)
        };
        let mut tac_order = TacticalOrder::init();
        let result = super::goal::calc_tac_order(
            w,
            slot,
            action,
            wid,
            location,
            radius,
            0,
            1,
            150.0,
            300,
            &mut tac_order,
        );
        if result == 0 {
            if tac_order.code != OrderCode::None {
                if commander_is_home(w, slot) {
                    set_player_tac_order(w, slot, tac_order, false);
                } else {
                    set_general_tac_order(w, slot, tac_order);
                }
            }
        } else if result > 0 {
            set_main_goal(w, slot, goal_action::NONE, None, None, -1.0);
            clear_cur_tac_order(w, slot, true);
        }
    }

    // All brain modules should return an error code... (0 none; anything else the C++ only notes)
    clear_alarms_history(w, slot);
}

fn clear_alarms_history(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        for h in &mut p.alarm_history {
            h.clear();
        }
    }
}

/// `if (brain && brainAlarmCallback[code]) { CurAlarm = code; brain->execute(NULL, brainAlarmCallback[code]); }`
/// (warrior.cpp:4833-4838): run the brain's handler for alarm `code`, if its brain defines one.
pub fn call_alarm_handler(w: &mut World, slot: u16, code: usize) {
    if w.pilots.get(slot).and_then(|p| p.brain).is_none()
        || super::calls::PILOT_ALARM_FUNCTION_NAMES.get(code).is_none()
    {
        return;
    }
    if w.abl_running {
        // The environment is running another script: the handler runs nested, once the native that raised it returns.
        w.deferred_brain_calls
            .push(DeferredBrainCall::Alarm(slot, code));
        return;
    }
    let Some(mut env) = take_env(w) else {
        return;
    };
    run_alarm_script(w, slot, code, &mut env);
    restore_env(w, env);
}

/// Run alarm handler `code` of the pilot's brain on an environment that is out of the world, if the brain defines one.
fn run_alarm_script(w: &mut World, slot: u16, code: usize, env: &mut Env) {
    let Some(inst) = w.pilots.get(slot).and_then(|p| p.brain) else {
        return;
    };
    let Some(name) = super::calls::PILOT_ALARM_FUNCTION_NAMES.get(code) else {
        return;
    };
    let Some(function) = env.find_function(inst, name, true) else {
        return;
    };
    let result = {
        let mut bindings = crate::mission::bindings::SimBindings::pilot(w, slot);
        bindings.cur_alarm = code as i32;
        env.execute_function(inst, function, &mut bindings)
    };
    if let (Ok(n), Some(p)) = (&result, w.pilots.get_mut(slot)) {
        p.brain_statements = *n;
    }
    if result.is_err() {
        if let Some(me) = vehicle(w, slot) {
            report_unavailable(w, me, AiUnavailable::AblRuntimeError);
        }
    }
}

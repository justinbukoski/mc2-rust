// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mech gestures on real data: the per-chassis `[Gestures%d]` tables of the `.ini`, and the gesture
//! state machine (`Mech3DAppearance::update`, mech3d.cpp:3790) with real frame counts.  Skipped
//! without `MC2_DATA`.

use mc2_formats::vfs::Vfs;
use mc2_sim::appearance::mech::{MechAppearance, MechAppearanceType, UpdateCtx};
use mc2_sim::appearance::tables::{gesture, state};
use mc2_sim::rng::{RngMode, SimRng};
use std::path::PathBuf;

fn vfs() -> Option<Vfs> {
    let dir = std::env::var_os("MC2_DATA")
        .map(PathBuf::from)
        .filter(|p| p.join("system.cfg").exists());
    if dir.is_none() {
        eprintln!("skipped: MC2_DATA not set");
    }
    dir.map(|d| Vfs::from_system_cfg(d).expect("vfs"))
}

/// `Gestures<n>` blocks of an `.ini` read with a plain line parser (independent of the FIT reader):
/// `[StartVel, EndVel, StartFrame, Reverse]` per block, `None` for a key the block lacks.
fn raw_gestures(text: &str) -> Vec<Option<[Option<f32>; 4]>> {
    let mut out = vec![None; gesture::MAX];
    let mut cur: Option<usize> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.trim_end_matches(']');
            cur = name
                .strip_prefix("Gestures")
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|&n| n < gesture::MAX);
            if let Some(i) = cur {
                out[i] = Some([None; 4]);
            }
            continue;
        }
        let (Some(i), Some((k, v))) = (cur, line.split_once('=')) else {
            continue;
        };
        let v = v.split("//").next().unwrap().trim().trim_end_matches(';');
        let e = out[i].as_mut().unwrap();
        match k.trim() {
            "StartVel" => e[0] = Some(v.parse().unwrap()),
            "EndVel" => e[1] = Some(v.parse().unwrap()),
            "StartFrame" => e[2] = Some(v.parse::<i32>().unwrap() as f32),
            "Reverse" => e[3] = Some(v.parse::<i32>().unwrap() as f32),
            _ => {}
        }
    }
    out
}

#[test]
fn gesture_tables_match_the_ini_for_two_chassis() {
    let Some(mut vfs) = vfs() else { return };
    for name in ["atlas", "madcat", "bushwacker"] {
        let ty = MechAppearanceType::load(&mut vfs, name).expect(name);
        let bytes = vfs.read(&format!("data/tgl/{name}.ini")).unwrap().unwrap();
        let raw = raw_gestures(&String::from_utf8_lossy(&bytes));
        let mut blocks = 0;
        for (g, r) in raw.iter().enumerate() {
            let Some(keys) = *r else { continue };
            blocks += 1;
            let d = &ty.gestures[g];
            let got = [
                d.start_vel,
                d.end_vel,
                d.frame_start as f32,
                f32::from(u8::from(d.reverse)),
            ];
            for (k, want) in keys.iter().enumerate() {
                if let Some(want) = want {
                    assert_eq!(got[k], *want, "{name} Gestures{g} key {k}");
                }
            }
        }
        assert!(blocks >= 20, "{name} has its gesture blocks ({blocks})");
        // a mech walks and runs at a positive speed, stands still
        assert_eq!(ty.start_vel(gesture::STAND), 0.0, "{name}");
        assert!(ty.end_vel(gesture::WALK) > 0.0, "{name}");
        assert!(
            ty.end_vel(gesture::RUN) > ty.end_vel(gesture::WALK),
            "{name}"
        );
        // the animation files of the walk and run gestures are loaded, with their frame counts
        for g in [gesture::WALK, gesture::RUN, gesture::JUMP] {
            assert!(ty.anims[g as usize].is_some(), "{name} gesture {g} anim");
            assert!(ty.num_frames(g) >= 1, "{name} gesture {g} frames");
        }
        assert!(ty.num_frames(gesture::WALK) > 4, "{name}");
        assert!(ty.frame_rate(gesture::WALK) > 0.0, "{name}");
    }
}

const FRAME: f32 = 1.0 / 30.0;

struct Rig {
    ty: MechAppearanceType,
    a: MechAppearance,
    rng: SimRng,
    turn: i64,
}

impl Rig {
    fn new(name: &str) -> Option<Rig> {
        let mut vfs = vfs()?;
        let mut ty = MechAppearanceType::load(&mut vfs, name).expect(name);
        let mut rng = SimRng::new(RngMode::Pcg32, 7);
        let a = MechAppearance::new(&mut ty, 0, &mut rng);
        Some(Rig {
            ty,
            a,
            rng,
            turn: 10,
        })
    }

    fn update(&mut self) {
        self.turn += 1;
        let mut ctx = UpdateCtx {
            turn: self.turn,
            frame_length: FRAME,
            sim_rng: &mut self.rng,
            gesture_rng: None,
            terrain: None,
        };
        self.a.update(&mut self.ty, true, &mut ctx);
    }

    /// Updates until `done` or `max` frames; returns the frames taken.
    fn until(&mut self, max: usize, mut done: impl FnMut(&MechAppearance) -> bool) -> usize {
        for n in 1..=max {
            self.update();
            if done(&self.a) {
                return n;
            }
        }
        panic!(
            "not reached in {max} frames: gesture {}",
            self.a.current_gesture
        );
    }
}

/// Run the machine on each chassis.
fn each(f: impl Fn(&mut Rig)) {
    for name in ["atlas", "madcat"] {
        if let Some(mut r) = Rig::new(name) {
            r.update();
            f(&mut r);
        }
    }
}

#[test]
fn stands_then_walks_then_stops() {
    each(|r| {
        assert_eq!(r.a.current_gesture, gesture::STAND);
        assert_eq!(r.a.velocity_magnitude(&r.ty), 0.0);
        // walk goal: the table's chain from standing (stand -> walk, via the walk-start gesture when the
        // chassis has one), then the walk loops
        assert_eq!(r.a.set_gesture_goal(state::WALKING), -1);
        // first caller wins for the frame
        assert_eq!(r.a.set_gesture_goal(state::RUNNING), 0);
        let mut seen = vec![r.a.current_gesture];
        r.until(400, |a| {
            if seen.last() != Some(&a.current_gesture) {
                seen.push(a.current_gesture);
            }
            a.current_gesture == gesture::WALK && a.current_state_goal == -1
        });
        assert_eq!(seen.last(), Some(&gesture::WALK), "{seen:?}");
        // walking: the frame advances mechFrameRate * frameLength and wraps at the frame count
        let n = r.ty.num_frames(gesture::WALK) as f32;
        let rate = r.ty.frame_rate(gesture::WALK).abs();
        let mut wrapped = false;
        let mut prev = r.a.current_frame;
        for _ in 0..(n as usize * 2 + 4) {
            r.update();
            assert_eq!(r.a.current_gesture, gesture::WALK);
            let step = r.a.current_frame - prev;
            if step < 0.0 {
                wrapped = true;
                assert!((step + n - rate * FRAME).abs() < 1e-3, "wrap {step}");
            } else {
                assert!(
                    (step - rate * FRAME).abs() < 1e-3,
                    "step {step} rate {rate}"
                );
            }
            prev = r.a.current_frame;
            // speed follows the gesture's velocity curve
            let v = r.a.velocity_magnitude(&r.ty);
            let (s, e) = (r.ty.start_vel(gesture::WALK), r.ty.end_vel(gesture::WALK));
            assert!(v >= s.min(e) - 1e-3 && v <= s.max(e) + 1e-3);
        }
        assert!(wrapped, "the walk cycle loops");
        // stop: back to standing through the table's chain
        assert_eq!(r.a.set_gesture_goal(state::STANDING), -1);
        r.until(400, |a| {
            a.current_gesture == gesture::STAND && a.current_state_goal == -1
        });
        assert_eq!(r.a.velocity_magnitude(&r.ty), 0.0);
    });
}

#[test]
fn walks_to_run_and_back() {
    each(|r| {
        r.a.set_gesture_goal(state::WALKING);
        r.until(400, |a| {
            a.current_gesture == gesture::WALK && a.current_state_goal == -1
        });
        r.a.set_gesture_goal(state::RUNNING);
        let mut seen = vec![];
        r.until(400, |a| {
            if seen.last() != Some(&a.current_gesture) {
                seen.push(a.current_gesture);
            }
            a.current_gesture == gesture::RUN && a.current_state_goal == -1
        });
        assert!(seen.contains(&gesture::RUN), "{seen:?}");
        assert!(r.a.velocity_magnitude(&r.ty) > r.ty.end_vel(gesture::WALK) * 0.9);
        r.a.set_gesture_goal(state::WALKING);
        r.until(400, |a| {
            a.current_gesture == gesture::WALK && a.current_state_goal == -1
        });
    });
}

#[test]
fn brake_stops_at_once() {
    each(|r| {
        r.a.set_gesture_goal(state::WALKING);
        r.until(400, |a| {
            a.current_gesture == gesture::WALK && a.current_state_goal == -1
        });
        assert!(r.a.velocity_magnitude(&r.ty) > 0.0);
        r.a.set_brake(true);
        r.update();
        assert_eq!(r.a.current_gesture, gesture::STAND);
        // the stand gesture restarts at frame 0 and advances inside its own frame count
        let n = r.ty.num_frames(gesture::STAND).max(1) as f32;
        assert!(r.a.current_frame >= 0.0 && r.a.current_frame < n);
        assert_eq!(r.a.velocity_magnitude(&r.ty), 0.0);
    });
}

#[test]
fn idles_after_twenty_seconds_standing() {
    each(|r| {
        // idleMAX = 20 s of 1/30 s frames
        // `idleTime` starts at RandomNumber(idleMAX) (a sim draw), so the wait is what is left of 20 s
        let start = r.a.idle_time;
        let n = r.until(1000, |a| a.current_gesture == gesture::IDLE);
        let expect = (20.0 - start) / FRAME;
        assert!(
            (n as f32 - expect).abs() <= 3.0,
            "idle after {n} frames, expected {expect}"
        );
        // the idle gesture plays once and returns to stand
        r.until(2000, |a| a.current_gesture == gesture::STAND);
    });
}

#[test]
fn combat_mode_never_idles() {
    each(|r| {
        r.a.set_combat_mode(true);
        for _ in 0..700 {
            r.update();
            assert_eq!(r.a.current_gesture, gesture::STAND);
        }
    });
}

#[test]
fn jump_takes_off_flies_and_lands() {
    each(|r| {
        // jumping needs the destination first (`setJumpParameters`)
        assert_eq!(r.a.set_gesture_goal(state::JUMPING), 0);
        r.a.set_object_parameters(mc2_sim::math::Vec3::default(), 0.0);
        assert!(r.a.set_jump_parameters(mc2_sim::math::Vec3 {
            x: 100.0,
            y: 0.0,
            z: 0.0
        }));
        assert_eq!(r.a.set_gesture_goal(state::JUMPING), -1);
        r.until(100, |a| a.current_gesture == gesture::JUMP);
        let (take, land) = (
            r.ty.jump_takeoff_frame() as f32,
            r.ty.jump_land_frame() as f32,
        );
        assert!(take < land);
        let airborne = r.until(400, |a| a.jump_airborne);
        assert!(r.a.current_frame >= take);
        let flight = r.until(400, |a| !a.jump_airborne);
        assert!(r.a.current_frame >= land);
        // the flight lasts (landing - takeoff) frames at the jump's frame rate
        let expect = (land - take) / (r.ty.frame_rate(gesture::JUMP).abs() * FRAME);
        assert!(
            (flight as f32 - expect).abs() <= 2.0,
            "flight {flight} expect {expect} ({airborne})"
        );
        r.until(400, |a| a.current_gesture == gesture::STAND && !a.in_jump);
    });
}

/// The destroyed/disabled fall roll (`RollDice(50)`, mech3d.cpp:3952-3967) reads the status the C++ appearance holds, which
/// `BattleMech::update` hands over to a dead mech only the frame after `mechRemoved` (mech.cpp:6229-6233, 6282-6283). D-52's
/// immediate DESTROYED (`set_obj_status`) is for the damage shape and must not draw: on mc2_14 seed 1 the early rolls (one a frame
/// from the end of the fall) were the first sim-stream divergence from the oracle (turn 981, which draws from turn 1087).
#[test]
fn a_dead_mech_rolls_its_fall_only_after_the_status_handover() {
    each(|r| {
        r.a.recalc_bounds(&mut r.ty, true, 0);
        r.a.set_obj_status(&r.ty, 2);
        let draws = r.rng.draws();
        for _ in 0..30 {
            r.update();
        }
        assert_eq!(r.rng.draws(), draws, "no roll before the hand-over");
        assert_eq!(r.a.current_state_goal, -1);
        // the hand-over: one draw, and the goal becomes a fall
        r.a.cpp_status = 2;
        r.update();
        assert_eq!(r.rng.draws(), draws + 1);
        assert!(
            r.a.current_state_goal == state::FALLEN_FORWARD
                || r.a.current_state_goal == state::FALLEN_BACKWARD,
            "{}",
            r.a.current_state_goal
        );
        // on its way down (goal 7/8) the mech does not roll again
        r.update();
        assert_eq!(r.rng.draws(), draws + 1);
    });
    // a disabled mech (never destroyed) is handed over every frame, so it rolls at once
    each(|r| {
        r.a.recalc_bounds(&mut r.ty, true, 0);
        r.a.set_obj_status(&r.ty, 1);
        r.a.cpp_status = 1;
        let draws = r.rng.draws();
        r.update();
        assert_eq!(r.rng.draws(), draws + 1);
    });
}

#[test]
fn destroyed_mech_falls_and_stays_down() {
    each(|r| {
        r.a.recalc_bounds(&mut r.ty, true, 0);
        r.a.set_obj_status(&r.ty, 2);
        // the fall goal comes from the appearance's own roll here, which reads the C++ hand-over status
        r.a.cpp_status = 2;
        r.until(400, |a| {
            a.current_gesture == gesture::FALL_FORWARD
                || a.current_gesture == gesture::FALL_BACKWARD
        });
        let fall = r.a.current_gesture;
        // The fall is drawn: the gesture stays bound to the shape and its frame advances (not frozen at the kill).
        let f0 = r.a.shape.frame_num;
        let mut frames = std::collections::BTreeSet::new();
        for _ in 0..10 {
            r.update();
            if r.a.current_gesture == fall {
                assert_eq!(r.a.bound_gesture, usize::try_from(fall).ok());
                frames.insert(r.a.shape.frame_num.to_bits());
            }
        }
        assert!(
            frames.len() > 1 || f0 != r.a.shape.frame_num,
            "fall frames advance"
        );
        r.until(600, |a| {
            a.current_gesture == gesture::FALLEN_FORWARD
                || a.current_gesture == gesture::FALLEN_BACKWARD
        });
        let fallen = r.a.current_gesture;
        assert_eq!(
            fallen,
            if fall == gesture::FALL_FORWARD {
                gesture::FALLEN_FORWARD
            } else {
                gesture::FALLEN_BACKWARD
            }
        );
        for _ in 0..60 {
            r.update();
            assert_eq!(r.a.current_gesture, fallen);
        }
    });
}

#[test]
fn a_hit_plays_its_gesture_and_blocks_goals() {
    each(|r| {
        r.a.hit(&r.ty, gesture::HIT_BACK);
        assert!(
            matches!(r.a.current_gesture, gesture::HIT_BACK),
            "{}",
            r.a.current_gesture
        );
        assert_eq!(
            r.a.set_gesture_goal(state::WALKING),
            0,
            "goals are ignored during a hit"
        );
        r.until(200, |a| a.current_gesture == gesture::STAND);
        assert_eq!(r.a.set_gesture_goal(state::WALKING), -1);
    });
}

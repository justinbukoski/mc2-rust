//! Smoke tests for the scaffold: the world can be created, streams are wired to the ABL host, and the public surface is stable.

use mc2_abl::AblRng;
use mc2_sim::rng::RngMode;
use mc2_sim::World;

#[test]
fn world_creates_and_shares_rng_with_abl() {
    let w = World::new(RngMode::LegacyMsvc, 1);
    let mut host = w.rng.clone();
    // ABL random(n) draws from the sim stream: MSVC seed 1 -> 41, 18467, ...
    assert_eq!(host.random(100), 41);
    assert_eq!(w.rng.0.borrow().sim.draws(), 1);
    assert_eq!(w.rng.0.borrow().visual.draws(), 0);
    // random(0) consumes a draw and returns 0, like the pinned RandomNumber(0); random(-7) follows C `%`
    assert_eq!(host.random(0), 0);
    assert_eq!(w.rng.0.borrow().sim.draws(), 2);
    let v = host.random(-7);
    assert!((0..7).contains(&v));
    assert_eq!(w.rng.0.borrow().sim.draws(), 3);
    // reseeding from a script reseeds the sim stream only
    host.seed(1);
    assert_eq!(w.rng.0.borrow().sim.draws(), 0);
}

// Compile-time check of the borrow patterns the lane interfaces rely on (review round 1, finding 4): the pilot lives in
// `w.pilots`, objects in `w.objects`, appearances in `w.appearance`; free functions take the whole world.
#[allow(dead_code)]
fn borrow_patterns(w: &mut World, h: mc2_sim::ObjectHandle) {
    // split borrow of two world fields
    let pos = w.objects.get(h).map(|o| o.c.position);
    mc2_sim::ai::pause_path(w, 1);
    let _ = pos;
    // take / use the world / put back
    if let Some(a) = w.appearance.take(h) {
        let _scalar = w.clock.frame_length;
        w.appearance.put(h, a);
    }
    // free functions on the whole world
    let _ = mc2_sim::appearance::velocity_magnitude(w, h);
    mc2_sim::appearance::update(w, h, true);
    mc2_sim::ai::rethink_path(w, h, 0);
    // single clock
    let _ = w.clock.scenario_time;
    // Surface the explicit TODO status; this compile fixture does not hide it.
    let _ = mc2_sim::ai::pilot_update(w, h);
}

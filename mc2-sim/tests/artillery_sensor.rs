//! Sensor artillery strike refusal before mission object types are loaded. A
//! real sensor strike needs its shipped `ArtilleryType`; its production
//! behaviour (taking a sensor, revealing a contact, and expiring on time) is
//! covered through `Sim::step` in `tests/artillery_sensor_strike.rs`.

use mc2_sim::combat::artillery::{call_strike, call_strike_seconds};
use mc2_sim::combat::artillery_math::ARTILLERY_SENSOR;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::Vec3;
use mc2_sim::object::ObjectStore;
use mc2_sim::rng::RngMode;
use mc2_sim::World;

/// A pool with the shipped number of free sensors, as `sensors::init` builds one.
fn world_with_pool() -> World {
    let mut w = World::new(RngMode::LegacyMsvc, 1);
    mc2_sim::ai::sensors::init(&mut w);
    w.teams.teams[0] = Some(crate_team());
    w
}

fn crate_team() -> mc2_sim::ai::team::Team {
    mc2_sim::ai::team::Team::default()
}

#[test]
fn sensor_strike_without_a_loaded_type_is_refused_without_taking_a_sensor() {
    let mut w = world_with_pool();

    assert!(!call_strike(&mut w, 0, ARTILLERY_SENSOR, Vec3::default()));
    assert_eq!(
        mc2_sim::ai::sensors::free_sensor_slot(&w),
        Some(0),
        "a missing type cannot create a partial sensor strike"
    );
    assert!(w
        .objects
        .artillery
        .iter()
        .all(|h| w.objects.get(*h).is_none_or(|o| !o.exists())));
}

/// A sensor strike with the pool full creates no strike (the C++ `Fatal`s on "No More Free Sensors"; Rust refuses and reports).
#[test]
fn sensor_strike_with_a_full_pool_is_refused() {
    let mut w = world_with_pool();
    for sensor in &mut w.sensors.pool {
        sensor.owner = ObjectHandle(1);
    }
    let slot_before = ObjectStore::next_pool_slot(&mut w, mc2_sim::object::Pool::Artillery);
    assert!(!call_strike_seconds(
        &mut w,
        0,
        ARTILLERY_SENSOR,
        Vec3::default(),
        -1
    ));
    assert!(
        w.combat_unavailable_frame
            .contains(&mc2_sim::combat::artillery::SENSOR_POOL_EXHAUSTED_HOOK),
        "the refusal is reported"
    );
    assert_eq!(
        slot_before,
        ObjectStore::next_pool_slot(&mut w, mc2_sim::object::Pool::Artillery),
        "the strike pool cursor did not move: no strike object was created"
    );
    assert!(
        !w.objects
            .artillery
            .iter()
            .any(|h| w.objects.get(*h).is_some_and(|o| o.exists())),
        "no strike object exists"
    );
}

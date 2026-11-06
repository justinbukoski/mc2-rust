//! A data-less world cannot fabricate an artillery strike: only loaded object
//! data supplies the rules. The production `Sim::step` coverage is in
//! `tests/artillery_sensor_strike.rs`.

use mc2_sim::combat::artillery::call_strike;
use mc2_sim::combat::artillery_math::ARTILLERY_SENSOR;
use mc2_sim::math::Vec3;
use mc2_sim::rng::RngMode;
use mc2_sim::World;

#[test]
fn sensor_strike_does_not_fabricate_missing_type_data() {
    let mut w = World::new(RngMode::LegacyMsvc, 1);
    mc2_sim::ai::sensors::init(&mut w);
    w.teams.teams[0] = Some(mc2_sim::ai::team::Team::default());

    assert!(!call_strike(&mut w, 0, ARTILLERY_SENSOR, Vec3::default()));
    assert_eq!(mc2_sim::ai::sensors::free_sensor_slot(&w), Some(0));
    assert!(w
        .objects
        .artillery
        .iter()
        .all(|h| w.objects.get(*h).is_none_or(|o| !o.exists())));
}

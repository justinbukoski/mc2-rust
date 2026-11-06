// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Loads the shipped `mc2.fx` and checks it against the facts of the C++ loader. Skips without
//! `MC2_DATA`.

use mc2_formats::vfs::Vfs;
use mc2_fx::library::{EffectLibrary, LIBRARY_PATH};
use mc2_fx::spec::{EffectClass, SpecKind};
use mc2_fx::weaponfx::{WeaponEffects, EFFECTS_CSV_PATH, NONE};

fn open() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Vfs::from_system_cfg(root).ok()
}

fn library(vfs: &mut Vfs) -> Option<EffectLibrary> {
    // The installed data may retain `effect.fst` only as a zero-byte optional
    // placeholder. `Vfs::from_system_cfg` correctly leaves that archive
    // unmounted, so this real-data test has no library to inspect.
    let bytes = vfs.read(LIBRARY_PATH).unwrap()?;
    Some(EffectLibrary::load(&bytes).expect("every effect in mc2.fx loads"))
}

#[test]
fn loads_every_effect_with_counts_per_class() {
    let Some(mut vfs) = open() else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let Some(lib) = library(&mut vfs) else {
        eprintln!("mc2.fx unavailable: skipping");
        return;
    };
    assert_eq!(lib.version, 17);
    assert_eq!(lib.len(), 904);
    assert_eq!(
        lib.counts_by_class(),
        vec![
            (EffectClass::Effect, 231),
            (EffectClass::PointCloud, 2),
            (EffectClass::ShardCloud, 24),
            (EffectClass::CardCloud, 388),
            (EffectClass::ShapeCloud, 9),
            (EffectClass::EffectCloud, 39),
            (EffectClass::Card, 136),
            (EffectClass::Shape, 10),
            (EffectClass::Tube, 31),
            (EffectClass::DebrisCloud, 34),
        ]
    );
}

#[test]
fn references_between_effects_resolve() {
    let Some(mut vfs) = open() else {
        return;
    };
    let Some(lib) = library(&mut vfs) else {
        eprintln!("mc2.fx unavailable: skipping");
        return;
    };
    for (i, spec) in lib.specs().iter().enumerate() {
        assert_eq!(spec.base.effect_id as usize, i);
        for ev in &spec.base.events {
            assert!(
                (ev.effect_id as usize) < lib.len(),
                "{}: event refers to effect {}",
                spec.name(),
                ev.effect_id
            );
            assert!(
                (0.0..=1.0).contains(&ev.time),
                "{}: event time",
                spec.name()
            );
        }
        if let SpecKind::EffectCloud(c) = &spec.kind {
            assert!(
                (c.particle_effect_id as usize) < lib.len(),
                "{}",
                spec.name()
            );
        }
    }
}

#[test]
fn weapon_effect_names_resolve_in_the_library() {
    let Some(mut vfs) = open() else {
        return;
    };
    let Some(lib) = library(&mut vfs) else {
        eprintln!("mc2.fx unavailable: skipping");
        return;
    };
    let csv = vfs.read(EFFECTS_CSV_PATH).unwrap().expect("effects.csv");
    let table = WeaponEffects::from_csv_bytes(&csv).unwrap();
    assert!(table.len() > 76);
    assert_eq!(table.effect_name(4), "ppc_trail");
    assert_eq!(table.effect_name(39), "Mech_Explosion");
    let mut missing = Vec::new();
    for id in 0..table.len() as i64 {
        for name in [
            table.effect_name(id),
            table.muzzle_flash_name(id),
            table.hit_effect_name(id),
            table.miss_effect_name(id),
        ] {
            if name != NONE && lib.find(name).is_none() {
                missing.push((id, name.to_string()));
            }
        }
    }
    // Real gaps in the shipped data: the game's `Find` returns NULL for these and skips the
    // effect ("NONE " has a trailing space, so the game's `strcmp(.., "NONE")` does not match).
    assert_eq!(
        missing,
        vec![
            (26, "red_las_hit".to_string()),
            (26, "red_las_flare".to_string()),
            (64, "NONE ".to_string()),
        ]
    );
}

/// The translucent cone around a large explosion is `Blast_Ring` (spawned by `Kaboom` at 0.02 s,
/// `Mech_Explosion` -> `Kaboom`): an open three-ring frustum whose own state is opaque with no
/// z-write/z-compare, drawn through the effect's additive state (`MLRState::Combine`,
/// mlrstate.cpp:205-221). Its texture `FX_Blast` is opaque (alpha 255), so the blend, not the
/// texture alpha, is what makes it translucent.
#[test]
fn blast_ring_is_an_additive_untextured_effect_state_over_an_fx_blast_frustum() {
    let Some(mut vfs) = open() else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let Some(lib) = library(&mut vfs) else {
        return;
    };
    let spec = lib.find("Blast_Ring").expect("Blast_Ring");
    assert_eq!(spec.base.state.blend(), mc2_fx::state::BlendMode::OneOne);
    assert!(spec.base.state.texture.is_none());
    let SpecKind::Shape(shape) = &spec.kind else {
        panic!("Blast_Ring is a Shape");
    };
    let prim = &shape.shape.primitives[0];
    assert_eq!(prim.state.texture.as_ref().unwrap().name, "FX_Blast");
    assert_eq!(prim.coords.len(), 42);
    assert_eq!(prim.index.len() / 3, 52);
    let combined = mc2_fx::state::MlrState::combine(&spec.base.state, &prim.state);
    assert_eq!(combined.blend(), mc2_fx::state::BlendMode::OneOne);
}

//! The object lines of the per-frame digest (oracle/DIGEST.md): `VA`, the mover lines (delegated to lane B), `B`, `T`, `G`,
//! `W`, `K` and the level-3 `OT` table.
//!
//! Levels: 1 writes `VA` and the mover lines; 2 adds the structure lines, `W` and `K`; 3 adds the `OT` line (handle,
//! part id and watch id table fingerprint, list fingerprints, counts); 4 adds one `OH` row per object.
//!
//! The structure lines carry trailing debug tokens (appended after the original tokens, DIGEST.md rule):
//! * `B ... d<damage> t<team> e<extent> pa<parentWID> bt<baseTileId> sp<special>`
//! * `T ... d<damage> r<turretRotation> e<extent> iw<idleWait> ix<x> iy<y> iz<z> tg<targetWID> pa<parentWID> fr<minRange>`
//! * `G ... o<opened><closed> e<extent> pa<parentWID> gs<opening><closing><lockedOpen><lockedClose> t<team>`

use crate::digest::{fmt_f32, LineBuilder};
use crate::ids::ObjectHandle;
use crate::World;

use super::{hooks, Kind, Obj};

/// FNV-1a 64-bit (offset 14695981039346656037, prime 1099511628211) over little-endian 32-bit words.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn mix(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

pub(crate) fn object_prefix(tag: &str, world: &World, o: &Obj) -> LineBuilder {
    let mut b = LineBuilder::new(tag);
    b.i(i64::from(o.c.part_id.0)).i(o.c.class as i64);
    b.f(o.c.position.x).f(o.c.position.y).f(o.c.position.z);
    b.f(o.c.rotation)
        .i(o.c.status as i64)
        .i(i64::from(o.c.flags))
        .pi(
            "iv",
            i64::from(hooks::appearance_in_view(world, o.c.handle)),
        );
    b
}

fn damage_of(o: &Obj) -> f32 {
    o.structure().map(|s| s.damage).unwrap_or(0.0)
}

/// `VA <hash> <activeBlocks> <activeVertices>`: the camera-derived terrain-object activity of the frame (oracle_main.cpp
/// `visHash`).
fn visibility_line(world: &World) -> String {
    let side = i64::from(super::terr::vertices_per_side(&world.terrain));
    let nv = side * side;
    let nb = i64::from(super::terr::num_obj_blocks(&world.terrain));
    let mut h: u64 = 1_469_598_103_934_665_603;
    let (mut active_v, mut active_b) = (0i64, 0i64);
    for i in 0..nv {
        if world.visibility.vertex_active(i as u32) {
            active_v += 1;
            h = (h ^ (i as u64)).wrapping_mul(1_099_511_628_211);
        }
    }
    for b in 0..nb {
        if world.visibility.block_active(b as u32) {
            active_b += 1;
            h = (h ^ (0x1_0000_0000u64 + b as u64)).wrapping_mul(1_099_511_628_211);
        }
    }
    format!("VA {h:x} {active_b} {active_v}")
}

/// All object lines for the current state.
pub fn object_lines(world: &World, level: u32, out: &mut Vec<String>) {
    out.push(visibility_line(world));
    for &h in &world.objects.movers {
        crate::mover::digest_mover(world, h, level, out);
    }
    structure_lines(world, level, out);
}

/// The lines after the movers: `B`, `T`, `G`, `W`, `K`, then `OT`/`OH`.
pub fn structure_lines(world: &World, level: u32, out: &mut Vec<String>) {
    if level >= 2 {
        let store = &world.objects;
        for &h in &store.buildings {
            let Some(o) = store.get(h) else { continue };
            if !o.exists() {
                continue;
            }
            let Kind::Building(b) = &o.kind else { continue };
            let mut l = object_prefix("B", world, o);
            l.s(&format!("d{}", fmt_f32(damage_of(o))))
                .pi("t", i64::from(b.team_id));
            l.s(&format!("e{}", fmt_f32(store.extent_radius(h))));
            l.pi("pa", i64::from(b.parent.0));
            l.pi("bt", i64::from(b.base_tile_id));
            l.pi(
                "sp",
                i64::from(super::structure::is_special_building(world, h)),
            );
            out.push(l.build());
        }
        for &h in &store.turrets {
            let Some(o) = store.get(h) else { continue };
            if !o.exists() {
                continue;
            }
            let Kind::Turret(t) = &o.kind else { continue };
            let mut l = object_prefix("T", world, o);
            l.s(&format!("d{}", fmt_f32(damage_of(o))))
                .s(&format!("r{}", fmt_f32(t.turret_rotation)));
            l.s(&format!("e{}", fmt_f32(store.extent_radius(h))));
            l.s(&format!("iw{}", fmt_f32(t.idle_wait)));
            l.s(&format!("ix{}", fmt_f32(t.idle_position.x)));
            l.s(&format!("iy{}", fmt_f32(t.idle_position.y)));
            l.s(&format!("iz{}", fmt_f32(t.idle_position.z)));
            l.pi("tg", i64::from(t.target.0));
            l.pi("pa", i64::from(t.parent.0));
            l.s(&format!("fr{}", fmt_f32(t.min_range)));
            out.push(l.build());
        }
        for &h in &store.gates {
            let Some(o) = store.get(h) else { continue };
            if !o.exists() {
                continue;
            }
            let Kind::Gate(g) = &o.kind else { continue };
            let mut l = object_prefix("G", world, o);
            l.s(&format!("o{}{}", i32::from(g.opened), i32::from(g.closed)));
            l.s(&format!("e{}", fmt_f32(store.extent_radius(h))));
            l.pi("pa", i64::from(g.parent.0));
            l.s(&format!(
                "gs{}{}{}{}",
                i32::from(g.opening),
                i32::from(g.closing),
                i32::from(g.locked_open),
                i32::from(g.locked_close)
            ));
            l.pi("t", i64::from(g.team_id));
            out.push(l.build());
        }
        let live = store
            .weapons
            .iter()
            .filter(|&&h| store.get(h).is_some_and(|o| o.exists()))
            .count();
        out.push(format!("W {live}"));
        if level >= 5 {
            // `WB`: bolts that exist and have not hit (a bolt that hit lingers for its hit effect, `bolt_flight::hit_effect_frames`).
            let in_flight = store
                .weapons
                .iter()
                .filter(|&&h| {
                    store.get(h).is_some_and(|o| {
                        o.exists() && matches!(&o.kind, Kind::WeaponBolt(b) if !b.hit_target)
                    })
                })
                .count();
            out.push(format!("WB {in_flight}"));
        }
        let mut kc = 0;
        let mut kt = 0;
        for &h in &store.carnage {
            if let Some(o) = store.get(h) {
                if o.exists() {
                    kc += 1;
                    if o.c.tangible() {
                        kt += 1;
                    }
                }
            }
        }
        out.push(format!("K {kc} {kt}"));
    }
    if level >= 3 {
        // KD <handle> <x y z> <timer> <extent> t<tangible> x<effectId> <gosEffect>: one row per live carnage object
        for &h in &world.objects.carnage {
            if let Some(o) = world.objects.get(h) {
                if let (true, Kind::Carnage(c)) = (o.exists(), &o.kind) {
                    out.push(format!(
                        "KD {} {} {} {} {} {} t{} x{} {}",
                        h.0,
                        fmt_f32(o.c.position.x),
                        fmt_f32(o.c.position.y),
                        fmt_f32(o.c.position.z),
                        fmt_f32(c.timer),
                        fmt_f32(c.extent_radius()),
                        i32::from(o.c.tangible()),
                        c.effect_id,
                        i32::from(c.has_effect)
                    ));
                }
            }
        }
        out.push(table_line(world));
    }
    if level >= 4 {
        let store = &world.objects;
        for i in 1..=store.max_objects() {
            let h = ObjectHandle(i as u32);
            match store.get(h) {
                Some(o) => out.push(format!(
                    "OH {} {} {} {} {} {} {}",
                    i,
                    o.c.part_id.0,
                    o.c.watch_id.0,
                    o.c.class as i32,
                    i32::from(o.exists()),
                    o.c.type_handle,
                    o.c.handle.0
                )),
                None => out.push(format!("OH {i} none")),
            }
        }
    }
}

/// `OT <maxObjects> <nextWatchId> <mechs> <vehicles> <terrain> <buildings> <turrets> <gates> <special> <power> <turretCtl>
/// <gateCtl> <objHash> <blockHash> <listHash>`: the handle/part id/watch id table and the list structure, fingerprinted.
pub fn table_line(world: &World) -> String {
    let store = &world.objects;
    let mut oh = Fnv::new();
    for i in 1..=store.max_objects() {
        match store.get(ObjectHandle(i as u32)) {
            Some(o) => {
                oh.mix(1);
                oh.mix(o.c.part_id.0 as u32);
                oh.mix(o.c.watch_id.0);
                oh.mix(o.c.class as i32 as u32);
                oh.mix(u32::from(o.exists()));
                oh.mix(o.c.type_handle as u32);
                oh.mix(o.c.handle.0);
            }
            None => oh.mix(0),
        }
    }
    let mut bh = Fnv::new();
    for b in &store.blocks {
        bh.mix(b.first_handle);
        bh.mix(b.num_objects);
        bh.mix(b.num_collidable);
    }
    let mut lh = Fnv::new();
    let lists: [&[ObjectHandle]; 13] = [
        &store.mechs,
        &store.vehicles,
        &store.terrain_objects,
        &store.buildings,
        &store.turrets,
        &store.gates,
        &store.special_buildings,
        &store.movers,
        &store.good_movers,
        &store.bad_movers,
        &store.power_generators,
        &store.turret_controls,
        &store.gate_controls,
    ];
    for l in lists {
        lh.mix(l.len() as u32);
        for h in l {
            lh.mix(h.0);
        }
    }
    format!(
        "OT {} {} {} {} {} {} {} {} {} {} {} {} {:x} {:x} {:x}",
        store.max_objects(),
        store.next_watch_id,
        store.num_mechs,
        store.num_vehicles,
        store.terrain_objects.len(),
        store.buildings.len(),
        store.turrets.len(),
        store.gates.len(),
        store.special_buildings.len(),
        store.power_generators.len(),
        store.turret_controls.len(),
        store.gate_controls.len(),
        oh.0,
        bh.0,
        lh.0
    )
}

/// `SH <hashB> <hashT> <hashG> <countB> <countT> <countG>` over the `B`, `T` and `G` lines of `lines` (FNV-1a 64 over each
/// line plus a newline); the committed goldens store this instead of the lines (`oracle-tools hash-structures`).
pub fn structure_hash_line(lines: &[String]) -> Option<String> {
    let mut hashes = [0xcbf2_9ce4_8422_2325u64; 3];
    let mut counts = [0usize; 3];
    for l in lines {
        let idx = match l.split(' ').next() {
            Some("B") => 0,
            Some("T") => 1,
            Some("G") => 2,
            _ => continue,
        };
        counts[idx] += 1;
        for b in l.bytes().chain(std::iter::once(b'\n')) {
            hashes[idx] = (hashes[idx] ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    if counts == [0, 0, 0] {
        return None;
    }
    Some(format!(
        "SH {:x} {:x} {:x} {} {} {}",
        hashes[0], hashes[1], hashes[2], counts[0], counts[1], counts[2]
    ))
}

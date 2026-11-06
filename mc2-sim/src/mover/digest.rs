//! The `M` / `V` digest lines of movers (oracle/DIGEST.md, `emitMover` in oracle/oracle_main.cpp).
//!
//! ```text
//! M <part> <class> <x y z> <rot> <status> <flags> iv<0|1> t<team> c<cmdr> <vx vy vz> <timeLeft>
//!   A <curArmor...> S <curInternalStructure...> [W <wounds> o<order> g<stage> tw<target wid> tg<lastTargetWID>] [I <master:health:amount:ready...>]
//! ```
//!
//! Written from the state the world holds, nothing else: armor / structure / inventory from the mover's [`Loadout`](crate::combat::Loadout),
//! the `W` group from its pilot (only when the mover has one, as `if (p)` in the oracle), `tg` from the pilot's `lastTarget` field read
//! directly (the C++ getter clears dead targets, so the digest never calls it). Level 2 adds the `I` group.

use crate::combat::Loadout;
use crate::ids::ObjectHandle;
use crate::object::Kind;
use crate::World;

/// `MAX_MOVER_INVENTORY_ITEMS`.
const MAX_MOVER_INVENTORY_ITEMS: usize = 80;

/// Append the `M` (mech) or `V` (ground vehicle) line of mover `h` to `out`.
pub fn digest_mover(w: &World, h: ObjectHandle, level: u32, out: &mut Vec<String>) {
    let Some(o) = w.objects.get(h) else { return };
    let (tag, core, loadout): (&str, _, &Loadout) = match &o.kind {
        Kind::Mech(m) => ("M", &m.m, &m.loadout),
        Kind::Vehicle(v) => ("V", &v.m, &v.loadout),
        _ => return,
    };
    let mut b = crate::object::digest::object_prefix(tag, w, o);
    b.pi("t", i64::from(core.team.0))
        .pi("c", i64::from(core.commander.0));
    b.f(core.velocity.x).f(core.velocity.y).f(core.velocity.z);
    b.f(core.time_left);
    b.s("A");
    for &(cur, _) in &loadout.armor {
        b.f(cur);
    }
    b.s("S");
    for l in &loadout.body {
        b.f(l.cur_internal_structure);
    }
    // `if (p)`: a mover without a pilot has no W group. Slot 0 is never handed out.
    if core.pilot_slot != 0 {
        if let Some(p) = w.pilots.get(core.pilot_slot) {
            b.s("W")
                .f(p.wounds)
                .pi("o", p.cur_order.code as i64)
                .pi("g", i64::from(p.cur_order.stage))
                .pi("tw", i64::from(p.cur_order.target.0))
                .pi("tg", i64::from(p.last_target.0 .0));
        }
    }
    if level >= 2 {
        b.s("I");
        let n = (loadout.counts.0 as usize + loadout.counts.1 as usize + loadout.counts.2 as usize)
            .min(MAX_MOVER_INVENTORY_ITEMS);
        for it in loadout.inventory.iter().take(n) {
            b.s(&format!(
                "{}:{}:{}:{}",
                it.master_id,
                it.health,
                it.amount,
                crate::digest::fmt_f32(it.ready_time)
            ));
        }
    }
    out.push(b.build());
}

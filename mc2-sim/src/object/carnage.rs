// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Carnage`: pooled explosion and fire objects (carnage.cpp). Instances keep `objectClass == GameObject`.
//!
//! The 50-slot pool lives in [`ObjectStore::carnage`](super::ObjectStore); `GameObjectManager::getCarnage` /
//! `createExplosion` are in `store.rs`. `JUSTCREATED` is cleared on the first update, the `DelayUntilCollidable` timer counts
//! down by `frameLength`, and the object is tangible only on the update where the timer reaches zero.
//!
//! **gosFX stand-in.** An explosion whose weapon-effects row names a gosFX effect (anything but `NONE`) owns a
//! `gosFX::Effect` in the original; `Carnage::update` keeps the object alive until `gosEffect->Execute` returns false
//! (carnage.cpp:768-790), and an explosion without one ends as soon as the delay has run out (carnage.cpp:795). The effect
//! library (lifespan curves, particle children, its private random stream) is not ported: here an effect lives for one
//! update after the delay ran out, which keeps the explosion alive through the update where it is tangible (that is what
//! collides) and ends it one frame later. The real lifetimes are longer (the oracle's `K` line shows 15 to 60+ frames), so
//! `K`, and the pool slot reuse of a long battle, differ; see the porting log.
//!
//! `ExplosionType::handleCollision` (damage delivery, carnage.cpp:343) is [`explosion_handle_collision`]; it calls lane C's
//! `calc_hit_location` / `handle_weapon_hit`. Fires are dead code in the shipped game (`createFire` has no callers that
//! matter) and only their pool bookkeeping is kept.

use super::{flags, hooks, Kind, ObjectClass, ObjectStore};
use crate::combat::{AttackSource, WeaponShotInfo};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::{Vec3, Vec3Ext};

/// `EXPLOSION_TIME`.
pub const EXPLOSION_TIME: f32 = 0.5;
/// `MINE_EXPLOSION_ID` (mclib/weaponfx.h:24).
pub const MINE_EXPLOSION_ID: i32 = 53;

/// `CarnageEnumType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CarnageType {
    /// `CARNAGE_NONE`.
    #[default]
    None,
    /// `CARNAGE_FIRE`.
    Fire,
    /// `CARNAGE_EXPLOSION`.
    Explosion,
}

/// Explosion or fire instance state (`Carnage` data members; the fire/explosion `info` union is flattened to the three
/// words they share: `timer`/`timeToBurn`, `radius`, `chunkSize`).
#[derive(Clone, Debug)]
pub struct CarnageState {
    /// `carnageType`.
    pub carnage_type: CarnageType,
    /// `effectId` (index into the weapon effects table).
    pub effect_id: i32,
    /// `ownerWID`.
    pub owner: WatchId,
    /// `info.explosion.timer`: seconds until the explosion becomes damage-capable (`delayUntilCollidable`).
    pub timer: f32,
    /// `info.explosion.radius` (per-instance extent radius).
    pub radius: f32,
    /// `info.explosion.chunkSize`.
    pub chunk_size: f32,
    /// `gosEffect != NULL` (the effect row's name is not `NONE`).
    pub has_effect: bool,
    /// Updates run since the delay timer reached zero (the gosFX stand-in's clock).
    pub post_delay_updates: u32,
}

impl Default for CarnageState {
    fn default() -> Self {
        CarnageState::new()
    }
}

impl CarnageState {
    /// `Carnage::init(bool create)` (carnage.cpp:442): the state of a freshly constructed or reset carnage object.
    pub fn new() -> CarnageState {
        CarnageState {
            carnage_type: CarnageType::None,
            effect_id: 0,
            owner: WatchId::NONE,
            // info.fire.* are zeroed first, then info.explosion.timer = EXPLOSION_TIME overwrites timeToBurn;
            // chunkSize aliases reallyVisible and is zeroed.
            timer: EXPLOSION_TIME,
            radius: 0.0,
            chunk_size: 0.0,
            has_effect: false,
            post_delay_updates: 0,
        }
    }

    /// `Carnage::init(bool create)`: reset everything but the effect id (that one is never touched by `init`).
    pub fn reset(&mut self) {
        let effect_id = self.effect_id;
        *self = CarnageState::new();
        self.effect_id = effect_id;
    }

    /// `Carnage::init(CarnageEnumType)` (carnage.cpp:459): sets the type.
    pub fn init_type(&mut self, t: CarnageType) {
        self.carnage_type = t;
    }

    /// `getExtentRadius()`: the instance radius for fires and explosions, else 0.
    pub fn extent_radius(&self) -> f32 {
        match self.carnage_type {
            CarnageType::Fire | CarnageType::Explosion => self.radius,
            CarnageType::None => 0.0,
        }
    }

    /// `setExtentRadius(r)`.
    pub fn set_extent_radius(&mut self, r: f32) {
        match self.carnage_type {
            CarnageType::Fire | CarnageType::Explosion => self.radius = r,
            CarnageType::None => {}
        }
    }
}

/// `Carnage::finishNow()` (carnage.cpp:608): a fire spreads when it goes out; an explosion is reset (`init(false)`).
pub fn finish_now(world: &mut crate::World, h: ObjectHandle) {
    let Some(o) = world.objects.get_mut(h) else {
        return;
    };
    if let Kind::Carnage(c) = &mut o.kind {
        match c.carnage_type {
            CarnageType::Fire => o.c.set_flag(flags::SPREAD, true),
            CarnageType::Explosion => c.reset(),
            CarnageType::None => {}
        }
    }
}

/// `explosion->init(effectId); explosion->init(false, objectType)` (objmgr.cpp:2677-2678, carnage.cpp:655): set the effect id,
/// then `GameObject::init(true, type)` (type handle, class `GameObject`), `JUSTCREATED`, and for explosions the chunk size,
/// the collidable delay timer and `tangible = false`.
pub fn init_instance(world: &mut crate::World, h: ObjectHandle, effect_id: i32, type_num: i32) {
    // `S_stricmp(weaponEffects->GetEffectName(effectId), "NONE") != 0` creates the gosFX effect.
    let has_effect = !world
        .objects
        .effect_name(effect_id)
        .eq_ignore_ascii_case("NONE");
    let (chunk, delay) = match world.objects.type_by_num(type_num).map(|t| &t.data) {
        Some(mc2_sim_data::objtype::ObjectTypeData::Explosion(e)) => {
            (Some(e.chunk_size), e.delay_until_collidable)
        }
        _ => (None, EXPLOSION_TIME),
    };
    let Some(o) = world.objects.get_mut(h) else {
        return;
    };
    let Kind::Carnage(c) = &mut o.kind else {
        return;
    };
    c.effect_id = effect_id;
    c.has_effect = has_effect;
    c.post_delay_updates = 0;
    o.c.type_handle = type_num;
    o.c.class = ObjectClass::GameObject;
    o.c.set_flag(flags::JUSTCREATED, true);
    if c.carnage_type == CarnageType::Explosion {
        if let Some(chunk) = chunk {
            c.chunk_size = chunk;
        }
        c.timer = delay;
        o.c.set_tangible(false);
    }
}

/// `Carnage::setOwner(owner)`: stores the owner's watch id (assigning one on first use, which fixes the numbering order).
pub fn set_owner(world: &mut crate::World, h: ObjectHandle, owner: Option<ObjectHandle>) {
    let wid = match owner {
        Some(oh) => world.objects.watch_id(oh),
        None => WatchId::NONE,
    };
    if let Some(o) = world.objects.get_mut(h) {
        if let Kind::Carnage(c) = &mut o.kind {
            c.owner = wid;
        }
    }
}

/// `Carnage::update()` (carnage.cpp:~700-840) without the gosFX effect and light (presentation only).
pub fn update(world: &mut crate::World, h: ObjectHandle) -> bool {
    let frame_length = world.clock.frame_length;
    let (ctype, owner, just_created) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Carnage(c) => (c.carnage_type, c.owner, o.c.flag(flags::JUSTCREATED)),
            _ => return true,
        },
        None => return true,
    };
    match ctype {
        CarnageType::Fire | CarnageType::None => true,
        CarnageType::Explosion => {
            if just_created {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_flag(flags::JUSTCREATED, false);
                    // gosEffect is NULL headless; setExists(true); sound/light are presentation.
                    o.set_exists(true);
                }
            }
            // Attach the explosion to the thing dying.
            if !owner.is_none() {
                if let Some(pos) = world.objects.get_by_watch_id(owner).map(|oo| oo.c.position) {
                    if let Some(o) = world.objects.get_mut(h) {
                        o.c.position = pos;
                    }
                }
            }
            let Some(o) = world.objects.get_mut(h) else {
                return true;
            };
            let Kind::Carnage(c) = &mut o.kind else {
                return true;
            };
            o.c.set_flag(flags::TANGIBLE, false);
            if c.timer > 0.0 {
                c.timer -= frame_length;
                if c.timer <= 0.0 {
                    o.c.set_flag(flags::TANGIBLE, true);
                }
            }
            // With a gosEffect the explosion lives while the effect runs (stand-in: one update past the delay, so the update
            // where it is tangible still collides); without one it is over once the delay has run out.
            if c.timer <= 0.0 {
                c.post_delay_updates += 1;
                if !c.has_effect || c.post_delay_updates > 1 {
                    return false;
                }
            }
            true
        }
    }
}

/// `GameObject::getBlockAndVertexNumber` for objects that do not store them (carnage, bolts, movers): derived from the
/// position (gameobj.cpp:1572). Terrain objects, buildings, turrets and gates return their stored (file-derived) numbers.
pub fn block_and_vertex_number(world: &crate::World, h: ObjectHandle) -> (i32, i32) {
    let Some(o) = world.objects.get(h) else {
        return (0, 0);
    };
    if let Some(s) = o.structure() {
        return (s.block_number, s.vertex_number);
    }
    let t = &world.terrain;
    let half = super::terr::vertices_per_side(t) >> 1;
    let blocks_side = super::terr::blocks_per_side(t);
    let block_side = super::terr::VERTICES_BLOCK_SIDE;
    let one_over_block_side: f32 = 1.0f32 / block_side as f32;
    // float2long truncates (mathfunc.h:115)
    let mx = ((o.c.position.x as i32) >> 7) + half;
    let block_x = (mx as f32 * one_over_block_side) as i32;
    let my = half - (((o.c.position.y as i32) >> 7) + 1);
    let block_y = (my as f32 * one_over_block_side) as i32;
    let block_num = block_x + block_y * blocks_side;
    let vertex_x = mx - block_x * block_side;
    let vertex_y = my - block_y * block_side;
    (block_num, vertex_x + vertex_y * block_side)
}

/// `Carnage::handleStaticCollision()` (carnage.cpp:509): an explosion that is tangible this frame is tested against every
/// object of its own 20x20 block and the eight blocks around it (not gates or turrets), then sets off mines in a 9x9 cell
/// neighbourhood.
pub fn handle_static_collision(world: &mut crate::World, h: ObjectHandle) {
    let (tangible, effect_id, radius, position) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Carnage(c) => (o.c.tangible(), c.effect_id, c.radius, o.c.position),
            _ => return,
        },
        None => return,
    };
    if !tangible {
        return;
    }
    let (block_number, _vertex) = block_and_vertex_number(world, h);
    let blocks_side = super::terr::blocks_per_side(&world.terrain);
    let top_left = block_number - blocks_side - 1;
    let mut current = top_left;
    let total_blocks = blocks_side * blocks_side;
    for i in 0..3 {
        for _j in 0..3 {
            if current >= 0 && current < total_blocks {
                let (first, count) = match world.objects.blocks.get(current as usize) {
                    Some(b) => (b.first_handle, b.num_objects),
                    None => (0, 0),
                };
                for idx in 0..count {
                    let oh = ObjectHandle(first + idx);
                    let ok = world.objects.get(oh).is_some_and(|obj| {
                        obj.exists()
                            && obj.c.class != ObjectClass::Gate
                            && obj.c.class != ObjectClass::Turret
                    });
                    if ok {
                        super::collision::detect_static_collision(world, h, oh);
                    }
                }
            }
            current += 1;
        }
        current = top_left + blocks_side * (i + 1);
    }

    // Mines: radius in cells, unless we are a mine explosion.
    if effect_id != MINE_EXPLOSION_ID && radius > 0.0 {
        let (cell_row, cell_col) = super::terr::world_to_cell(&world.terrain, &position);
        let start_row = cell_row - 4;
        let start_col = cell_col - 4;
        for i in start_row..start_row + 9 {
            for j in start_col..start_col + 9 {
                if super::terr::in_bounds(&world.terrain, i, j)
                    && super::terr::get_mine(&world.terrain, i, j) == 1
                {
                    let mine_pos = super::terr::cell_to_world(&world.terrain, i, j);
                    let mut d = Vec3 {
                        x: position.x - mine_pos.x,
                        y: position.y - mine_pos.y,
                        z: position.z - mine_pos.z,
                    };
                    d.z = 0.0;
                    let dist = crate::math::approximate_length(&d);
                    if dist < radius {
                        super::terr::set_mine(&mut world.terrain, i, j, 2);
                        let (dmg, range) = super::hooks::mine_splash(world);
                        ObjectStore::create_explosion(
                            world,
                            MINE_EXPLOSION_ID,
                            None,
                            mine_pos,
                            dmg,
                            range * crate::terrain::WORLD_UNITS_PER_METER,
                        );
                    }
                }
            }
        }
    }
}

/// `ExplosionType::handleCollision(collidee, collider)` (carnage.cpp:343): chunked damage to whatever the explosion touches.
/// Always returns `false` (an explosion never removes itself through a collision).
pub fn explosion_handle_collision(
    world: &mut crate::World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let (damage_left0, chunk_size, c_pos, c_extent) = match world.objects.get(collidee) {
        Some(o) => match &o.kind {
            Kind::Carnage(c) => (
                o.c.expl_damage,
                c.chunk_size,
                o.c.position,
                world.objects.extent_radius(collidee),
            ),
            _ => return false,
        },
        None => return false,
    };
    // `chunkSize` inside ExplosionType::handleCollision is the type's member; instances copy it at init.
    if damage_left0 == 0.0 {
        return false;
    }
    let damage_amount = if damage_left0 > chunk_size {
        chunk_size
    } else {
        damage_left0
    };
    let (is_mover, class, k_pos) = match world.objects.get(collider) {
        Some(o) => (o.is_mover(), o.c.class, o.c.position),
        None => return false,
    };
    let shot = |damage: f32, hit_location: i32| WeaponShotInfo {
        attacker: WatchId::NONE,
        master_id: -1,
        damage,
        hit_location,
        entry_angle: 0.0,
    };
    let mut damage_left = damage_left0;
    if is_mover {
        let mut s = shot(damage_amount, 0);
        while damage_left > 0.0 {
            s.hit_location =
                hooks::calc_hit_location(world, collider, collidee, -1, AttackSource::Artillery, 0);
            hooks::weapon_hit(world, collider, &s);
            damage_left -= chunk_size;
        }
    } else {
        match class {
            ObjectClass::Gate | ObjectClass::Turret => {
                // An explosion may not damage a gate or turret unless it is within the "little" extent radius.
                let d = Vec3 {
                    x: k_pos.x - c_pos.x,
                    y: k_pos.y - c_pos.y,
                    z: k_pos.z - c_pos.z,
                };
                let range = d.length();
                let little = super::structure::little_extent(world, collider);
                if little < range {
                    let real_range = range - little;
                    if real_range > c_extent {
                        return false;
                    }
                }
                let mut s = shot(damage_amount, 0);
                while damage_left > 0.0 {
                    s.hit_location = 0;
                    hooks::weapon_hit(world, collider, &s);
                    damage_left -= chunk_size;
                }
            }
            _ => {
                let s = shot(damage_left0, 0);
                hooks::weapon_hit(world, collider, &s);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RngMode;
    use crate::World;

    /// A world whose first carnage slot is a live explosion with `timer` seconds of collision delay.
    fn explosion(timer: f32, has_effect: bool) -> (World, ObjectHandle) {
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        let rng = w.rng.clone();
        w.objects.set_num_objects(&rng, 0, 0);
        let h = w.objects.carnage[0];
        let o = w.objects.get_mut(h).unwrap();
        o.set_exists(true);
        o.c.set_flag(flags::JUSTCREATED, true);
        let Kind::Carnage(c) = &mut o.kind else {
            panic!("not carnage")
        };
        c.init_type(CarnageType::Explosion);
        c.timer = timer;
        c.has_effect = has_effect;
        w.clock.frame_length = 0.25;
        (w, h)
    }

    fn tangible(w: &World, h: ObjectHandle) -> bool {
        w.objects.get(h).unwrap().c.tangible()
    }

    #[test]
    fn explosion_is_tangible_for_exactly_the_update_where_the_delay_runs_out() {
        let (mut w, h) = explosion(0.6, true);
        // 0.6 -> 0.35 -> 0.1 -> -0.15 (tangible) -> stays at -0.15 (no longer tangible)
        for expect in [false, false, true, false] {
            assert!(update(&mut w, h) || !expect, "alive while the effect runs");
            assert_eq!(tangible(&w, h), expect);
            if !w.objects.get(h).unwrap().exists() {
                break;
            }
        }
        // the first update cleared JUSTCREATED
        assert!(!w.objects.get(h).unwrap().c.flag(flags::JUSTCREATED));
    }

    #[test]
    fn explosion_without_an_effect_ends_when_the_delay_is_over() {
        let (mut w, h) = explosion(0.3, false);
        assert!(update(&mut w, h)); // 0.05 left
        assert!(!update(&mut w, h)); // -0.2: over, it never collides
    }

    #[test]
    fn explosion_with_an_effect_lives_one_more_update_after_the_delay() {
        let (mut w, h) = explosion(0.3, true);
        assert!(update(&mut w, h));
        assert!(update(&mut w, h)); // crossed: tangible, still alive for the collision update
        assert!(tangible(&w, h));
        assert!(!update(&mut w, h));
    }
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work.
// ==================================================================================================================

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

/// `CarnageEnumType` as one byte: 0 none, 1 fire, 2 explosion.
impl Snapshot for CarnageType {
    fn save(&self, w: &mut Writer) {
        w.u8(match self {
            CarnageType::None => 0,
            CarnageType::Fire => 1,
            CarnageType::Explosion => 2,
        });
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(match r.u8()? {
            0 => CarnageType::None,
            1 => CarnageType::Fire,
            2 => CarnageType::Explosion,
            t => return Err(SaveError(format!("save: bad carnage type tag {t}"))),
        })
    }
}

crate::snapshot_struct!(CarnageState {
    carnage_type,
    effect_id,
    owner,
    timer,
    radius,
    chunk_size,
    has_effect,
    post_delay_updates,
});

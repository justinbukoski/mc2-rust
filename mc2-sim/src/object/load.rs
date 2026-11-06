// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission load of the object manager: `countTerrainObjects`, `loadTerrainObjects` and `addObject` (objmgr.cpp:781-1265).
//!
//! The order matters for handles, watch ids and the RNG:
//! 1. [`ObjectStore::count_terrain_objects`] reads packet 1 of the mission pak, assigns every terrain object to a 20x20-vertex
//!    block and fixes each block's first handle;
//! 2. [`ObjectStore::set_num_objects`] allocates every object (turret constructors draw `idleWait`);
//! 3. the caller creates the movers (their watch ids come first);
//! 4. [`load_terrain_objects`] initialises each terrain object in file order, links turrets and gates to their control
//!    buildings, builds the special-building and power-generator lists and points lit things at their nearest generator;
//! 5. [`ObjectStore::build_mover_lists`].
//!
//! [`build_for_mission`] runs the sequence with a closure for step 3.

use std::rc::Rc;

use mc2_assets::map::{decode_packet, MissionPacket, TerrainObject as FileObject};
use mc2_formats::pak::PakFile;
use mc2_sim_data::objtype::building::BuildingSubType;
use mc2_sim_data::objtype::ObjectTable;

use super::store::{type_object_class, ObjBlock, ObjDataLoader};
use super::{flags, structure, terr, Kind, Obj, ObjectClass, ObjectStatus, ObjectStore};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::terrain::Terrain;
use crate::World;

/// Read the terrain-object records of a mission: packet 1 of `data/missions/<mission>.pak`.
pub fn read_mission_objects(
    vfs: &mut mc2_formats::vfs::Vfs,
    mission: &str,
) -> Result<Vec<FileObject>, String> {
    let name = format!("data/missions/{mission}.pak");
    let bytes = vfs
        .read(&name)
        .map_err(|e| format!("{name}: {e}"))?
        .ok_or_else(|| format!("{name}: not found"))?;
    let pak = PakFile::from_bytes(bytes).map_err(|e| format!("{name}: {e}"))?;
    let packet = pak
        .read_packet(1)
        .map_err(|e| format!("{name} packet 1: {e}"))?;
    match decode_packet(1, &packet).map_err(|e| format!("{name} packet 1: {}", e.0))? {
        MissionPacket::Objects(v) => Ok(v),
        _ => Err(format!("{name}: packet 1 is not the object packet")),
    }
}

/// `GameObjectManager::countTerrainObjects(pFile, firstHandle)` + the mission's call-site arguments: block assignment, class
/// counts and block handle ranges. `first_handle` is `(numMechs + 128) + (numVehicles + 128) + 1`.
impl ObjectStore {
    /// See the module docs.
    pub fn count_terrain_objects(
        &mut self,
        terrain: &Terrain,
        types: Rc<ObjectTable>,
        objects: &[FileObject],
        first_handle: u32,
    ) -> Result<(), String> {
        self.types = Some(types);
        let blocks_side = terr::blocks_per_side(terrain);
        let num_blocks = terr::num_obj_blocks(terrain).max(0) as usize;
        self.blocks = vec![ObjBlock::default(); num_blocks];
        self.count_terrain_objects_total = 0;
        self.count_buildings_total = 0;
        self.count_turrets_total = 0;
        self.count_gates_total = 0;
        self.pending.clear();

        for f in objects {
            let position = Vec3 {
                x: f.position[0],
                y: f.position[1],
                z: f.position[2],
            };
            // convert to block and vertex
            let (mut tile_row, mut tile_col) = terr::world_to_tile(terrain, &position);
            let block_i = tile_col / 20;
            let block_j = tile_row / 20;
            let block_number = block_j * blocks_side + block_i;
            tile_col -= block_i * 20;
            tile_row -= block_j * 20;
            let data = ObjDataLoader {
                obj_type_num: f.object_type,
                position,
                rotation: f.rotation,
                damage: f.damage,
                team_id: f.team_id,
                parent_id: f.parent_id,
                tile_col,
                tile_row,
                block_number,
                // data->vertexNumber = tileRow * 20 + tileRow (objmgr.cpp:839: the row is used twice)
                vertex_number: tile_row * 20 + tile_row,
            };
            self.count_object(&data)?;
            self.pending.push(data);
        }

        // fix up handles
        let mut cur = first_handle;
        for b in self.blocks.iter_mut() {
            b.first_handle = cur;
            cur += b.num_objects;
        }
        Ok(())
    }

    /// `countObject(data)` (objmgr.cpp:858).
    fn count_object(&mut self, data: &ObjDataLoader) -> Result<(), String> {
        let class = match self.type_by_num(data.obj_type_num) {
            Some(t) => type_object_class(t),
            None => return Ok(()),
        };
        let block = usize::try_from(data.block_number)
            .ok()
            .filter(|&b| b < self.blocks.len());
        let Some(block) = block else {
            return Err(format!(
                "terrain object of type {} is outside the map (block {})",
                data.obj_type_num, data.block_number
            ));
        };
        match class {
            ObjectClass::TerrainObject | ObjectClass::Tree => {
                self.blocks[block].num_collidable += 1;
                self.count_terrain_objects_total += 1;
            }
            ObjectClass::Turret => {
                self.blocks[block].num_collidable += 1;
                self.count_turrets_total += 1;
            }
            ObjectClass::Gate => {
                self.blocks[block].num_collidable += 1;
                self.count_gates_total += 1;
            }
            ObjectClass::Building | ObjectClass::TreeBuilding => {
                let special = match self.type_by_num(data.obj_type_num).map(|t| &t.data) {
                    Some(mc2_sim_data::objtype::ObjectTypeData::Building(b)) => {
                        (b.perimeter_alarm_range > 0.0 && b.perimeter_alarm_timer > 0.0)
                            || b.lookout_tower_range > 0.0
                            || b.sensor_range > 0.0
                    }
                    _ => false,
                };
                if special {
                    self.blocks[block].num_collidable += 1;
                }
                self.count_buildings_total += 1;
            }
            ObjectClass::Bridge => {
                self.count_terrain_objects_total += 1;
            }
            other => {
                return Err(format!(
                    "GameObjectManager.countTerrainObjects: bad object type (class {})",
                    other as i32
                ));
            }
        }
        self.blocks[block].num_objects += 1;
        Ok(())
    }
}

/// Load the real lane F building/tree appearances used by this mission. Run after
/// [`ObjectStore::count_terrain_objects`] and before [`load_terrain_objects`], so `addObject` can
/// create the corresponding per-object appearance as each structure is initialized.
pub fn load_structure_appearances(
    vfs: &mut mc2_formats::vfs::Vfs,
    world: &mut World,
) -> Result<(), String> {
    let requests: Vec<_> = world
        .objects
        .pending
        .iter()
        .filter_map(|d| {
            let t = world.objects.type_by_num(d.obj_type_num)?;
            let class = type_object_class(t);
            matches!(
                class,
                ObjectClass::TerrainObject
                    | ObjectClass::Tree
                    | ObjectClass::Building
                    | ObjectClass::TreeBuilding
                    | ObjectClass::Turret
                    | ObjectClass::Gate
            )
            .then(|| {
                (
                    class == ObjectClass::Tree,
                    t.base
                        .appearance_name
                        .clone()
                        .unwrap_or_else(|| "TESTBLDG".to_string()),
                )
            })
        })
        .collect();
    let effects = super::hooks::EffectLibraryNames::new(
        vfs.read("data/effects/mc2.fx")
            .map_err(|e| format!("data/effects/mc2.fx: {e}"))?
            .unwrap_or_default(),
    );
    for (tree, name) in requests {
        if tree {
            world
                .appearance
                .tree_type(vfs, &name)
                .map_err(|e| e.to_string())?;
        } else {
            let i = world
                .appearance
                .building_type(vfs, &name)
                .map_err(|e| e.to_string())?;
            let effect = &mut world.appearance.building_types[i].destruct_effect;
            if !effect.is_empty() && !effects.contains(effect) {
                effect.clear();
            }
        }
    }
    Ok(())
}

/// `WeaponEffects::init("Effects")` (weaponfx.cpp:30): `data/objects/effects.csv`, row `i + 2` of each effect: name (column 2),
/// effect object number (column 5). The muzzle, hit and miss names (columns 3, 4, 6) are lane C/F presentation data.
pub fn load_weapon_effects(
    vfs: &mut mc2_formats::vfs::Vfs,
    store: &mut ObjectStore,
) -> Result<(), String> {
    let path = "data/objects/effects.csv";
    let bytes = vfs
        .read(path)
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path}: not found"))?;
    let text = String::from_utf8_lossy(&bytes);
    let rows = crate::combat::weapon_fx::parse_weapon_effects(&text)
        .map_err(|e| format!("{path}: {e}"))?;
    store.effect_names = rows.iter().map(|r| r.names[0].clone()).collect();
    store.effect_objects = crate::combat::weapon_fx::effect_obj_nums(&rows);
    store.effect_rows = rows;
    Ok(())
}

/// The whole object-manager part of `Mission::init`, with the mover creation in the middle supplied by the caller (lane E):
/// `countTerrainObjects(first handle)`, `setNumObjects`, `create_movers`, `loadTerrainObjects`, `buildMoverLists`.
pub fn build_for_mission<F>(
    ctx: &mut crate::mission::LoadCtx<'_>,
    num_mechs: usize,
    num_vehicles: usize,
    create_movers: F,
) -> Result<(), String>
where
    F: FnOnce(&mut World) -> Result<(), String>,
{
    let types = ctx
        .world
        .data
        .object_types
        .clone()
        .ok_or_else(|| "object types are not loaded".to_string())?;
    let objects = read_mission_objects(ctx.vfs, &ctx.spec.mission)?;
    let first_handle = (num_mechs + 8 * 16 + num_vehicles + 8 * 16 + 1) as u32;
    {
        let w = &mut *ctx.world;
        w.objects
            .count_terrain_objects(&w.terrain, types, &objects, first_handle)?;
        load_structure_appearances(ctx.vfs, w)?;
        load_weapon_effects(ctx.vfs, &mut w.objects)?;
        let rng = w.rng.clone();
        w.objects.set_num_objects(&rng, num_mechs, num_vehicles);
    }
    create_movers(ctx.world)?;
    load_terrain_objects(ctx.world)?;
    let w = &mut *ctx.world;
    w.objects.build_mover_lists(&w.teams);
    Ok(())
}

/// `GameObjectManager::loadTerrainObjects` (objmgr.cpp:927).
pub fn load_terrain_objects(world: &mut World) -> Result<(), String> {
    let pending = std::mem::take(&mut world.objects.pending);

    // Take the structure objects out of the list: addObject re-seats each one at its block handle, and the slots they came
    // from can be handed to other objects (the C++ moves pointers).
    let mut staged_terrain = stage(&mut world.objects, |s| &s.terrain_objects);
    let mut staged_buildings = stage(&mut world.objects, |s| &s.buildings);
    let mut staged_turrets = stage(&mut world.objects, |s| &s.turrets);
    let mut staged_gates = stage(&mut world.objects, |s| &s.gates);

    // handles[2*i] = firstHandle of the block, handles[2*i+1] = first non-collidable handle
    let mut handles: Vec<(u32, u32)> = world
        .objects
        .blocks
        .iter()
        .map(|b| (b.first_handle, b.first_handle + b.num_collidable))
        .collect();

    let mut cur = Indices::default();
    for data in &pending {
        add_object(
            world,
            data,
            &mut cur,
            &mut handles,
            &mut staged_terrain,
            &mut staged_buildings,
            &mut staged_turrets,
            &mut staged_gates,
        )?;
    }

    link_controls(world);
    special_and_power(world);
    Ok(())
}

/// Indices into the typed arrays (`curTerrainObjectIndex` & co).
#[derive(Default)]
struct Indices {
    terrain: usize,
    building: usize,
    turret: usize,
    gate: usize,
}

type Staged = Vec<Option<Obj>>;

fn stage(store: &mut ObjectStore, which: impl Fn(&ObjectStore) -> &Vec<ObjectHandle>) -> Staged {
    let handles: Vec<ObjectHandle> = which(store).clone();
    handles
        .iter()
        .map(|h| store.list[h.0 as usize].take())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn add_object(
    world: &mut World,
    data: &ObjDataLoader,
    cur: &mut Indices,
    handles: &mut [(u32, u32)],
    staged_terrain: &mut Staged,
    staged_buildings: &mut Staged,
    staged_turrets: &mut Staged,
    staged_gates: &mut Staged,
) -> Result<(), String> {
    let type_n = data.obj_type_num;
    let class = match world.objects.type_by_num(type_n) {
        Some(t) => type_object_class(t),
        None => return Ok(()),
    };
    let block = data.block_number as usize;
    let real_dmg = data.damage & 0x0f;
    let (vertex_number, block_number) = (data.vertex_number, data.block_number);

    let obj_handle: Option<ObjectHandle> = match class {
        ObjectClass::TerrainObject | ObjectClass::Tree => {
            let i = cur.terrain;
            cur.terrain += 1;
            let obj = staged_terrain
                .get_mut(i)
                .and_then(|o| o.take())
                .ok_or_else(|| "No More Trees".to_string())?;
            let new = handles[block].0;
            handles[block].0 += 1;
            let h = place(&mut world.objects, obj, new);
            world.objects.terrain_objects[i] = h;
            structure::init_terrain_object(world, h, type_n);
            if real_dmg != 0 {
                let level = structure::damage_level(world, h);
                structure::set_terrain_damage(world, h, level as i64);
            }
            if let Some(o) = world.objects.get_mut(h) {
                o.set_exists(true);
            }
            Some(h)
        }
        ObjectClass::Turret => {
            let i = cur.turret;
            cur.turret += 1;
            let obj = staged_turrets
                .get_mut(i)
                .and_then(|o| o.take())
                .ok_or_else(|| "No More Turrets".to_string())?;
            let new = handles[block].0;
            handles[block].0 += 1;
            let h = place(&mut world.objects, obj, new);
            world.objects.turrets[i] = h;
            structure::init_turret(world, h, type_n);
            if real_dmg != 0 {
                let level = structure::damage_level(world, h);
                structure::set_turret_damage(world, h, level);
            }
            if let Some(o) = world.objects.get_mut(h) {
                o.set_exists(true);
            }
            structure::set_parent_id(world, h, data.parent_id as u32);
            structure::set_team_id(world, h, data.team_id, true);
            Some(h)
        }
        ObjectClass::Gate => {
            let i = cur.gate;
            cur.gate += 1;
            let obj = staged_gates
                .get_mut(i)
                .and_then(|o| o.take())
                .ok_or_else(|| "No More Gates".to_string())?;
            let new = handles[block].0;
            handles[block].0 += 1;
            let h = place(&mut world.objects, obj, new);
            world.objects.gates[i] = h;
            structure::init_gate(world, h, type_n);
            if real_dmg != 0 {
                let level = structure::damage_level(world, h);
                structure::set_gate_damage(world, h, level);
            }
            if let Some(o) = world.objects.get_mut(h) {
                o.set_exists(true);
            }
            structure::set_parent_id(world, h, data.parent_id as u32);
            structure::set_team_id(world, h, data.team_id, true);
            Some(h)
        }
        ObjectClass::Building | ObjectClass::TreeBuilding => {
            let i = cur.building;
            cur.building += 1;
            let obj = staged_buildings
                .get_mut(i)
                .and_then(|o| o.take())
                .ok_or_else(|| "No More Buildings".to_string())?;
            // The handle depends on whether the building is special (collidable block part) or not.
            let special = structure::building_info(world, type_n).is_some_and(|b| b.is_special());
            let new = if special {
                let h = handles[block].0;
                handles[block].0 += 1;
                h
            } else {
                let h = handles[block].1;
                handles[block].1 += 1;
                h
            };
            let h = place(&mut world.objects, obj, new);
            world.objects.buildings[i] = h;
            structure::init_building(world, h, type_n);
            if real_dmg != 0 {
                let level = structure::damage_level(world, h);
                structure::set_building_damage(world, h, level);
            }
            if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                b.base_tile_id = (data.damage >> 4) as u8;
            }
            if let Some(o) = world.objects.get_mut(h) {
                o.set_exists(true);
            }
            structure::set_parent_id(world, h, data.parent_id as u32);
            structure::set_team_id(world, h, data.team_id, true);
            Some(h)
        }
        ObjectClass::Bridge => None,
        other => {
            return Err(format!(
                "GameObjectManager.addObject: bad object type (class {})",
                other as i32
            ));
        }
    };

    let Some(h) = obj_handle else {
        return Ok(());
    };

    structure::set_terrain_position(world, h, data.position, vertex_number, block_number);
    let (cell_row, cell_col, instance_class) = {
        let o = world.objects.get(h).expect("object just placed");
        (i32::from(o.c.cell.0), i32::from(o.c.cell.1), o.c.class)
    };
    world.objects.set_part_id(h, cell_row, cell_col, 0);

    // Just keep the designers from hurting themselves.
    if class != ObjectClass::Turret {
        structure::set_rotation(world, h, data.rotation);
    }

    // TerrainObject::calcCellFootprint(pos): cellFootprint/vectorFootprint are never read by the simulation (the only reader,
    // getLineOfSightNodes, is commented out of lineOfSight), so the shape-based computation is skipped.

    if instance_class == ObjectClass::Building {
        let sub_type = structure::building_info(world, type_n).map(|b| b.sub_type);
        match sub_type {
            Some(BuildingSubType::Wall) => {
                if let Some(fp) = find_footprint(&world.terrain, cell_row, cell_col) {
                    structure::calc_sub_areas(world, h, &fp);
                }
                structure::close_sub_areas(world, h);
            }
            Some(BuildingSubType::LandBridge) => {
                if let Some(fp) = find_footprint(&world.terrain, cell_row, cell_col) {
                    structure::calc_sub_areas(world, h, &fp);
                }
                structure::open_sub_areas(world, h);
            }
            _ => {}
        }
    } else if instance_class == ObjectClass::Gate {
        if let Some(fp) = find_footprint(&world.terrain, cell_row, cell_col) {
            structure::calc_sub_areas(world, h, &fp);
        }
        structure::open_sub_areas(world, h);
        let (sub0, sub1, destroyed, team) = {
            let o = world.objects.get(h).expect("gate exists");
            let Kind::Gate(g) = &o.kind else {
                return Ok(());
            };
            (
                g.s.sub_areas[0].clone(),
                g.s.sub_areas[1].clone(),
                o.c.status == ObjectStatus::Destroyed,
                g.team_id,
            )
        };
        let wid = world.objects.watch_id(h);
        for (i, &a0) in sub0.iter().enumerate() {
            let a1 = sub1.get(i).copied().unwrap_or(0);
            terr::global_map_mut(&mut world.terrain, 0)
                .set_area_owner_wid(i32::from(a0), wid.0 as i32);
            terr::global_map_mut(&mut world.terrain, 1)
                .set_area_owner_wid(i32::from(a1), wid.0 as i32);
            if destroyed {
                terr::global_map_mut(&mut world.terrain, 0).set_area_team_id(i32::from(a0), -1);
                terr::global_map_mut(&mut world.terrain, 1).set_area_team_id(i32::from(a1), -1);
            } else {
                terr::global_map_mut(&mut world.terrain, 0).set_area_team_id(i32::from(a0), team);
                terr::global_map_mut(&mut world.terrain, 1).set_area_team_id(i32::from(a1), -1);
            }
        }
        let team_now = world.objects.get(h).map(|o| o.team_id()).unwrap_or(-1);
        structure::set_team_id(world, h, team_now, true);
        let cells: Vec<(i16, i16)> = world
            .objects
            .get(h)
            .and_then(|o| o.structure())
            .map(|s| s.cells_covered.clone())
            .unwrap_or_default();
        for (r, c) in cells {
            terr::set_gate(&mut world.terrain, i32::from(r), i32::from(c), true);
        }
    }
    Ok(())
}

fn find_footprint(terrain: &Terrain, row: i32, col: i32) -> Option<Vec<[i16; 2]>> {
    terr::special_areas(terrain)
        .iter()
        .find(|f| i32::from(f.cell_position_row) == row && i32::from(f.cell_position_col) == col)
        .map(|f| f.cells[..(f.num_cells.max(0) as usize).min(f.cells.len())].to_vec())
}

/// Seat `obj` at `handle` (`objList[handle] = obj; obj->setHandle(handle)`).
fn place(store: &mut ObjectStore, mut obj: Obj, handle: u32) -> ObjectHandle {
    let h = ObjectHandle(handle);
    obj.c.handle = h;
    store.list[handle as usize] = Some(obj);
    h
}

/// "Finally, build the control building lists": turrets, then gates, point at the building on the cell their `parentId` names.
fn link_controls(world: &mut World) {
    let turrets = world.objects.turrets.clone();
    for t in turrets {
        let parent_id = match world.objects.get(t).map(|o| &o.kind) {
            Some(Kind::Turret(ts)) => ts.parent_id,
            _ => continue,
        };
        if parent_id != 0xffff_ffff && parent_id != 0 {
            let row = (parent_id >> 16) as i32;
            let col = (parent_id & 0xffff) as i32;
            match world.objects.find_by_cell_position(row, col) {
                Some(cb) => {
                    let already = world
                        .objects
                        .get(cb)
                        .is_some_and(|o| o.c.flag(flags::CONTROLBUILDING));
                    if !already {
                        let idx = world.objects.turret_controls.len() as u8;
                        if let Some(o) = world.objects.get_mut(cb) {
                            o.c.set_flag(flags::CAPTURABLE, true);
                            o.c.set_flag(flags::CONTROLBUILDING, true);
                            if let Kind::Building(b) = &mut o.kind {
                                b.list_id = idx;
                            }
                        }
                        world.objects.turret_controls.push(cb);
                    }
                }
                None => {
                    // PAUSE(...): turret linked to a cell without a building.
                    if let Some(Kind::Turret(ts)) = world.objects.get_mut(t).map(|o| &mut o.kind) {
                        ts.parent_id = 0xffff_ffff;
                    }
                }
            }
        }
    }
    let gates = world.objects.gates.clone();
    for g in gates {
        let parent_id = match world.objects.get(g).map(|o| &o.kind) {
            Some(Kind::Gate(gs)) => gs.parent_id,
            _ => continue,
        };
        if parent_id != 0xffff_ffff && parent_id != 0 {
            let row = (parent_id >> 16) as i32;
            let col = (parent_id & 0xffff) as i32;
            match world.objects.find_by_cell_position(row, col) {
                Some(cb) => {
                    let already = world
                        .objects
                        .get(cb)
                        .is_some_and(|o| o.c.flag(flags::CONTROLBUILDING));
                    if !already {
                        let idx = world.objects.gate_controls.len() as u8;
                        if let Some(o) = world.objects.get_mut(cb) {
                            o.c.set_flag(flags::CONTROLBUILDING, true);
                            if let Kind::Building(b) = &mut o.kind {
                                b.list_id = idx;
                            }
                        }
                        world.objects.gate_controls.push(cb);
                    }
                }
                None => {
                    if let Some(Kind::Gate(gs)) = world.objects.get_mut(g).map(|o| &mut o.kind) {
                        gs.parent_id = 0xffff_ffff;
                    }
                }
            }
        }
    }
}

/// Special buildings (updated every frame), power generators, and every building/terrain object's nearest generator.
fn special_and_power(world: &mut World) {
    let buildings = world.objects.buildings.clone();
    for &b in &buildings {
        if structure::is_special_building(world, b) {
            world.objects.special_buildings.push(b);
        }
    }
    // Now point every lit building to at least one power generator.
    for &b in &buildings {
        if structure::is_power_source(world, b) {
            world.objects.power_generators.push(b);
        }
    }
    if world.objects.power_generators.is_empty() {
        return;
    }
    let gens = world.objects.power_generators.clone();
    let pos_of = |w: &World, h: ObjectHandle| {
        w.objects
            .get(h)
            .map(|o| o.c.position)
            .unwrap_or_else(|| Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            })
    };
    let nearest = |w: &World, from: ObjectHandle| -> ObjectHandle {
        let p = pos_of(w, from);
        let sub = |g: ObjectHandle| {
            let q = pos_of(w, g);
            crate::math::approximate_length(&Vec3 {
                x: p.x - q.x,
                y: p.y - q.y,
                z: p.z - q.z,
            })
        };
        let mut best = 0usize;
        let mut min_distance = sub(gens[0]);
        for (j, &g) in gens.iter().enumerate().skip(1) {
            let d = sub(g);
            if d < min_distance {
                best = j;
                min_distance = d;
            }
        }
        gens[best]
    };
    for &b in &buildings {
        let g = nearest(world, b);
        structure::set_power_supply(world, b, g);
    }
    let terrain_objects = world.objects.terrain_objects.clone();
    for &t in &terrain_objects {
        let g = nearest(world, t);
        structure::set_power_supply(world, t, g);
    }
}

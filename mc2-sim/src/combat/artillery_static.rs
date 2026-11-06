// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Artillery::handleStaticCollision` (artlry.cpp:670): a tangible strike sets off every live mine in the 9x9 cell area around it,
//! then is tested against every existing object of its own terrain block and the eight blocks around it.

use crate::ids::ObjectHandle;
use crate::object::carnage::{block_and_vertex_number, MINE_EXPLOSION_ID};
use crate::object::collision::detect_static_collision;
use crate::object::{hooks, terr, ObjectStore};
use crate::terrain::WORLD_UNITS_PER_METER;
use crate::World;

/// `Artillery::handleStaticCollision`, mine clearance (artlry.cpp:670-722).
pub fn handle_static_collision(world: &mut World, h: ObjectHandle) {
    let Some(o) = world.objects.get(h) else {
        return;
    };
    if !o.exists() || !o.c.tangible() {
        return;
    }
    let position = o.c.position;

    // MUST figure out radius to set off mines in CELLS now (-fs): a 9x9 block centred on the strike's cell.
    let (row, col) = terr::world_to_cell(&world.terrain, &position);
    for i in row - 4..row - 4 + 9 {
        for j in col - 4..col - 4 + 9 {
            if !terr::in_bounds(&world.terrain, i, j) {
                continue;
            }
            if terr::get_mine(&world.terrain, i, j) != 1 {
                continue;
            }
            let mine_pos = terr::cell_to_world(&world.terrain, i, j);
            // setMine before createExplosion, as the C++ does.
            terr::set_mine(&mut world.terrain, i, j, 2);
            let (dmg, range) = hooks::mine_splash(world);
            ObjectStore::create_explosion(
                world,
                MINE_EXPLOSION_ID,
                None,
                mine_pos,
                dmg,
                range * WORLD_UNITS_PER_METER,
            );
        }
    }

    // Static objects: the strike's block and the eight around it (artlry.cpp:724-749). Unlike a carnage, no class is skipped.
    let (block_number, _vertex) = block_and_vertex_number(world, h);
    let blocks_side = terr::blocks_per_side(&world.terrain);
    let top_left = block_number - blocks_side - 1;
    let total_blocks = blocks_side * blocks_side;
    let mut current = top_left;
    for i in 0..3 {
        for _j in 0..3 {
            if current >= 0 && current < total_blocks {
                let (first, count) = match world.objects.blocks.get(current as usize) {
                    Some(b) => (b.first_handle, b.num_objects),
                    None => (0, 0),
                };
                for idx in 0..count {
                    let oh = ObjectHandle(first + idx);
                    if world.objects.get(oh).is_some_and(|obj| obj.exists()) {
                        detect_static_collision(world, h, oh);
                    }
                }
            }
            current += 1;
        }
        current = top_left + blocks_side * (i + 1);
    }
}

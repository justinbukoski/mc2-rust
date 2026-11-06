// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Area/door graph and area-level A*. Port of `GlobalMap` (move.h ~965-1393,
//! move.cpp ~1451-1680 loading, ~2952-2990 helpers, ~3123-3765 start/goal doors,
//! propagation, `calcPath`, and open/close of areas).
//!
//! Packet layout (per `GlobalMap::init(PacketFile*)`), starting at `first`:
//! - version u32 (must be `0x022523`), height, width, sectorDim (unused), sectorHeight,
//!   sectorWidth, numAreas, numDoors, numDoorInfos, numDoorLinks (each i32)
//! - areaMap: `height*width` i16 (-1 = no area)
//! - areas: `numAreas` records of 25 bytes (`GlobalMapArea`, pack(1))
//! - per area with `numDoors > 0`: one packet of `numDoors` DoorInfo (3 bytes each)
//! - doors: `numDoors + 2` GlobalMapDoor records of 57 bytes (last two are start/goal placeholders)
//! - per door, per side: one packet of `numLinks[side] + 2` DoorLink (11 bytes each)

use crate::movemap::{
    GateOracle, PathContext, PathError, COST_BLOCKED, HPRIME_NOT_CALCED, MOVEFLAG_CLOSED,
    MOVEFLAG_OPEN,
};
use crate::packets::{
    bool_at, get_exact, i16_at, i32_at, i8_at, read_i32_packet, u32_at, LoadError, PacketSource,
};
use crate::pqueue::PqNode;
use crate::tables::{team_relation, RELATION_FRIENDLY};

/// Index offset of the start placeholder door, counted from `num_doors`.
pub const DOOR_OFFSET_START: usize = 0;
/// Index offset of the goal placeholder door, counted from `num_doors`.
pub const DOOR_OFFSET_GOAL: usize = 1;
/// Placeholder doors appended after the real doors (`NUM_DOOR_OFFSETS`).
pub const NUM_DOOR_OFFSETS: usize = 2;
/// Extra link slots per door side reserved for start/goal placeholder links.
pub const NUM_EXTRA_DOOR_LINKS: usize = 2;
/// `GLOBALMAP_VERSION_NUMBER`.
pub const GLOBALMAP_VERSION_NUMBER: u32 = 0x0002_2523;
/// `MAX_GLOBAL_PATH`.
pub const MAX_GLOBAL_PATH: usize = 50;
/// Capacity of the shared open list (`openList->init(5000)`).
pub const OPEN_LIST_CAPACITY: usize = 5000;
/// `GLOBAL_CONFIDENCE_BAD`.
pub const GLOBAL_CONFIDENCE_BAD: i32 = 0;
/// `GLOBAL_CONFIDENCE_AT_LEAST`.
pub const GLOBAL_CONFIDENCE_AT_LEAST: i32 = 1;
/// `GLOBAL_CONFIDENCE_GOOD`.
pub const GLOBAL_CONFIDENCE_GOOD: i32 = 2;
/// `GLOBALPATH_EXISTS_UNKNOWN`.
pub const GLOBALPATH_EXISTS_UNKNOWN: u8 = 0;
/// `GLOBALPATH_EXISTS_TRUE`.
pub const GLOBALPATH_EXISTS_TRUE: u8 = 1;
/// `GLOBALPATH_EXISTS_FALSE`.
pub const GLOBALPATH_EXISTS_FALSE: u8 = 2;

const AREA_RECORD_BYTES: usize = 25;
const DOOR_INFO_BYTES: usize = 3;
const DOOR_RECORD_BYTES: usize = 57;
const DOOR_LINK_BYTES: usize = 11;

/// `AreaType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaType {
    Normal,
    Wall,
    Gate,
    LandBridge,
    Forest,
}

impl AreaType {
    pub fn from_raw(v: u32) -> Option<AreaType> {
        Some(match v {
            0 => AreaType::Normal,
            1 => AreaType::Wall,
            2 => AreaType::Gate,
            3 => AreaType::LandBridge,
            4 => AreaType::Forest,
            _ => return None,
        })
    }
}

/// `DoorInfo`: one door on an area's boundary. `door_side` is which of the door's two areas this is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoorInfo {
    pub door_index: i16,
    pub door_side: i8,
}

/// `DoorLink`: a precomputed hop from a door through an area to another door.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoorLink {
    pub door_index: i16,
    pub door_side: i8,
    pub cost: i32,
    pub open_cost: i32,
}

/// `GlobalMapDoor`. Layout fields first, then the per-search scratch fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalMapDoor {
    pub row: i16,
    pub col: i16,
    /// Door length in cells.
    pub length: i8,
    pub open: bool,
    pub team_id: i8,
    pub area: [i16; 2],
    pub area_cost: [i16; 2],
    pub direction: [i8; 2],
    pub num_links: [i16; 2],
    pub cost: i32,
    pub parent: i32,
    pub from_area_index: i32,
    pub flags: u32,
    pub g: i32,
    pub h_prime: i32,
    pub f_prime: i32,
}

/// `GlobalMapArea`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalMapArea {
    pub sector_r: i16,
    pub sector_c: i16,
    pub area_type: AreaType,
    pub num_doors: i16,
    pub owner_wid: i32,
    pub team_id: i8,
    pub off_map: bool,
    pub open: bool,
}

/// `GlobalPathStep` as filled by `GlobalMap::calcPath`: `thru_area` is the area traversed
/// to reach `goal_door`. The final entry's `goal_door` is the pseudo goal door
/// (index `num_doors + 1`). Start position, goal position and `goal_cell` are left to
/// the cell-level planner (the C++ leaves them unset here too).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalPathStep {
    pub thru_area: i32,
    pub goal_door: i32,
    pub cost_to_goal: i32,
}

/// Raw pieces for building a `GlobalMap` in code (tests, tools). Real doors only;
/// the two placeholder doors and their link slots are added by [`GlobalMap::from_parts`].
#[derive(Clone, Debug)]
pub struct GlobalMapParts {
    pub width: i32,
    pub height: i32,
    /// Area id per cell, row-major with stride `width`; -1 for no area.
    pub area_map: Vec<i16>,
    pub areas: Vec<GlobalMapArea>,
    /// Real doors. Their `num_links` are recomputed from `links`.
    pub doors: Vec<GlobalMapDoor>,
    /// `links[door][side]`: hops reachable by traversing `doors[door].area[side]`.
    pub links: Vec<[Vec<DoorLink>; 2]>,
}

/// Area/door graph for one movement class (ground, hover or heli).
#[derive(Clone, Debug)]
pub struct GlobalMap {
    width: i32,
    height: i32,
    area_map: Vec<i16>,
    areas: Vec<GlobalMapArea>,
    /// `areas_doors[area]`: the doors bordering that area.
    area_doors: Vec<Vec<DoorInfo>>,
    /// Real doors followed by the start and goal placeholders.
    doors: Vec<GlobalMapDoor>,
    /// `doors_links[door][side]`, each with room for the placeholder links.
    doors_links: Vec<[Vec<DoorLink>; 2]>,
    /// Packed two-bit directed area-pair cache (`GlobalMap::pathExistsTable`).
    path_exists_table: Vec<u8>,
    off_map_areas: Vec<usize>,
    pub hover: bool,
    pub blank: bool,
    /// `useClosedAreas`: treat enemy/closed doors as passable at a cost (used by goal planning).
    pub use_closed_areas: bool,
    /// `moverTeamID`.
    pub mover_team_id: i8,
    goal_sector: [i32; 2],
    start_cell: [i32; 2],
    goal_cell: [i32; 2],
}

impl GlobalMap {
    /// Build from in-memory parts (same post-processing as the packet loader).
    pub fn from_parts(parts: GlobalMapParts) -> Result<Self, LoadError> {
        let n_real = parts.doors.len();
        if parts.links.len() != n_real {
            return Err(LoadError::Inconsistent {
                what: "links per door",
                expected: n_real,
                actual: parts.links.len(),
            });
        }
        if parts.area_map.len() != (parts.width * parts.height) as usize {
            return Err(LoadError::BadValue {
                what: "area map size",
                value: parts.area_map.len() as i64,
            });
        }
        let mut area_doors: Vec<Vec<DoorInfo>> = vec![Vec::new(); parts.areas.len()];
        for (i, d) in parts.doors.iter().enumerate() {
            for side in 0..2usize {
                let a = d.area[side];
                if a >= 0 && (a as usize) < area_doors.len() {
                    area_doors[a as usize].push(DoorInfo {
                        door_index: i as i16,
                        door_side: side as i8,
                    });
                }
            }
        }
        let mut areas = parts.areas;
        let num_areas = areas.len();
        for (a, a_doors) in areas.iter_mut().zip(area_doors.iter()) {
            a.num_doors = a_doors.len() as i16;
        }
        let mut doors = parts.doors;
        let mut doors_links = parts.links;
        for (d, pair) in doors.iter_mut().zip(doors_links.iter_mut()) {
            d.num_links = [pair[0].len() as i16, pair[1].len() as i16];
            for side in pair.iter_mut() {
                side.resize(side.len() + NUM_EXTRA_DOOR_LINKS, DoorLink::default());
            }
        }
        for _ in 0..NUM_DOOR_OFFSETS {
            doors.push(GlobalMapDoor::default());
            doors_links.push([
                vec![DoorLink::default(); NUM_EXTRA_DOOR_LINKS],
                vec![DoorLink::default(); NUM_EXTRA_DOOR_LINKS],
            ]);
        }
        let mut map = GlobalMap {
            width: parts.width,
            height: parts.height,
            area_map: parts.area_map,
            areas,
            area_doors,
            doors,
            doors_links,
            path_exists_table: vec![0; num_areas * (num_areas / 4 + 1)],
            off_map_areas: Vec::new(),
            hover: false,
            blank: false,
            use_closed_areas: false,
            mover_team_id: 0,
            goal_sector: [0, 0],
            start_cell: [-1, -1],
            goal_cell: [-1, -1],
        };
        map.finish_load()?;
        Ok(map)
    }

    /// Load from packets starting at `first`. Returns the map and the number of packets used.
    pub fn from_packets(src: &dyn PacketSource, first: usize) -> Result<(Self, usize), LoadError> {
        let mut p = first;
        let ver = u32_at(get_exact(src, p, 4, "global version")?, 0);
        p += 1;
        if ver != GLOBALMAP_VERSION_NUMBER {
            return Err(LoadError::BadVersion { found: ver });
        }
        let mut hdr = [0i32; 9];
        let names = [
            "global height",
            "global width",
            "global sectorDim",
            "global sectorHeight",
            "global sectorWidth",
            "global numAreas",
            "global numDoors",
            "global numDoorInfos",
            "global numDoorLinks",
        ];
        for (i, slot) in hdr.iter_mut().enumerate() {
            *slot = read_i32_packet(src, p, names[i])?;
            p += 1;
        }
        let height = hdr[0];
        let width = hdr[1];
        let num_areas = nonneg(hdr[5], "numAreas")?;
        let num_doors = nonneg(hdr[6], "numDoors")?;
        let num_door_infos = nonneg(hdr[7], "numDoorInfos")?;
        let num_door_links = nonneg(hdr[8], "numDoorLinks")?;
        let height_u = nonneg(height, "height")?;
        let width_u = nonneg(width, "width")?;

        let area_map_len = height_u
            .checked_mul(width_u)
            .and_then(|n| n.checked_mul(2))
            .ok_or(LoadError::BadValue {
                what: "global areaMap size",
                value: height as i64 * width as i64,
            })?;
        let area_map_bytes = get_exact(src, p, area_map_len, "global areaMap")?;
        p += 1;
        let area_map: Vec<i16> = area_map_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();

        let area_bytes = get_exact(src, p, num_areas * AREA_RECORD_BYTES, "global areas")?;
        p += 1;
        let mut areas = Vec::with_capacity(num_areas);
        for rec in area_bytes.as_chunks::<AREA_RECORD_BYTES>().0 {
            let raw_type = u32_at(rec, 8);
            let area_type = AreaType::from_raw(raw_type).ok_or(LoadError::BadValue {
                what: "area type",
                value: raw_type as i64,
            })?;
            areas.push(GlobalMapArea {
                sector_r: i16_at(rec, 0),
                sector_c: i16_at(rec, 2),
                area_type,
                num_doors: i16_at(rec, 12),
                owner_wid: i32_at(rec, 14),
                team_id: i8_at(rec, 18),
                off_map: bool_at(rec, 19),
                open: bool_at(rec, 20),
            });
        }

        let mut area_doors: Vec<Vec<DoorInfo>> = Vec::with_capacity(num_areas);
        let mut door_info_total = 0usize;
        for a in &areas {
            let n = nonneg(a.num_doors as i32, "area numDoors")?;
            if n > 0 {
                let b = get_exact(src, p, n * DOOR_INFO_BYTES, "global doorInfos")?;
                p += 1;
                area_doors.push(
                    b.as_chunks::<DOOR_INFO_BYTES>()
                        .0
                        .iter()
                        .map(|r| DoorInfo {
                            door_index: i16_at(r, 0),
                            door_side: i8_at(r, 2),
                        })
                        .collect(),
                );
            } else {
                area_doors.push(Vec::new());
            }
            door_info_total += n;
        }
        if door_info_total != num_door_infos {
            return Err(LoadError::Inconsistent {
                what: "doorInfo count",
                expected: num_door_infos,
                actual: door_info_total,
            });
        }

        let total_doors = num_doors + NUM_DOOR_OFFSETS;
        let door_bytes = get_exact(src, p, total_doors * DOOR_RECORD_BYTES, "global doors")?;
        p += 1;
        let mut doors = Vec::with_capacity(total_doors);
        for r in door_bytes.as_chunks::<DOOR_RECORD_BYTES>().0 {
            doors.push(GlobalMapDoor {
                row: i16_at(r, 0),
                col: i16_at(r, 2),
                length: i8_at(r, 4),
                open: bool_at(r, 5),
                team_id: i8_at(r, 6),
                area: [i16_at(r, 7), i16_at(r, 9)],
                area_cost: [i16_at(r, 11), i16_at(r, 13)],
                direction: [i8_at(r, 15), i8_at(r, 16)],
                num_links: [i16_at(r, 17), i16_at(r, 19)],
                // bytes 21..29: legacy 32-bit pointers, unused
                cost: i32_at(r, 29),
                parent: i32_at(r, 33),
                from_area_index: i32_at(r, 37),
                flags: u32_at(r, 41),
                g: i32_at(r, 45),
                h_prime: i32_at(r, 49),
                f_prime: i32_at(r, 53),
            });
        }

        let mut doors_links = Vec::with_capacity(total_doors);
        let mut link_total = 0usize;
        for d in &doors {
            let mut pair: [Vec<DoorLink>; 2] = [Vec::new(), Vec::new()];
            for (side, slot) in pair.iter_mut().enumerate() {
                let raw = d.num_links[side];
                if raw < 0 {
                    return Err(LoadError::BadValue {
                        what: "door numLinks",
                        value: raw as i64,
                    });
                }
                let n = raw as usize + NUM_EXTRA_DOOR_LINKS;
                let b = get_exact(src, p, n * DOOR_LINK_BYTES, "global doorLinks")?;
                p += 1;
                *slot = b
                    .as_chunks::<DOOR_LINK_BYTES>()
                    .0
                    .iter()
                    .map(|r| DoorLink {
                        door_index: i16_at(r, 0),
                        door_side: i8_at(r, 2),
                        cost: i32_at(r, 3),
                        open_cost: i32_at(r, 7),
                    })
                    .collect();
                link_total += n;
            }
            doors_links.push(pair);
        }
        if link_total != num_door_links {
            return Err(LoadError::Inconsistent {
                what: "doorLink count",
                expected: num_door_links,
                actual: link_total,
            });
        }

        let mut map = GlobalMap {
            width,
            height,
            area_map,
            areas,
            area_doors,
            doors,
            doors_links,
            path_exists_table: vec![0; num_areas * (num_areas / 4 + 1)],
            off_map_areas: Vec::new(),
            hover: false,
            blank: false,
            use_closed_areas: false,
            mover_team_id: 0,
            goal_sector: [0, 0],
            start_cell: [-1, -1],
            goal_cell: [-1, -1],
        };
        map.finish_load()?;
        Ok((map, p - first))
    }

    /// Check every index the search will follow: the area ids in the map, the door
    /// records and their areas, the door-to-area lists, and the links. Malformed packets
    /// must fail here rather than panic during a later search.
    fn validate(&self) -> Result<(), LoadError> {
        let num_areas = self.areas.len() as i64;
        let num_doors = self.num_doors();
        for &v in &self.area_map {
            if v as i64 >= num_areas {
                return Err(LoadError::BadValue {
                    what: "areaMap id",
                    value: v as i64,
                });
            }
        }
        for d in &self.doors[..num_doors] {
            if d.length < 0 {
                return Err(LoadError::BadValue {
                    what: "door length",
                    value: d.length as i64,
                });
            }
            for side in 0..2 {
                let a = d.area[side] as i64;
                if a < 0 || a >= num_areas {
                    return Err(LoadError::BadValue {
                        what: "door area",
                        value: a,
                    });
                }
            }
        }
        for (a, infos) in self.area_doors.iter().enumerate() {
            for info in infos {
                let di = info.door_index as i64;
                if di < 0 || di as usize >= num_doors {
                    return Err(LoadError::BadValue {
                        what: "area door index",
                        value: di,
                    });
                }
                let side = info.door_side as i64;
                if side != 0 && side != 1 {
                    return Err(LoadError::BadValue {
                        what: "area door side",
                        value: side,
                    });
                }
                if self.doors[di as usize].area[side as usize] as usize != a {
                    return Err(LoadError::BadValue {
                        what: "area door does not border its area",
                        value: a as i64,
                    });
                }
            }
        }
        // Only real doors carry links in the file. The placeholders' link data is overwritten
        // by `set_start_door`/`set_goal_door` before any search reads it (see finish_load).
        for (door, pair) in self.doors_links[..num_doors].iter().enumerate() {
            for (side, links) in pair.iter().enumerate() {
                let n = (self.doors[door].num_links[side].max(0) as usize).min(links.len());
                for link in &links[..n] {
                    let li = link.door_index as i64;
                    if li < 0 || li as usize >= num_doors {
                        return Err(LoadError::BadValue {
                            what: "door link index",
                            value: li,
                        });
                    }
                    let ls = link.door_side as i64;
                    if ls != 0 && ls != 1 {
                        return Err(LoadError::BadValue {
                            what: "door link side",
                            value: ls,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Off-map areas start closed; every other area starts open (load-time half of `init`).
    fn finish_load(&mut self) -> Result<(), LoadError> {
        // The placeholder records in a file hold stale link counts; the search sets them up
        // again from scratch, so start them empty.
        let nd = self.num_doors();
        for d in &mut self.doors[nd..] {
            d.num_links = [0, 0];
        }
        self.validate()?;
        self.off_map_areas.clear();
        for i in 0..self.areas.len() {
            if self.areas[i].off_map {
                self.off_map_areas.push(i);
                self.close_area(i as i32);
            } else {
                self.open_area(i as i32);
            }
        }
        Ok(())
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn num_areas(&self) -> usize {
        self.areas.len()
    }

    /// Number of real doors (excluding the two placeholders).
    pub fn num_doors(&self) -> usize {
        self.doors.len() - NUM_DOOR_OFFSETS
    }

    pub fn areas(&self) -> &[GlobalMapArea] {
        &self.areas
    }

    /// Door record by index (real doors, or placeholders at `num_doors()+DOOR_OFFSET_*`).
    pub fn door(&self, index: usize) -> &GlobalMapDoor {
        &self.doors[index]
    }

    pub fn area_doors(&self, area: usize) -> &[DoorInfo] {
        &self.area_doors[area]
    }

    pub fn off_map_areas(&self) -> &[usize] {
        &self.off_map_areas
    }

    pub fn is_door_open(&self, door: usize) -> bool {
        self.doors[door].open
    }

    pub fn is_closed_area(&self, area: usize) -> bool {
        !self.areas[area].open
    }

    /// Area id at (row, col), or -1 (also for out-of-map coordinates).
    pub fn calc_area(&self, row: i32, col: i32) -> i32 {
        if row < 0 || col < 0 || row >= self.height || col >= self.width {
            return -1;
        }
        // calcArea: any negative id means "no area".
        let v = self.area_map[(row * self.width + col) as usize] as i32;
        if v < 0 {
            -1
        } else {
            v
        }
    }

    /// `clearPathExistsTable`: reset the packed two-bit directed area-pair cache.
    pub fn clear_path_exists_table(&mut self) {
        self.path_exists_table.fill(GLOBALPATH_EXISTS_UNKNOWN);
    }

    /// `setPathExists(fromArea, toArea, value)`. Invalid area ids are ignored as in C++.
    pub fn set_path_exists(&mut self, from_area: i32, to_area: i32, value: u8) {
        if from_area < 0 || to_area < 0 || to_area >= self.areas.len() as i32 {
            return;
        }
        let row_width = self.areas.len() / 4 + 1;
        let index = from_area as usize * row_width + to_area as usize / 4;
        let shift = (to_area as usize % 4) * 2;
        let mask = 0x03_u8 << shift;
        if let Some(byte) = self.path_exists_table.get_mut(index) {
            *byte = (*byte & !mask) | ((value << shift) & mask);
        }
    }

    /// `getPathExists(fromArea, toArea)`: returns the masked byte exactly as C++ does.
    /// For area indices not divisible by four the two-bit value remains shifted, a legacy quirk
    /// relied on by callers that compare against the unshifted constants.
    pub fn get_path_exists(&self, from_area: i32, to_area: i32) -> u8 {
        if from_area < 0 || to_area < 0 || to_area >= self.areas.len() as i32 {
            return GLOBALPATH_EXISTS_UNKNOWN;
        }
        let row_width = self.areas.len() / 4 + 1;
        let index = from_area as usize * row_width + to_area as usize / 4;
        let shift = (to_area as usize % 4) * 2;
        self.path_exists_table
            .get(index)
            .map_or(GLOBALPATH_EXISTS_UNKNOWN, |byte| byte & (0x03 << shift))
    }

    /// `GlobalMap::getPathCost` (`move.cpp:2666`). The compiled source always calculates a fresh route;
    /// `calc_it` is therefore intentionally unused, matching its active `#if 1` implementation.
    #[allow(clippy::too_many_arguments)]
    pub fn get_path_cost(
        &mut self,
        ctx: &mut PathContext,
        start_area: i32,
        goal_area: i32,
        with_special_areas: bool,
        confidence: &mut i32,
        _calc_it: bool,
        oracle: &dyn GateOracle,
    ) -> i32 {
        if start_area < 0 || goal_area < 0 {
            return 0;
        }
        let (Some(start), Some(goal)) = (
            self.areas.get(start_area as usize),
            self.areas.get(goal_area as usize),
        ) else {
            return 0;
        };
        if !start.open || !goal.open {
            return 0;
        }
        if start_area == goal_area {
            return 1;
        }
        self.use_closed_areas = with_special_areas;
        let result = self.calc_path(ctx, start_area, goal_area, None, None, oracle);
        self.use_closed_areas = false;
        *confidence = GLOBAL_CONFIDENCE_GOOD;
        match result {
            Ok(path) if !path.is_empty() => {
                let goal_door = self.num_doors() + DOOR_OFFSET_GOAL;
                self.doors.get(goal_door).map_or(0, |door| door.g)
            }
            _ => 0,
        }
    }

    /// Set a door's area-owned team (C++ `setAreaTeamID`): applies to the area and its doors.
    pub fn set_area_team_id(&mut self, area: i32, team_id: i8) {
        if area < 0 {
            return;
        }
        let a = area as usize;
        self.areas[a].team_id = team_id;
        for k in 0..self.area_doors[a].len() {
            let di = self.area_doors[a][k].door_index as usize;
            self.doors[di].team_id = team_id;
        }
    }

    pub fn set_area_owner_wid(&mut self, area: i32, owner_wid: i32) {
        if area < 0 {
            return;
        }
        self.areas[area as usize].owner_wid = owner_wid;
    }

    pub fn open_door(&mut self, door: usize) {
        self.doors[door].cost = 10;
        self.doors[door].open = true;
    }

    pub fn close_door(&mut self, door: usize) {
        self.doors[door].open = false;
    }

    /// Close an area: its doors close and the links into it cost 1000.
    pub fn close_area(&mut self, area: i32) {
        if area < 0 {
            return;
        }
        let a = area as usize;
        self.areas[a].open = false;
        for k in 0..self.area_doors[a].len() {
            let di = self.area_doors[a][k].door_index as usize;
            self.close_door(di);
        }
        for k in 0..self.area_doors[a].len() {
            let info = self.area_doors[a][k];
            let di = info.door_index as usize;
            let side = info.door_side as usize;
            let n = self.doors[di].num_links[side] as usize;
            for j in 0..n {
                self.doors_links[di][side][j].cost = 1000;
            }
        }
    }

    /// Open an area: doors whose two areas are both open open, and links revert to their open cost.
    pub fn open_area(&mut self, area: i32) {
        if area < 0 {
            return;
        }
        let a = area as usize;
        self.areas[a].open = true;
        for k in 0..self.area_doors[a].len() {
            let di = self.area_doors[a][k].door_index as usize;
            let area0 = self.doors[di].area[0] as i32;
            let area1 = self.doors[di].area[1] as i32;
            if !self.is_closed_area(area0 as usize) && !self.is_closed_area(area1 as usize) {
                self.open_door(di);
            }
        }
        for k in 0..self.area_doors[a].len() {
            let info = self.area_doors[a][k];
            let di = info.door_index as usize;
            let side = info.door_side as usize;
            let n = self.doors[di].num_links[side] as usize;
            for j in 0..n {
                let oc = self.doors_links[di][side][j].open_cost;
                self.doors_links[di][side][j].cost = oc;
            }
        }
    }

    fn set_link(&mut self, door: usize, side: usize, idx: usize, link: DoorLink) {
        let v = &mut self.doors_links[door][side];
        if v.len() <= idx {
            v.resize(idx + 1, DoorLink::default());
        }
        v[idx] = link;
    }

    fn start_index(&self) -> usize {
        self.num_doors() + DOOR_OFFSET_START
    }

    fn goal_index(&self) -> usize {
        self.num_doors() + DOOR_OFFSET_GOAL
    }

    /// `setStartDoor`: wire the start placeholder to the doors of `start_area`.
    fn set_start_door(&mut self, start_area: usize) {
        let sd = self.start_index();
        let n0 = self.areas[start_area].num_doors;
        {
            let d = &mut self.doors[sd];
            d.row = 0;
            d.col = 0;
            d.length = 0;
            d.open = true;
            d.team_id = -1;
            d.area = [start_area as i16, start_area as i16];
            d.area_cost = [1, 1];
            d.direction = [-1, -1];
            d.num_links = [n0, 0];
            d.from_area_index = 1;
        }
        let start_cell = self.start_cell;
        for cur in 0..n0.max(0) as usize {
            let info = self.area_doors[start_area][cur];
            let di = info.door_index as usize;
            let side = info.door_side as usize;
            let (drow, dcol, dnl) = {
                let cd = &self.doors[di];
                (cd.row as i32, cd.col as i32, cd.num_links[side] as usize)
            };
            let mut cost_sum = 1i32;
            if start_cell[0] > -1 {
                cost_sum += (start_cell[0] - drow).abs();
                cost_sum += (start_cell[1] - dcol).abs();
            }
            self.set_link(
                sd,
                0,
                cur,
                DoorLink {
                    door_index: di as i16,
                    door_side: side as i8,
                    cost: cost_sum,
                    open_cost: cost_sum,
                },
            );
            self.set_link(
                di,
                side,
                dnl,
                DoorLink {
                    door_index: sd as i16,
                    door_side: 0,
                    cost: cost_sum,
                    open_cost: cost_sum,
                },
            );
            self.doors[di].num_links[side] += 1;
        }
    }

    fn reset_start_door(&mut self, start_area: usize) {
        let sd = self.start_index();
        let n = self.doors[sd].num_links[0].max(0) as usize;
        for cur in 0..n {
            let info = self.area_doors[start_area][cur];
            let di = info.door_index as usize;
            self.doors[di].num_links[info.door_side as usize] -= 1;
        }
    }

    /// `setGoalDoor`: wire the goal placeholder to the doors of `goal_area`.
    fn set_goal_door(&mut self, goal_area: usize) {
        let gd = self.goal_index();
        self.goal_sector = [
            self.areas[goal_area].sector_r as i32,
            self.areas[goal_area].sector_c as i32,
        ];
        let n0 = self.areas[goal_area].num_doors;
        {
            let d = &mut self.doors[gd];
            d.row = 0;
            d.col = 0;
            d.length = 0;
            d.open = true;
            d.team_id = -1;
            d.area = [goal_area as i16, goal_area as i16];
            d.area_cost = [1, 1];
            d.direction = [-1, -1];
            d.num_links = [n0, 0];
        }
        let goal_cell = self.goal_cell;
        for cur in 0..n0.max(0) as usize {
            let info = self.area_doors[goal_area][cur];
            let di = info.door_index as usize;
            let side = info.door_side as usize;
            let (drow, dcol, dnl) = {
                let cd = &self.doors[di];
                (cd.row as i32, cd.col as i32, cd.num_links[side] as usize)
            };
            let mut cost_sum = 1i32;
            if goal_cell[0] > -1 {
                cost_sum += (goal_cell[0] - drow).abs();
                cost_sum += (goal_cell[1] - dcol).abs();
            }
            self.set_link(
                gd,
                0,
                cur,
                DoorLink {
                    door_index: di as i16,
                    door_side: side as i8,
                    cost: cost_sum,
                    open_cost: cost_sum,
                },
            );
            self.set_link(
                di,
                side,
                dnl,
                DoorLink {
                    door_index: gd as i16,
                    door_side: 0,
                    cost: cost_sum,
                    open_cost: cost_sum,
                },
            );
            self.doors[di].num_links[side] += 1;
        }
    }

    fn reset_goal_door(&mut self, goal_area: usize) {
        let gd = self.goal_index();
        let n = self.doors[gd].num_links[0].max(0) as usize;
        for cur in 0..n {
            let info = self.area_doors[goal_area][cur];
            let di = info.door_index as usize;
            self.doors[di].num_links[info.door_side as usize] -= 1;
        }
    }

    /// `calcHPrime(door)`: sector-distance heuristic to the goal sector.
    fn calc_h_prime(&self, door: usize) -> i32 {
        let a0 = self.doors[door].area[0] as usize;
        let a1 = self.doors[door].area[1] as usize;
        let sr = (self.areas[a0].sector_r as i32 + self.areas[a1].sector_r as i32) / 2;
        let sc = (self.areas[a0].sector_c as i32 + self.areas[a1].sector_c as i32) / 2;
        (sr - self.goal_sector[0]).abs() + (sc - self.goal_sector[1]).abs()
    }

    /// `propogateCost` (recursive). Note the C++ quirks kept on purpose: the parent
    /// branch passes `to_area_index` as the from-area, and an open door is re-keyed
    /// through its door index (not its heap index).
    fn propogate_cost(
        &mut self,
        ctx: &mut PathContext,
        door: usize,
        cost: i32,
        from_area_index: i32,
        g: i32,
    ) {
        if self.doors[door].g > g + cost {
            self.doors[door].g = g + cost;
            self.doors[door].f_prime = self.doors[door].g + self.doors[door].h_prime;
            if self.doors[door].flags & MOVEFLAG_OPEN != 0 {
                let _open_index = ctx.open_list.find(door as u32);
                // C++ passes the door id where a heap index is expected; kept as-is.
                ctx.open_list.change(door, self.doors[door].f_prime);
            } else {
                let to_area_index = 1 - from_area_index;
                let to = to_area_index as usize;
                let num_links = self.doors[door].num_links[to].max(0) as usize;
                for cur_link in 0..num_links {
                    let link = self.doors_links[door][to][cur_link];
                    let succ = link.door_index as usize;
                    let mut succ_door_cost = link.cost;
                    let succ_team = self.doors[succ].team_id;
                    if self.use_closed_areas {
                        if succ_team > -1 && succ_team != self.mover_team_id {
                            succ_door_cost = 1000;
                        }
                    } else {
                        if succ_team > -1 && succ_team != self.mover_team_id {
                            succ_door_cost = COST_BLOCKED;
                        }
                        if !self.doors[succ].open {
                            succ_door_cost = COST_BLOCKED;
                        }
                    }
                    let succ_from_area_index =
                        (self.doors[succ].area[1] == self.doors[door].area[to]) as i32;
                    if (self.doors[succ].open || self.use_closed_areas)
                        && succ_door_cost < COST_BLOCKED
                        && self.doors[succ].h_prime != HPRIME_NOT_CALCED
                    {
                        if door as i32 == self.doors[succ].parent {
                            let g_cur = self.doors[door].g;
                            self.propogate_cost(ctx, succ, succ_door_cost, to_area_index, g_cur);
                        } else if self.doors[door].g + succ_door_cost < self.doors[succ].g {
                            self.doors[succ].cost = succ_door_cost;
                            self.doors[succ].parent = door as i32;
                            self.doors[succ].from_area_index = succ_from_area_index;
                            let g_cur = self.doors[door].g;
                            self.propogate_cost(
                                ctx,
                                succ,
                                succ_door_cost,
                                succ_from_area_index,
                                g_cur,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Area-level A* (`GlobalMap::calcPath(startArea, goalArea, path, ...)`).
    ///
    /// Gate areas are opened or closed from `oracle` and the mover team first, as the C++
    /// does on every call. Returns the door path (empty when there is no route), or
    /// `PathError::NoArea` when either endpoint has no area (C++ returns -1).
    pub fn calc_path(
        &mut self,
        ctx: &mut PathContext,
        start_area: i32,
        goal_area: i32,
        start_cell: Option<(i32, i32)>,
        goal_cell: Option<(i32, i32)>,
        oracle: &dyn GateOracle,
    ) -> Result<Vec<GlobalPathStep>, PathError> {
        if start_area < 0 || goal_area < 0 {
            return Err(PathError::NoArea);
        }
        let sa = start_area as usize;
        let ga = goal_area as usize;
        if sa >= self.areas.len() || ga >= self.areas.len() {
            return Err(PathError::NoArea);
        }
        self.start_cell = match start_cell {
            Some((r, c)) => [r, c],
            None => [-1, -1],
        };
        self.goal_cell = match goal_cell {
            Some((r, c)) => [r, c],
            None => [-1, -1],
        };

        let nd = self.num_doors();
        let start_door = nd + DOOR_OFFSET_START;
        let goal_door = nd + DOOR_OFFSET_GOAL;
        for d in 0..nd + NUM_DOOR_OFFSETS {
            let door = &mut self.doors[d];
            door.cost = 1;
            door.parent = -1;
            door.from_area_index = -1;
            door.flags = 0;
            door.g = 0;
            door.h_prime = HPRIME_NOT_CALCED;
            door.f_prime = 0;
        }

        self.set_start_door(sa);
        self.set_goal_door(ga);
        if self.areas[sa].off_map {
            self.open_area(start_area);
        }
        if self.areas[ga].off_map {
            self.open_area(goal_area);
        }

        for i in 0..self.areas.len() {
            let a = self.areas[i];
            if a.area_type != AreaType::Gate {
                continue;
            }
            if a.team_id == self.mover_team_id || a.team_id == -1 {
                if a.owner_wid > 0 {
                    if oracle.is_gate_disabled(a.owner_wid) {
                        self.close_area(i as i32);
                    } else {
                        self.open_area(i as i32);
                    }
                } else {
                    self.open_area(i as i32);
                }
            } else if oracle.is_gate_open(a.owner_wid) {
                self.open_area(i as i32);
            } else {
                self.close_area(i as i32);
            }
        }

        ctx.open_list.clear();
        let initial = PqNode {
            key: 0,
            id: start_door as u32,
            row: 0,
            col: 0,
        };
        ctx.open_list
            .insert(initial)
            .map_err(|_| PathError::OpenListFull)?;
        self.doors[start_door].flags |= MOVEFLAG_OPEN;

        let mut goal_found = false;
        while !ctx.open_list.is_empty() {
            let best = ctx.open_list.remove();
            let best_door = best.id as usize;
            self.doors[best_door].flags &= !MOVEFLAG_OPEN;
            let best_door_g = self.doors[best_door].g;
            let from_area_index = self.doors[best_door].from_area_index;
            self.doors[best_door].flags |= MOVEFLAG_CLOSED;

            if best_door == goal_door {
                goal_found = true;
                break;
            }
            let to = (1 - from_area_index).clamp(0, 1) as usize;
            let thru_area = self.doors[best_door].area[to] as i32;
            let num_links = self.doors[best_door].num_links[to].max(0) as usize;
            for cur_link in 0..num_links {
                let link = self.doors_links[best_door][to][cur_link];
                let succ = link.door_index as usize;
                let mut succ_door_cost = link.cost;
                let succ_team = self.doors[succ].team_id;
                if self.use_closed_areas {
                    if succ_team > -1
                        && team_relation(succ_team, self.mover_team_id) == RELATION_FRIENDLY
                    {
                        succ_door_cost = 50;
                    }
                    if !self.doors[succ].open {
                        succ_door_cost = 1000;
                    }
                } else {
                    if succ_team > -1 && succ_team != self.mover_team_id {
                        succ_door_cost = COST_BLOCKED;
                    }
                    if !self.doors[succ].open {
                        succ_door_cost = COST_BLOCKED;
                    }
                }
                if succ_door_cost < COST_BLOCKED {
                    if self.doors[succ].h_prime == HPRIME_NOT_CALCED {
                        self.doors[succ].h_prime = self.calc_h_prime(succ);
                    }
                    let succ_door_g = best_door_g + succ_door_cost;
                    let succ_from_area_index =
                        (self.doors[succ].area[1] as i32 == thru_area) as i32;
                    if self.doors[succ].flags & MOVEFLAG_OPEN != 0 {
                        if succ_door_g < self.doors[succ].g {
                            self.doors[succ].cost = succ_door_cost;
                            self.doors[succ].parent = best_door as i32;
                            self.doors[succ].from_area_index = succ_from_area_index;
                            self.doors[succ].g = succ_door_g;
                            self.doors[succ].f_prime = succ_door_g + self.doors[succ].h_prime;
                            let open_index = ctx.open_list.find(succ as u32);
                            let f = self.doors[succ].f_prime;
                            ctx.open_list.change(open_index, f);
                        }
                    } else if self.doors[succ].flags & MOVEFLAG_CLOSED != 0 {
                        if succ_door_g < self.doors[succ].g {
                            self.doors[succ].cost = succ_door_cost;
                            self.doors[succ].parent = best_door as i32;
                            self.doors[succ].from_area_index = succ_from_area_index;
                            self.propogate_cost(
                                ctx,
                                succ,
                                succ_door_cost,
                                succ_from_area_index,
                                best_door_g,
                            );
                        }
                    } else {
                        self.doors[succ].cost = succ_door_cost;
                        self.doors[succ].parent = best_door as i32;
                        self.doors[succ].from_area_index = succ_from_area_index;
                        self.doors[succ].g = succ_door_g;
                        self.doors[succ].f_prime = succ_door_g + self.doors[succ].h_prime;
                        let node = PqNode {
                            key: self.doors[succ].f_prime,
                            id: succ as u32,
                            row: 0,
                            col: 0,
                        };
                        ctx.open_list
                            .insert(node)
                            .map_err(|_| PathError::OpenListFull)?;
                        self.doors[succ].flags |= MOVEFLAG_OPEN;
                    }
                }
            }
        }

        self.reset_start_door(sa);
        self.reset_goal_door(ga);
        if self.areas[sa].off_map {
            self.close_area(start_area);
        }
        if self.areas[ga].off_map {
            self.close_area(goal_area);
        }

        if !goal_found {
            return Ok(Vec::new());
        }

        let mut num_steps = 1usize;
        let mut cur = goal_door;
        while cur != start_door {
            num_steps += 1;
            cur = self.doors[cur].parent as usize;
        }
        let num_path_doors = num_steps - 1;
        if num_path_doors >= MAX_GLOBAL_PATH {
            return Err(PathError::PathTooLong {
                steps: num_path_doors,
                max: MAX_GLOBAL_PATH,
            });
        }
        let mut path = vec![GlobalPathStep::default(); num_path_doors];
        let mut cur = goal_door;
        let mut cur_path_door = num_path_doors;
        let mut cost_to_goal = 0;
        while cur_path_door > 0 {
            let from = self.doors[cur].from_area_index.clamp(0, 1) as usize;
            path[cur_path_door - 1] = GlobalPathStep {
                thru_area: self.doors[cur].area[from] as i32,
                goal_door: cur as i32,
                cost_to_goal,
            };
            cost_to_goal += self.doors[cur].cost;
            cur = self.doors[cur].parent as usize;
            cur_path_door -= 1;
        }
        Ok(path)
    }

    /// Cell-based convenience: areas are looked up from the cells, as `GlobalMap::calcPath(start, goal, path)`.
    pub fn calc_path_cells(
        &mut self,
        ctx: &mut PathContext,
        start: (i32, i32),
        goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<Vec<GlobalPathStep>, PathError> {
        let sa = self.calc_area(start.0, start.1);
        let ga = self.calc_area(goal.0, goal.1);
        self.calc_path(ctx, sa, ga, Some(start), Some(goal), oracle)
    }
}

fn nonneg(v: i32, what: &'static str) -> Result<usize, LoadError> {
    if v < 0 {
        return Err(LoadError::BadValue {
            what,
            value: v as i64,
        });
    }
    Ok(v as usize)
}

/// The part of a [`GlobalMap`] that changes during a mission: gates open and close doors, areas change owner and team, and the
/// path search toggles closed-area routing. Everything else is rebuilt from the mission packets. Used by the mc2-sim save format
/// (`mc2-sim` `terrain`); the fields are set exactly, without the side effects of the setters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalMapDynamic {
    /// `use_closed_areas`.
    pub use_closed_areas: bool,
    /// `mover_team_id`.
    pub mover_team_id: i8,
    /// Per area, in area order: `(team_id, owner_wid, open)`.
    pub areas: Vec<(i8, i32, bool)>,
    /// Per door, in door order: `(team_id, open)`.
    pub doors: Vec<(i8, bool)>,
}

impl GlobalMap {
    /// The mutable state (see [`GlobalMapDynamic`]).
    pub fn dynamic_state(&self) -> GlobalMapDynamic {
        GlobalMapDynamic {
            use_closed_areas: self.use_closed_areas,
            mover_team_id: self.mover_team_id,
            areas: self
                .areas
                .iter()
                .map(|a| (a.team_id, a.owner_wid, a.open))
                .collect(),
            doors: self.doors.iter().map(|d| (d.team_id, d.open)).collect(),
        }
    }

    /// Restores [`GlobalMap::dynamic_state`] onto a map built from the same packets. Fails when the area or door counts differ.
    pub fn set_dynamic_state(&mut self, s: &GlobalMapDynamic) -> Result<(), String> {
        if s.areas.len() != self.areas.len() || s.doors.len() != self.doors.len() {
            return Err(format!(
                "global map state: {} areas and {} doors saved, the map has {} and {}",
                s.areas.len(),
                s.doors.len(),
                self.areas.len(),
                self.doors.len()
            ));
        }
        self.use_closed_areas = s.use_closed_areas;
        self.mover_team_id = s.mover_team_id;
        for (a, &(team_id, owner_wid, open)) in self.areas.iter_mut().zip(&s.areas) {
            a.team_id = team_id;
            a.owner_wid = owner_wid;
            a.open = open;
        }
        for (d, &(team_id, open)) in self.doors.iter_mut().zip(&s.doors) {
            d.team_id = team_id;
            d.open = open;
        }
        Ok(())
    }
}

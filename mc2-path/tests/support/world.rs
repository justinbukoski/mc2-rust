//! Synthetic 60x60 world (four 30x30 sectors) and packet encoders for loader tests.
//! Areas: 0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right.
//! Doors: V0 (0|1), V1 (2|3) on the vertical boundary; H0 (0|2), H1 (1|3) on the horizontal one.

#![allow(dead_code)]

use mc2_path::{
    AreaType, DoorLink, FootPrint, GlobalMapArea, GlobalMapDoor, GlobalMapParts, MapCell,
    MissionMap, MoveData,
};

pub const W: i32 = 60;
pub const H: i32 = 60;

pub fn open_mission() -> MissionMap {
    let mut cells = vec![MapCell(0); (W * H) as usize];
    for c in &mut cells {
        c.set_passable(true);
    }
    MissionMap::from_cells(W, H, cells).unwrap()
}

pub fn area_grid() -> Vec<i16> {
    let mut v = Vec::with_capacity((W * H) as usize);
    for r in 0..H {
        for c in 0..W {
            v.push(((r / 30) * 2 + c / 30) as i16);
        }
    }
    v
}

pub fn door(a: i16, b: i16, row: i16, col: i16, len: i8, dir: i8) -> GlobalMapDoor {
    GlobalMapDoor {
        row,
        col,
        length: len,
        open: true,
        team_id: -1,
        area: [a, b],
        area_cost: [1, 1],
        direction: [dir, dir],
        ..Default::default()
    }
}

pub fn areas() -> Vec<GlobalMapArea> {
    (0..4)
        .map(|i| GlobalMapArea {
            sector_r: (i / 2) as i16,
            sector_c: (i % 2) as i16,
            area_type: AreaType::Normal,
            num_doors: 0,
            owner_wid: 0,
            team_id: -1,
            off_map: false,
            open: true,
        })
        .collect()
}

pub fn four_area_doors() -> Vec<GlobalMapDoor> {
    vec![
        door(0, 1, 0, 29, 30, 1),
        door(2, 3, 30, 29, 30, 1),
        door(0, 2, 29, 0, 30, 2),
        door(1, 3, 29, 30, 30, 2),
    ]
}

/// Links as the game builds them: from each door, through one of its areas, to every other door on that area.
pub fn parts_for(doors: Vec<GlobalMapDoor>) -> GlobalMapParts {
    let mut links: Vec<[Vec<DoorLink>; 2]> = Vec::with_capacity(doors.len());
    for (i, d) in doors.iter().enumerate() {
        let mut pair: [Vec<DoorLink>; 2] = [Vec::new(), Vec::new()];
        for (side, slot) in pair.iter_mut().enumerate() {
            let area = d.area[side];
            for (j, e) in doors.iter().enumerate() {
                if j == i {
                    continue;
                }
                if let Some(s) = (0..2).find(|&s| e.area[s] == area) {
                    slot.push(DoorLink {
                        door_index: j as i16,
                        door_side: s as i8,
                        cost: 1,
                        open_cost: 1,
                    });
                }
            }
        }
        links.push(pair);
    }
    GlobalMapParts {
        width: W,
        height: H,
        area_map: area_grid(),
        areas: areas(),
        doors,
        links,
    }
}

/// Encode one global map as its packets (layout of `GlobalMap::from_packets`).
pub fn encode_global(parts: &GlobalMapParts) -> Vec<Vec<u8>> {
    let n_real = parts.doors.len();
    let total = n_real + 2;
    let n_areas = parts.areas.len();
    let mut area_doors: Vec<Vec<(i16, i8)>> = vec![Vec::new(); n_areas];
    for (i, d) in parts.doors.iter().enumerate() {
        for s in 0..2 {
            area_doors[d.area[s] as usize].push((i as i16, s as i8));
        }
    }
    let num_door_infos: usize = area_doors.iter().map(|v| v.len()).sum();
    let num_links: usize = parts
        .links
        .iter()
        .map(|p| p[0].len() + 2 + p[1].len() + 2)
        .sum::<usize>()
        + 8;

    let mut pk: Vec<Vec<u8>> = Vec::new();
    pk.push(0x0002_2523u32.to_le_bytes().to_vec());
    for v in [
        parts.height,
        parts.width,
        30,
        parts.height / 30,
        parts.width / 30,
        n_areas as i32,
        n_real as i32,
        num_door_infos as i32,
        num_links as i32,
    ] {
        pk.push(v.to_le_bytes().to_vec());
    }
    pk.push(
        parts
            .area_map
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    );
    let mut area_bytes = Vec::new();
    for (a, rec) in parts.areas.iter().enumerate() {
        area_bytes.extend_from_slice(&rec.sector_r.to_le_bytes());
        area_bytes.extend_from_slice(&rec.sector_c.to_le_bytes());
        area_bytes.extend_from_slice(&[0u8; 4]);
        area_bytes.extend_from_slice(&0u32.to_le_bytes()); // AreaType::Normal
        area_bytes.extend_from_slice(&(area_doors[a].len() as i16).to_le_bytes());
        area_bytes.extend_from_slice(&rec.owner_wid.to_le_bytes());
        area_bytes.push(rec.team_id as u8);
        area_bytes.push(rec.off_map as u8);
        area_bytes.push(rec.open as u8);
        area_bytes.extend_from_slice(&[0u8; 4]);
    }
    pk.push(area_bytes);
    for infos in &area_doors {
        if !infos.is_empty() {
            let mut b = Vec::new();
            for (di, side) in infos {
                b.extend_from_slice(&di.to_le_bytes());
                b.push(*side as u8);
            }
            pk.push(b);
        }
    }
    let mut door_bytes = Vec::new();
    let mut lens = Vec::new();
    for d in &parts.doors {
        let mut r = vec![0u8; 57];
        r[0..2].copy_from_slice(&d.row.to_le_bytes());
        r[2..4].copy_from_slice(&d.col.to_le_bytes());
        r[4] = d.length as u8;
        r[5] = d.open as u8;
        r[6] = d.team_id as u8;
        r[7..9].copy_from_slice(&d.area[0].to_le_bytes());
        r[9..11].copy_from_slice(&d.area[1].to_le_bytes());
        r[11..13].copy_from_slice(&d.area_cost[0].to_le_bytes());
        r[13..15].copy_from_slice(&d.area_cost[1].to_le_bytes());
        r[15] = d.direction[0] as u8;
        r[16] = d.direction[1] as u8;
        door_bytes.push(r);
    }
    for (i, r) in door_bytes.iter_mut().enumerate() {
        let n0 = parts.links[i][0].len() as i16;
        let n1 = parts.links[i][1].len() as i16;
        r[17..19].copy_from_slice(&n0.to_le_bytes());
        r[19..21].copy_from_slice(&n1.to_le_bytes());
        lens.push((n0, n1));
    }
    for _ in 0..2 {
        door_bytes.push(vec![0u8; 57]);
        lens.push((0, 0));
    }
    pk.push(door_bytes.concat());
    for i in 0..total {
        for side in 0..2 {
            let mut b = Vec::new();
            if i < n_real {
                for l in &parts.links[i][side] {
                    b.extend_from_slice(&l.door_index.to_le_bytes());
                    b.push(l.door_side as u8);
                    b.extend_from_slice(&l.cost.to_le_bytes());
                    b.extend_from_slice(&l.open_cost.to_le_bytes());
                }
            }
            // Two padding slots, as the C++ writes (NUM_EXTRA_DOOR_LINKS).
            b.extend_from_slice(&[0u8; 22]);
            pk.push(b);
        }
    }
    pk
}

/// Mission map packets (height, width, planet, cells).
pub fn encode_mission(m: &MissionMap) -> Vec<Vec<u8>> {
    let mut cells = Vec::new();
    for r in 0..m.height() {
        for c in 0..m.width() {
            cells.extend_from_slice(&m.cell(r, c).0.to_le_bytes());
        }
    }
    vec![
        m.height().to_le_bytes().to_vec(),
        m.width().to_le_bytes().to_vec(),
        m.planet().to_le_bytes().to_vec(),
        cells,
    ]
}

/// One footprint record (264 bytes).
pub fn footprint_bytes(row: i16, col: i16, cells: &[(i16, i16)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(264);
    b.extend_from_slice(&row.to_le_bytes());
    b.extend_from_slice(&col.to_le_bytes());
    b.extend_from_slice(&0i16.to_le_bytes());
    b.extend_from_slice(&(cells.len() as i16).to_le_bytes());
    for i in 0..64 {
        let (r, c) = cells.get(i).copied().unwrap_or((0, 0));
        b.extend_from_slice(&r.to_le_bytes());
        b.extend_from_slice(&c.to_le_bytes());
    }
    b
}

/// A full move-data packet list: mission, three global maps, then the special-area count and footprint packets.
pub fn move_packets(
    mission: &MissionMap,
    global: &GlobalMapParts,
    count: Vec<u8>,
    footprints: Vec<u8>,
) -> Vec<Vec<u8>> {
    let mut pk = encode_mission(mission);
    for _ in 0..3 {
        pk.extend(encode_global(global));
    }
    pk.push(count);
    pk.push(footprints);
    pk
}

/// Build the in-memory `MoveData` for a world (three identical levels).
pub fn move_data(mission: MissionMap, global: GlobalMapParts) -> MoveData {
    let g = mc2_path::GlobalMap::from_parts(global).unwrap();
    MoveData {
        mission,
        global: [g.clone(), g.clone(), g],
        special_areas: Vec::<FootPrint>::new(),
        packets_used: 0,
    }
}

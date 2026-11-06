// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Lookup tables from `mclib/move.cpp`.

/// Corner-check ladder for diagonal steps (`StepAdjDir`, 9 entries).
pub const STEP_ADJ_DIR: [i32; 9] = [-1, 0, 2, 2, 4, 4, 6, 6, 0];

/// Row/col offsets of the cell beyond a door, indexed by door direction 0..3 (`adjTile`).
pub const ADJ_TILE: [[i32; 2]; 4] = [[-1, 0], [0, 1], [1, 0], [0, -1]];

/// Team relation matrix (`TeamRelations`): 0 friendly, 1 neutral, 2 enemy.
pub const TEAM_RELATIONS: [[u8; 8]; 8] = [
    [0, 2, 1, 2, 2, 2, 2, 2],
    [2, 0, 2, 2, 2, 2, 2, 2],
    [1, 2, 0, 2, 2, 2, 2, 2],
    [2, 2, 2, 0, 2, 2, 2, 2],
    [2, 2, 2, 2, 0, 2, 2, 2],
    [2, 2, 2, 2, 2, 0, 2, 2],
    [2, 2, 2, 2, 2, 2, 0, 2],
    [2, 2, 2, 2, 2, 2, 2, 0],
];

/// `RELATION_FRIENDLY` (move.cpp / Team relation enum).
pub const RELATION_FRIENDLY: u8 = 0;
/// `RELATION_ENEMY`.
pub const RELATION_ENEMY: u8 = 2;

/// `IsDiagonalStep`: directions 1,3,5,7 are diagonal; every other index is false.
pub fn is_diagonal_step(dir: usize) -> bool {
    dir < 8 && dir % 2 == 1
}

/// `reverseShift`: direction from a cell back to its parent. Equal to the C++
/// 128-entry table `(dir & !7) | ((dir + 4) & 7)` (checked against the source).
pub fn reverse_shift(dir: usize) -> usize {
    (dir & !7) | ((dir + 4) & 7)
}

/// Offsets (row, col) for direction pairs, from move.cpp `cellShift` (NUM_CELL_OFFSETS = 128).
pub const CELL_SHIFT: [i32; 256] = [
    -1, 0, -1, 1, 0, 1, 1, 1, 1, 0, 1, -1, 0, -1, -1, -1, -2, 0, -2, 2, 0, 2, 2, 2, 2, 0, 2, -2, 0,
    -2, -2, -2, -3, 0, -3, 3, 0, 3, 3, 3, 3, 0, 3, -3, 0, -3, -3, -3, -3, 2, -2, 3, 2, 3, 3, 2, 3,
    -2, 2, -3, -2, -3, -3, -2, -4, 0, -4, 4, 0, 4, 4, 4, 4, 0, 4, -4, 0, -4, -4, -4, -5, 0, -5, 5,
    0, 5, 5, 5, 5, 0, 5, -5, 0, -5, -5, -5, -6, 0, -6, 6, 0, 6, 6, 6, 6, 0, 6, -6, 0, -6, -6, -6,
    -7, 0, -7, 7, 0, 7, 7, 7, 7, 0, 7, -7, 0, -7, -7, -7, -8, 0, -8, 8, 0, 8, 8, 8, 8, 0, 8, -8, 0,
    -8, -8, -8, -9, 0, -9, 9, 0, 9, 9, 9, 9, 0, 9, -9, 0, -9, -9, -9, -10, 0, -10, 10, 0, 10, 10,
    10, 10, 0, 10, -10, 0, -10, -10, -10, -11, 0, -11, 11, 0, 11, 11, 11, 11, 0, 11, -11, 0, -11,
    -11, -11, -12, 0, -12, 12, 0, 12, 12, 12, 12, 0, 12, -12, 0, -12, -12, -12, -13, 0, -13, 13, 0,
    13, 13, 13, 13, 0, 13, -13, 0, -13, -13, -13, -14, 0, -14, 14, 0, 14, 14, 14, 14, 0, 14, -14,
    0, -14, -14, -14, -15, 0, -15, 15, 0, 15, 15, 15, 15, 0, 15, -15, 0, -15, -15, -15,
];

/// `TeamRelations[a][b]`; out-of-table teams are treated as enemies.
pub fn team_relation(a: i8, b: i8) -> u8 {
    if (0..8).contains(&a) && (0..8).contains(&b) {
        TEAM_RELATIONS[a as usize][b as usize]
    } else {
        RELATION_ENEMY
    }
}

// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Binary min-heap open list. Port of `mclib/pqueue.cpp` / `pqueue.h`.
//!
//! Behaviour is kept identical to the C++ on purpose, including the parts that look
//! odd: an equal-key insert sifts *above* existing equal keys (so the newest of a tie
//! pops first), index 0 is a sentinel rewritten by `up_heap`, `change` takes a heap
//! *index* (callers that pass an id produce the C++ behaviour), and storage persists
//! across `clear()` so stale slots above `len()` keep their old contents, as in C++.
//!
//! Deviations (only where the C++ has undefined behaviour or an unbounded loop):
//! - `change` with an index past the allocated storage is ignored instead of writing
//!   out of bounds.
//! - `down_heap` stops if it would loop on index 0 (the C++ would spin forever).
//! - `find_by_key` returns 0 when `start_index` is past the live items, rather than
//!   reading stale slots.

/// One open-list entry. Same field order as `PQNode` in pqueue.h.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PqNode {
    /// Sort value (smallest pops first).
    pub key: i32,
    /// Opaque id, e.g. a map-cell index or a door index.
    pub id: u32,
    pub row: i32,
    pub col: i32,
}

/// Returned by [`PriorityQueue::insert`] when the queue is full (C++ returns 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PqFull;

/// Id stored in the sentinel slot by `up_heap` (C++ writes 0xFFFFFFFF).
pub const PQ_SENTINEL_ID: u32 = 0xFFFF_FFFF;

#[derive(Clone, Debug)]
pub struct PriorityQueue {
    list: Vec<PqNode>,
    max_items: i32,
    num_items: i32,
    key_min: i32,
}

impl PriorityQueue {
    /// C++ `init(max, keyMinValue)`: storage for `max + 2` nodes (slot 0 is the sentinel).
    pub fn new(max_items: usize, key_min: i32) -> Self {
        let cap = max_items as i32 + 2;
        let mut list = vec![PqNode::default(); cap as usize + 1];
        list[0] = PqNode {
            key: key_min,
            id: PQ_SENTINEL_ID,
            row: 0,
            col: 0,
        };
        PriorityQueue {
            list,
            max_items: cap,
            num_items: 0,
            key_min,
        }
    }

    /// `new(max, -2000000)`, the C++ default key floor.
    pub fn with_default_key_min(max_items: usize) -> Self {
        Self::new(max_items, -2_000_000)
    }

    pub fn len(&self) -> usize {
        self.num_items as usize
    }

    pub fn is_empty(&self) -> bool {
        self.num_items == 0
    }

    /// C++ `clear()`: forgets live items; slot contents are kept.
    pub fn clear(&mut self) {
        self.num_items = 0;
    }

    /// Node at `index` (1-based for live items, as in C++ `getItem`).
    pub fn get(&self, index: usize) -> Option<&PqNode> {
        self.list.get(index)
    }

    /// Insert; fails only when `len() == max_items`, as the C++ does.
    pub fn insert(&mut self, item: PqNode) -> Result<(), PqFull> {
        if self.num_items == self.max_items {
            return Err(PqFull);
        }
        self.num_items += 1;
        let n = self.num_items as usize;
        self.list[n] = item;
        self.up_heap(n);
        Ok(())
    }

    fn up_heap(&mut self, mut cur: usize) {
        let start = self.list[cur];
        let stop_key = start.key;
        self.list[0].key = self.key_min;
        self.list[0].id = PQ_SENTINEL_ID;
        // `cur > 0` guards the C++ spin when the sentinel key is not below stop_key.
        while cur > 0 && self.list[cur / 2].key >= stop_key {
            self.list[cur] = self.list[cur / 2];
            cur /= 2;
        }
        self.list[cur] = start;
    }

    fn down_heap(&mut self, mut cur: usize) {
        let start = self.list[cur];
        let stop_key = start.key;
        let num = self.num_items as usize;
        while cur <= num / 2 {
            let mut next = cur << 1;
            if next == 0 {
                break;
            }
            if next < num && self.list[next].key > self.list[next + 1].key {
                next += 1;
            }
            if stop_key <= self.list[next].key {
                break;
            }
            self.list[cur] = self.list[next];
            cur = next;
        }
        self.list[cur] = start;
    }

    /// C++ `remove(item)`: take the root and re-heap. Caller must check `is_empty()`.
    pub fn remove(&mut self) -> PqNode {
        let item = self.list[1];
        let n = self.num_items as usize;
        self.list[1] = self.list[n];
        self.num_items -= 1;
        self.down_heap(1);
        item
    }

    /// C++ `change(itemIndex, newValue)`: re-key the node at heap *index* `index`.
    pub fn change(&mut self, index: usize, new_value: i32) {
        if index >= self.list.len() {
            return;
        }
        if new_value > self.list[index].key {
            self.list[index].key = new_value;
            self.down_heap(index);
        } else if new_value < self.list[index].key {
            self.list[index].key = new_value;
            self.up_heap(index);
        }
    }

    /// Linear scan over live slots (and the sentinel, as C++ does). Returns the heap
    /// index, or 0 when not found.
    pub fn find(&self, id: u32) -> usize {
        for index in 0..=self.num_items as usize {
            if self.list[index].id == id {
                return index;
            }
        }
        0
    }

    /// Recursive search of the subtree under `start_index` for a node with this key and id
    /// (only subtrees whose keys are <= `key` are visited). Returns 0 when not found.
    pub fn find_by_key(&self, key: i32, id: u32, start_index: usize) -> usize {
        let num = self.num_items as usize;
        if start_index == 0 || start_index > num {
            return 0;
        }
        if self.list[start_index].key == key && self.list[start_index].id == id {
            return start_index;
        }
        let left = start_index << 1;
        if left <= num && self.list[left].key <= key {
            let idx = self.find_by_key(key, id, left);
            if idx != 0 {
                return idx;
            }
        }
        let right = left + 1;
        if right <= num && self.list[right].key <= key {
            let idx = self.find_by_key(key, id, right);
            if idx != 0 {
                return idx;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(key: i32, id: u32) -> PqNode {
        PqNode {
            key,
            id,
            row: 0,
            col: 0,
        }
    }

    #[test]
    fn pops_in_key_order() {
        let mut q = PriorityQueue::with_default_key_min(16);
        for (i, k) in [7, 3, 9, 1, 5, 3, 8].iter().enumerate() {
            q.insert(node(*k, i as u32)).unwrap();
        }
        let mut keys = Vec::new();
        while !q.is_empty() {
            keys.push(q.remove().key);
        }
        assert_eq!(keys, vec![1, 3, 3, 5, 7, 8, 9]);
    }

    #[test]
    fn equal_keys_newest_pops_first() {
        // up_heap moves an equal-key parent down, so a later equal key ends up at the root.
        let mut q = PriorityQueue::with_default_key_min(8);
        q.insert(node(5, 1)).unwrap();
        q.insert(node(5, 2)).unwrap();
        q.insert(node(5, 3)).unwrap();
        assert_eq!(q.remove().id, 3);
        assert_eq!(q.remove().id, 2);
        assert_eq!(q.remove().id, 1);
    }

    #[test]
    fn change_and_find() {
        let mut q = PriorityQueue::with_default_key_min(8);
        for (i, k) in [10, 20, 30, 40].iter().enumerate() {
            q.insert(node(*k, 100 + i as u32)).unwrap();
        }
        let idx = q.find(102);
        assert_ne!(idx, 0);
        q.change(idx, 1);
        assert_eq!(q.remove().id, 102);
        assert_eq!(q.find(999), 0);
        assert_eq!(q.find_by_key(40, 103, 1), q.find(103));
    }

    #[test]
    fn full_queue_rejects_insert() {
        let mut q = PriorityQueue::with_default_key_min(2);
        assert!(q.insert(node(1, 1)).is_ok());
        assert!(q.insert(node(2, 2)).is_ok());
        assert!(q.insert(node(3, 3)).is_ok());
        assert!(q.insert(node(4, 4)).is_ok());
        assert_eq!(q.insert(node(5, 5)), Err(PqFull));
    }
}

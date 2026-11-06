//! Variant edit legality on the shipped chassis and components: named cases, then random edit
//! sequences checked against an independent model of the rules.

mod common;

use mc2_campaign::variant::{Ctx, Variant};
use mc2_campaign::LogisticsError as E;
use mc2_sim_data::component::ComponentForm as F;

fn empty(data: &mc2_campaign::GameData, stem: &str) -> (usize, Variant) {
    let ci = data.chassis_by_stem(stem).unwrap();
    let c = &data.chassis[ci];
    let mut v = Variant::new_empty(c, 0x0009_0000 | c.id, false);
    v.name = "test".into();
    (ci, v)
}

fn comp(data: &mc2_campaign::GameData, id: i32) -> usize {
    data.catalog
        .find(id)
        .unwrap_or_else(|| panic!("component {id}"))
}

/// Component ids in the shipped catalogue.
const LASER: i32 = 156; // 1x1, heat 3
const PPC: i32 = 145; // 1x3, heat 10
const LARGE_LASER: i32 = 140; // 1x2, heat 8
const HEAT_SINK: i32 = 7; // 1x1, removes 4
const JUMP_JETS: i32 = 10;
const ARMOR: i32 = 25; // 1x1, +32

fn add(
    data: &mc2_campaign::GameData,
    v: &mut Variant,
    ci: usize,
    id: i32,
) -> Result<(i64, i64), E> {
    let ctx = Ctx {
        catalog: &data.catalog,
        chassis: &data.chassis[ci],
    };
    let (mut x, mut y) = (-1, -1);
    v.add_component(&ctx, comp(data, id), &mut x, &mut y)?;
    Ok((x, y))
}

#[test]
fn werewolf_takes_two_armor_plates_and_no_more() {
    let data = require_data!();
    let (ci, mut v) = empty(&data, "werewolf");
    let ctx = Ctx {
        catalog: &data.catalog,
        chassis: &data.chassis[ci],
    };
    assert_eq!((v.armor(&ctx), data.chassis[ci].max_armor), (80, 144));
    add(&data, &mut v, ci, ARMOR).unwrap();
    add(&data, &mut v, ci, ARMOR).unwrap();
    assert_eq!(v.armor(&ctx), 144);
    assert_eq!(add(&data, &mut v, ci, ARMOR), Err(E::NoMoreArmor));
    assert_eq!(v.components.len(), 2, "a refused add changes nothing");
}

#[test]
fn jump_jets_follow_the_chassis_flag_and_come_once() {
    let data = require_data!();
    let (ci, mut v) = empty(&data, "atlas");
    assert_eq!(
        add(&data, &mut v, ci, JUMP_JETS),
        Err(E::JumpJetsNotAllowed)
    );
    let (ci, mut v) = empty(&data, "hunchback");
    assert_eq!(add(&data, &mut v, ci, JUMP_JETS), Ok((-2, -2)));
    assert_eq!(
        add(&data, &mut v, ci, JUMP_JETS),
        Err(E::OnlyOneJumpJetAllowed)
    );
    let ctx = Ctx {
        catalog: &data.catalog,
        chassis: &data.chassis[ci],
    };
    assert_eq!(v.jump_range(&ctx), 5);
    v.remove_component(&ctx, -2, -2).unwrap();
    assert_eq!(v.jump_range(&ctx), 0);
}

#[test]
fn heat_stops_weapons_at_the_chassis_limit_and_heat_sinks_raise_it() {
    let data = require_data!();
    // Hunchback: 24 heat, a 3x4 grid. Two PPCs (10 each) and a laser (3) make 23.
    let (ci, mut v) = empty(&data, "hunchback");
    let ctx = Ctx {
        catalog: &data.catalog,
        chassis: &data.chassis[ci],
    };
    assert_eq!(v.max_heat(&ctx), 24);
    assert_eq!(add(&data, &mut v, ci, PPC), Ok((0, 0)));
    assert_eq!(
        add(&data, &mut v, ci, PPC),
        Ok((1, 0)),
        "the 1x3 PPC goes in the next column"
    );
    assert_eq!(add(&data, &mut v, ci, LASER), Ok((2, 0)));
    assert_eq!(v.heat(&ctx), 23);
    assert_eq!(add(&data, &mut v, ci, LASER), Err(E::ComponentTooHot));
    // a heat sink (dissipates 4) raises the cap to 28 and lets the laser in
    assert_eq!(add(&data, &mut v, ci, HEAT_SINK), Ok((2, 1)));
    assert_eq!(v.max_heat(&ctx), 28);
    assert_eq!(add(&data, &mut v, ci, LASER), Ok((2, 2)));
    assert_eq!(v.heat(&ctx), 26);
    // the sink cannot come out any more: 28 - 4 = 24 < 26
    assert_eq!(v.remove_component(&ctx, 2, 1), Err(E::InsufficientHeat));
    // after the laser goes (heat 23), it can: 24 >= 23
    v.remove_component(&ctx, 2, 2).unwrap();
    v.remove_component(&ctx, 2, 1).unwrap();
    assert_eq!((v.heat(&ctx), v.max_heat(&ctx)), (23, 24));
}

#[test]
fn large_components_need_room_on_the_grid() {
    let data = require_data!();
    // Raven's grid is 3 wide and 2 high: a 1x3 PPC can never fit, a 1x2 large laser can.
    let (ci, mut v) = empty(&data, "raven");
    assert_eq!(add(&data, &mut v, ci, PPC), Err(E::AddComponentFailed));
    assert_eq!(add(&data, &mut v, ci, LARGE_LASER), Ok((0, 0)));
    let ctx = Ctx {
        catalog: &data.catalog,
        chassis: &data.chassis[ci],
    };
    // dropping a laser on the large laser's second cell is refused, next to it is fine
    let (mut x, mut y) = (0, 1);
    assert_eq!(
        v.add_component(&ctx, comp(&data, LASER), &mut x, &mut y),
        Err(E::ComponentSlotFull)
    );
    let (mut x, mut y) = (1, 1);
    v.add_component(&ctx, comp(&data, LASER), &mut x, &mut y)
        .unwrap();
    // and nothing may hang over the right edge
    let (mut x, mut y) = (3, 0);
    assert_eq!(
        v.add_component(&ctx, comp(&data, LASER), &mut x, &mut y),
        Err(E::ComponentSlotFull)
    );
}

// ------------------------------------------------------------------------------------------
// random edit sequences against an independent model

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Placed {
    id: i32,
    x: i64,
    y: i64,
}

struct Model<'a> {
    data: &'a mc2_campaign::GameData,
    ci: usize,
    placed: Vec<Placed>,
}

impl Model<'_> {
    fn c(&self, id: i32) -> &mc2_campaign::component::Component {
        self.data.catalog.by_id(id).unwrap()
    }
    fn rect_cells(&self, p: &Placed) -> Vec<(i64, i64)> {
        let c = self.c(p.id);
        let mut v = Vec::new();
        for dy in 0..i64::from(c.height) {
            for dx in 0..i64::from(c.width) {
                v.push((p.x + dx, p.y + dy));
            }
        }
        v
    }
    fn occupied(&self, x: i64, y: i64) -> Option<usize> {
        if (x, y) == (-1, -1) {
            return None;
        }
        self.placed
            .iter()
            .position(|p| self.rect_cells(p).contains(&(x, y)))
    }
    fn heat(&self) -> i32 {
        self.placed
            .iter()
            .fold(0i32, |h, p| (h as f32 + self.c(p.id).heat) as i32)
    }
    fn max_heat(&self) -> i32 {
        let sinks = self
            .placed
            .iter()
            .filter(|p| self.c(p.id).form == F::HeatSink)
            .fold(0i32, |h, p| (h as f32 + self.c(p.id).damage) as i32);
        sinks + self.data.chassis[self.ci].max_heat as i32
    }
    fn armor(&self) -> i64 {
        let plates = self
            .placed
            .iter()
            .filter(|p| self.c(p.id).form == F::Bulk)
            .count() as i64;
        self.data.chassis[self.ci].base_armor + 32 * plates
    }
    fn has(&self, f: F) -> bool {
        self.placed.iter().any(|p| self.c(p.id).form == f)
    }
    fn fits(&self, x: i64, y: i64, w: i64, h: i64) -> bool {
        let ch = &self.data.chassis[self.ci];
        (0..h).all(|l| {
            (0..w).all(|k| {
                self.occupied(x + k, y + l).is_none()
                    && x + k < ch.component_area_width
                    && y + l < ch.component_area_height
            })
        })
    }
    /// First free spot scanning rows then columns.
    fn first_fit(&self, w: i64, h: i64) -> Option<(i64, i64)> {
        let ch = &self.data.chassis[self.ci];
        for j in 0..ch.component_area_height {
            for i in 0..ch.component_area_width {
                if self.fits(i, j, w, h) {
                    return Some((i, j));
                }
            }
        }
        None
    }

    /// The rules, in the order the game applies them.
    fn add(&mut self, id: i32, x: i64, y: i64) -> Result<(i64, i64), E> {
        let ch = self.data.chassis[self.ci].clone();
        let c = self.c(id).clone();
        let (w, h) = (i64::from(c.width), i64::from(c.height));
        if self.heat() as f32 + c.heat > self.max_heat() as f32 {
            return Err(E::ComponentTooHot);
        }
        let (mut px, mut py) = (x, y);
        match c.form {
            F::JumpJet => {
                if !ch.can_have_jump_jets {
                    return Err(E::JumpJetsNotAllowed);
                }
                if self.has(F::JumpJet) {
                    return Err(E::OnlyOneJumpJetAllowed);
                }
                if (x != -1 && x != -2) || (y != -1 && y != -2) {
                    return Err(E::ComponentSlotFull);
                }
                if self.placed.len() >= 54 {
                    return Err(E::AddComponentFailed);
                }
                self.placed.push(Placed { id, x: -2, y: -2 });
                return Ok((-2, -2));
            }
            _ if (x, y) == (-2, -2) => return Err(E::ComponentSlotFull),
            _ => {}
        }
        if x != -1 && y != -1 {
            if !self.fits(x, y, w, h) {
                return Err(E::ComponentSlotFull);
            }
        } else if x == -1 {
            match self.first_fit(w, h) {
                Some((fx, fy)) => {
                    px = fx;
                    py = fy;
                }
                None => return Err(E::AddComponentFailed),
            }
        } else {
            // x given, y = -1: the search does not run and the pair counts as unplaced
            return Err(E::AddComponentFailed);
        }
        if c.form == F::Bulk && self.armor() + 32 > ch.max_armor {
            return Err(E::NoMoreArmor);
        }
        if c.form == F::Sensor {
            if !ch.can_have_adv_sensor {
                return Err(E::SensorsNotAllowed);
            }
            if self.has(F::Sensor) {
                return Err(E::OnlyOneSensorAllowed);
            }
        }
        if c.form == F::Ecm {
            if !ch.can_have_ecm {
                return Err(E::EcmNotAllowed);
            }
            if self.has(F::Ecm) {
                return Err(E::OnlyOneEcmAllowed);
            }
        }
        // placement proper (a variant holds at most 54 components)
        if self.placed.len() >= 54 {
            return Err(E::AddComponentFailed);
        }
        if matches!(c.form, F::Ecm | F::Sensor) {
            px = -3;
            py = -3;
        }
        self.placed.push(Placed { id, x: px, y: py });
        Ok((px, py))
    }

    fn remove(&mut self, x: i64, y: i64) -> Result<(), E> {
        let Some(i) = self.occupied(x, y) else {
            return Err(E::Failed);
        };
        let id = self.placed[i].id;
        if self.c(id).form == F::HeatSink
            && (self.max_heat() as f32 - self.c(id).damage) < self.heat() as f32
        {
            return Err(E::InsufficientHeat);
        }
        self.placed.remove(i);
        Ok(())
    }
}

#[test]
fn random_edits_agree_with_the_model_on_every_chassis() {
    let data = require_data!();
    let ids: Vec<i32> = data.catalog.iter().map(|c| c.id).collect();
    let mut rng = Lcg(0x5eed_1234_5678_9abc);
    let mut accepted = 0;
    let mut refused = std::collections::BTreeMap::<i32, u32>::new();

    for chassis in &data.chassis {
        let ci = chassis.index;
        let ctx = Ctx {
            catalog: &data.catalog,
            chassis,
        };
        let mut v = Variant::new_empty(chassis, 0x0001_0000 | chassis.id, false);
        let mut model = Model {
            data: &data,
            ci,
            placed: Vec::new(),
        };
        let (w, h) = (chassis.component_area_width, chassis.component_area_height);

        for step in 0..500 {
            let kind = rng.below(10);
            let before = v.clone();
            if kind < 7 || (kind < 9 && v.components.is_empty()) {
                // add; prefer the cheap, small components half of the time so grids fill up
                let id = if rng.below(2) == 0 {
                    *[LASER, HEAT_SINK, ARMOR, JUMP_JETS][rng.below(4) as usize..]
                        .first()
                        .unwrap()
                } else {
                    ids[rng.below(ids.len() as u64) as usize]
                };
                let (x, y) = match rng.below(4) {
                    0 | 1 => (-1, -1),
                    2 => (
                        rng.below((w + 3) as u64) as i64 - 2,
                        rng.below((h + 3) as u64) as i64 - 2,
                    ),
                    _ => (-1, rng.below(h.max(1) as u64) as i64),
                };
                let (mut ax, mut ay) = (x, y);
                let got = v.add_component(&ctx, data.catalog.find(id).unwrap(), &mut ax, &mut ay);
                let want = model.add(id, x, y);
                match (&got, &want) {
                    (Ok(()), Ok((mx, my))) => {
                        accepted += 1;
                        let last = v.components.last().unwrap();
                        assert_eq!(
                            (last.x, last.y),
                            (*mx, *my),
                            "{} step {step}: placed",
                            chassis.file_name
                        );
                    }
                    (Err(a), Err(b)) => {
                        assert_eq!(
                            a, b,
                            "{} step {step}: error for component {id} at ({x},{y})",
                            chassis.file_name
                        );
                        *refused.entry(a.code()).or_default() += 1;
                        assert_eq!(v, before, "a refused add changes nothing");
                    }
                    _ => panic!(
                        "{} step {step}: component {id} at ({x},{y}): game {got:?}, model {want:?}",
                        chassis.file_name
                    ),
                }
            } else {
                let x = rng.below((w + 4) as u64) as i64 - 3;
                let y = rng.below((h + 4) as u64) as i64 - 3;
                let got = v.remove_component(&ctx, x, y);
                let want = model.remove(x, y);
                assert_eq!(
                    got, want,
                    "{} step {step}: remove ({x},{y})",
                    chassis.file_name
                );
                if got.is_err() {
                    assert_eq!(v, before);
                }
            }

            // the variant and the model hold the same components in the same order
            let mine: Vec<_> = v
                .components
                .iter()
                .map(|p| Placed {
                    id: data.catalog.get(p.component).unwrap().id,
                    x: p.x,
                    y: p.y,
                })
                .collect();
            assert_eq!(mine, model.placed, "{} step {step}", chassis.file_name);

            // invariants
            assert!(
                v.heat(&ctx) <= v.max_heat(&ctx),
                "{}: overheated",
                chassis.file_name
            );
            if chassis.base_armor <= chassis.max_armor {
                assert!(
                    i64::from(v.armor(&ctx)) <= chassis.max_armor,
                    "{}: armor",
                    chassis.file_name
                );
            }
            for f in [F::JumpJet, F::Ecm, F::Sensor] {
                let n = v
                    .components
                    .iter()
                    .filter(|p| data.catalog.get(p.component).unwrap().form == f)
                    .count();
                assert!(n <= 1, "{}: {f:?} x{n}", chassis.file_name);
            }
            let mut cells = std::collections::HashSet::new();
            for p in v.components.iter().filter(|p| p.x >= 0 && p.y >= 0) {
                let c = data.catalog.get(p.component).unwrap();
                for dy in 0..i64::from(c.height) {
                    for dx in 0..i64::from(c.width) {
                        assert!(
                            cells.insert((p.x + dx, p.y + dy)),
                            "{}: overlap",
                            chassis.file_name
                        );
                    }
                }
            }
        }
    }
    assert!(accepted > 1500, "{accepted} accepted edits");
    // the sequences hit every refusal the shipped components can produce
    for code in [
        E::ComponentTooHot,
        E::JumpJetsNotAllowed,
        E::OnlyOneJumpJetAllowed,
        E::ComponentSlotFull,
        E::AddComponentFailed,
        E::NoMoreArmor,
    ] {
        assert!(
            refused.contains_key(&code.code()),
            "never saw {code}: {refused:?}"
        );
    }
}

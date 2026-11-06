//! MechCommander 2 simulation: a faithful, renderer-free port of the game logic in `code/*.cpp`.
//!
//! Read `DESIGN.md` first. In short: a [`World`] (no globals) holds all state; [`Sim::step`] advances it one frame in the exact
//! order of `Mission::update`; time, randomness and camera visibility are explicit inputs ([`FrameInput`], [`rng`], [`visibility`]) so
//! runs are reproducible and can be compared against the C++ oracle (`oracle/`, [`digest`]).
//!
//! Lanes (each can be ported independently against the interfaces in this scaffold):
//! * A - [`object`]: objects, handles, terrain objects, buildings, gates, turrets (non-fire), carnage, collisions, trigger areas
//! * B - [`mover`]: mech/vehicle movement and physics, path following, crash avoidance, `MovePathManager`
//! * C - [`combat`]: weapons, bolts, damage, criticals, ammo, artillery, turret fire control
//! * D - [`ai`]: pilots, tactical orders, forces, sensors/contacts, line of sight
//! * E - [`mission`]: mission init/loop/termination, objectives, ABL bindings, logistics hooks, save/load
//! * F - [`appearance`]: gesture/animation state machines and node geometry the simulation reads
//! * T - [`terrain`]: ground queries and map data

pub mod ai;
pub mod appearance;
pub mod combat;
pub mod command;
pub mod crater;
pub mod digest;
pub mod events;
pub mod ids;
pub mod math;
pub mod mission;
pub mod mover;
pub mod object;
pub mod rng;
pub mod save;
pub mod sim;
pub mod terrain;
#[cfg(feature = "test-hooks")]
pub mod test_hooks;
pub mod time;
pub mod visibility;
pub mod world;

pub use command::{PlayerCommand, TacticalOrder};
pub use events::{fx_id, FxAction, FxBase, FxEffect, FxPlacement, FxSlot, SimEvent, WeaponImpact};
pub use ids::{ObjectHandle, PartId, WatchId};
pub use sim::{FrameInput, FrameOutput, FrameUnavailable, Sim, SimError};
pub use world::{GameData, World};

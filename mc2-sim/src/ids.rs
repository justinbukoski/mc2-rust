//! Identifier newtypes. The C++ passes all of these around as `long`; mixing them up is a classic source of bugs.

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident($inner:ty)) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub $inner);
    };
}

id_type!(
    /// Index into `GameObjectManager::objList` (`GameObjectHandle`). 1-based; 0 means "no object".
    ObjectHandle(u32)
);
id_type!(
    /// `GameObjectWatchID`: a stable reference to an object that survives the object being recycled
    /// (assigned on first use from `GameObjectManager::nextWatchID`, 1-based; 0 means "no object").
    WatchId(u32)
);
id_type!(
    /// `GameObject::partId`: the mission-file part number for placed objects, or a synthesized id for terrain
    /// objects / reinforcements (`GameObjectManager::calcPartId`). 0 means "none".
    PartId(i32)
);
id_type!(
    /// Team index (`Team::teams`, 0..MAX_TEAMS). -1 means none.
    TeamId(i8)
);
id_type!(
    /// Commander index (`Commander::commanders`, 0..MAX_COMMANDERS). -1 means none.
    CommanderId(i8)
);

impl ObjectHandle {
    /// "No object".
    pub const NONE: ObjectHandle = ObjectHandle(0);
    /// True for the null handle.
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl WatchId {
    /// "No object".
    pub const NONE: WatchId = WatchId(0);
    /// True for the null id.
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// Number of teams (`MAX_TEAMS`).
pub const MAX_TEAMS: usize = 8;
/// Number of commanders (`MAX_COMMANDERS` / `MAX_MC_PLAYERS`).
pub const MAX_COMMANDERS: usize = 8;

// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The top-level game states and their transitions.
//!
//! The original runs one loop (`code/mechcmd2.cpp`: `UpdateRenderers`/`DoGameLogic`) and switches
//! between the logistics screens (`Logistics::update`, `logisticsState`: splash, main menu, mission
//! selection, mech bay ... `log_STARTMISSIONFROMCMDLINE`) and the mission (`mission->init`,
//! `eye->activate`, `mission->start`, `Mission::update` until `terminationResult` ends it, then the
//! results screens).  Here the same flow is an explicit state machine.  The menu, logistics and
//! results screens are drawn and driven by the UI lanes through [`crate::layers::UiLayer`];
//! the production host is [`crate::ui_host::UiHost`]; [`crate::layers::NullUi`] is the minimal stand-in
//! for tests that install no host.

/// Where the game is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameState {
    /// Data mounted, nothing shown yet.
    Boot,
    /// Main menu (UI lane U1).
    MainMenu,
    /// Logistics: mission selection, mech bay, pilots, purchase (UI lane U2, campaign C1).
    Logistics,
    /// Loading a mission (the original's loading screen with progress).
    MissionLoading(String),
    /// Playing a mission.
    InMission,
    /// Mission over: `scenarioResult` (results screens, UI lane U2/U3).
    Results(u32),
    /// Leaving the game.
    Exit,
}

impl GameState {
    pub fn name(&self) -> &'static str {
        match self {
            GameState::Boot => "boot",
            GameState::MainMenu => "main-menu",
            GameState::Logistics => "logistics",
            GameState::MissionLoading(_) => "mission-loading",
            GameState::InMission => "in-mission",
            GameState::Results(_) => "results",
            GameState::Exit => "exit",
        }
    }
}

/// A request to change state (from a screen, the mission, or the command line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateRequest {
    MainMenu,
    Logistics,
    StartMission(String),
    /// The mission ended with this `scenarioResult`.
    EndMission(u32),
    /// Restart the currently running mission with its current launch configuration.
    RestartMission(String),
    /// The mission loaded (internal).
    MissionReady,
    Exit,
}

#[derive(Clone, Debug)]
pub struct StateMachine {
    state: GameState,
    /// Requests that were not allowed from the state they arrived in.
    pub rejected: Vec<(GameState, StateRequest)>,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state: GameState::Boot,
            rejected: Vec::new(),
        }
    }

    pub fn state(&self) -> &GameState {
        &self.state
    }

    /// Applies `req` if the current state allows it; returns whether the state changed.
    pub fn request(&mut self, req: StateRequest) -> bool {
        use GameState as S;
        use StateRequest as R;
        let next = match (&self.state, &req) {
            (S::Exit, _) => None,
            (_, R::Exit) => Some(S::Exit),
            (S::Boot | S::Logistics | S::Results(_) | S::InMission, R::MainMenu) => {
                Some(S::MainMenu)
            }
            (S::MissionLoading(_), R::MainMenu) => Some(S::MainMenu),
            (S::MainMenu | S::Results(_), R::Logistics) => Some(S::Logistics),
            (S::Boot | S::MainMenu | S::Logistics | S::Results(_), R::StartMission(m)) => {
                Some(S::MissionLoading(m.clone()))
            }
            (S::MissionLoading(_), R::MissionReady) => Some(S::InMission),
            (S::InMission, R::EndMission(r)) => Some(S::Results(*r)),
            (S::InMission, R::RestartMission(m)) => Some(S::MissionLoading(m.clone())),
            _ => None,
        };
        match next {
            Some(s) => {
                self.state = s;
                true
            }
            None => {
                self.rejected.push((self.state.clone(), req));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_flow() {
        let mut m = StateMachine::new();
        assert!(m.request(StateRequest::StartMission("mc2_01".into())));
        assert_eq!(m.state(), &GameState::MissionLoading("mc2_01".into()));
        assert!(m.request(StateRequest::MissionReady));
        assert_eq!(m.state(), &GameState::InMission);
        // a mission cannot be started from inside a mission
        assert!(!m.request(StateRequest::StartMission("mc2_02".into())));
        assert!(m.request(StateRequest::EndMission(3)));
        assert_eq!(m.state(), &GameState::Results(3));
        assert!(m.request(StateRequest::Logistics));
        assert!(m.request(StateRequest::StartMission("mc2_02".into())));
        assert!(m.request(StateRequest::Exit));
        assert!(!m.request(StateRequest::MainMenu));
        assert_eq!(m.state(), &GameState::Exit);
        assert_eq!(m.rejected.len(), 2);
    }

    #[test]
    fn restart_returns_to_loading_the_same_mission() {
        let mut m = StateMachine::new();
        assert!(m.request(StateRequest::StartMission("mc2_01".into())));
        assert!(m.request(StateRequest::MissionReady));
        assert!(m.request(StateRequest::RestartMission("mc2_01".into())));
        assert_eq!(m.state(), &GameState::MissionLoading("mc2_01".into()));
    }

    #[test]
    fn boot_goes_to_the_menu() {
        let mut m = StateMachine::new();
        assert!(!m.request(StateRequest::MissionReady));
        assert!(m.request(StateRequest::MainMenu));
        assert!(m.request(StateRequest::Logistics));
        assert!(m.request(StateRequest::MainMenu));
    }
}

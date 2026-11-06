// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The results flow after a mission, and the choice of the next one.
//!
//! Port of the logic of `code/missionresults.cpp` (`MissionResults::begin`, `update`, `end`)
//! with the parts of `SalvageMechScreen` (`code/salvagemecharea.cpp`) and `PilotReviewScreen`
//! (`code/pilotreviewarea.cpp`) that change game state, and of the selection logic of
//! `MissionSelectionScreen` (`code/missionselectionscreen.cpp`). The screens themselves (lists,
//! animations, sounds) are the UI crate's job; it drives this state.
//!
//! The order the original runs, after the simulation reported a big win:
//! 1. [`Logistics::apply_outcome`]: the deployed force leaves the bay, survivors come back with
//!    their pilots, objectives pay out, the campaign advances (`Logistics::start(log_RESULTS)`).
//! 2. [`Logistics::begin_results`]: the salvage screen (unless the mission that just ended
//!    skips it), then the pilot review (unless skipped), as a [`ResultsFlow`].
//! 3. [`ResultsFlow::end`]: the autosave decision.
//! 4. [`Logistics::begin_mission_selection`]: the next mission (or the choice between several)
//!    with the surviving force still in its force group slots.

use crate::error::{LogisticsError, LogisticsResult};
use crate::interface::Outcome;
use crate::logistics::{Logistics, MechKey, SalvageOffer};

/// Where the results flow is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultsStage {
    /// The salvage screen is up.
    Salvage,
    /// The pilot review is up.
    PilotReview,
    /// Nothing left to show (`MissionResults::isDone`).
    Done,
}

/// A pilot on the review screen's "active duty" list.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewedPilot {
    /// Index into the pilot roster.
    pub pilot: usize,
    /// The pilot moved up a rank in this mission (the promotion screen shows).
    pub promoted: bool,
    /// Skill points gained (gunnery, piloting), at most 4 each.
    pub gain: (f32, f32),
    /// Medals earned in this mission, as [`crate::pilot::Medal`] indices.
    pub new_medals: Vec<usize>,
    /// What the pilot killed in the mission, in order: the review's kill markers.
    pub kills: Vec<crate::pilot::KilledKind>,
    /// The unit icon of each kill, in the order of `kills`.
    pub kill_units: Vec<crate::pilot::KilledUnit>,
}

/// What the pilot review shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PilotReview {
    /// "Killed in action": pilots that died in the mission (`justDied && isUsed`).
    pub killed: Vec<usize>,
    /// "Active duty": pilots alive and used in the mission (`!isDead && isUsed`).
    pub active: Vec<ReviewedPilot>,
}

/// A request to write the autosave (`MissionResults::end`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutosaveRequest {
    /// The mission just completed (`getLastMission`), which names the file.
    mission: String,
}

impl AutosaveRequest {
    /// Mission completed by the results flow that issued this request.
    pub fn mission(&self) -> &str {
        &self.mission
    }
}

/// The state of `MissionResults` for one mission end.
#[derive(Debug, Clone)]
pub struct ResultsFlow {
    stage: ResultsStage,
    /// The salvage list (empty when the screen is skipped).
    offers: Vec<SalvageOffer>,
    /// Offers ticked so far, as indices into `offers`.
    ticked: Vec<usize>,
    review: Option<PilotReview>,
    /// The promoted pilots that picked their specialty skill, in the order they picked.
    picked: Vec<usize>,
}

impl ResultsFlow {
    /// The stage the flow is in.
    pub fn stage(&self) -> ResultsStage {
        self.stage
    }

    /// `MissionResults::isDone`.
    pub fn is_done(&self) -> bool {
        self.stage == ResultsStage::Done
    }

    /// The salvage list, cheapest first.
    pub fn offers(&self) -> &[SalvageOffer] {
        &self.offers
    }

    /// Offers currently ticked.
    pub fn ticked(&self) -> &[usize] {
        &self.ticked
    }

    /// Whether an offer can be ticked now: it is already ticked (so it can be unticked) or its
    /// price fits the C-Bills (`SalvageListItem::update` disables the others).
    pub fn offer_enabled(&self, logistics: &Logistics, offer: usize) -> bool {
        match self.offers.get(offer) {
            Some(o) => self.ticked.contains(&offer) || o.exact_cost <= logistics.c_bills() as f32,
            None => false,
        }
    }

    /// Ticks or unticks a salvage box. Ticking charges the price (and is refused when the player
    /// cannot pay); unticking gives it back.
    pub fn toggle_salvage(
        &mut self,
        logistics: &mut Logistics,
        offer: usize,
    ) -> LogisticsResult<bool> {
        if self.stage != ResultsStage::Salvage {
            return Err(LogisticsError::Failed);
        }
        let Some(o) = self.offers.get(offer) else {
            return Err(LogisticsError::InvalidId);
        };
        if let Some(pos) = self.ticked.iter().position(|&t| t == offer) {
            logistics.uncheck_salvage(o.cost);
            self.ticked.remove(pos);
            Ok(false)
        } else {
            // `SalvageListItem::update` leaves the box disabled while the float price is more
            // than the C-Bills, even when the truncated charge would fit
            if !self.offer_enabled(logistics, offer) {
                return Err(LogisticsError::NotEnoughResourcePoints);
            }
            logistics.check_salvage(o.cost)?;
            self.ticked.push(offer);
            Ok(true)
        }
    }

    /// The salvage screen closes (`SalvageMechScreen::isDone` then `updateSalvage`): the ticked
    /// mechs join the bay, in list order. Moves on to the pilot review or finishes.
    pub fn finish_salvage(&mut self, logistics: &mut Logistics) -> Vec<MechKey> {
        if self.stage != ResultsStage::Salvage {
            return Vec::new();
        }
        let mut picked = self.ticked.clone();
        picked.sort_unstable();
        let variants: Vec<_> = picked.iter().map(|&i| self.offers[i].variant).collect();
        let before = logistics.inventory().len();
        logistics.finish_salvage(&variants);
        let added = logistics.inventory()[before..]
            .iter()
            .map(|m| m.key)
            .collect();
        self.ticked.clear();
        self.stage = if self.review.is_some() {
            ResultsStage::PilotReview
        } else {
            ResultsStage::Done
        };
        added
    }

    /// What the pilot review shows (`None` when the screen is skipped).
    pub fn pilot_review(&self) -> Option<&PilotReview> {
        self.review.as_ref()
    }

    /// The promoted pilot whose specialty the review is waiting on. The original shows the
    /// promotion area for one pilot at a time, and its Done stays disabled until a skill is
    /// picked (`PilotPromotionArea::update`). A promoted pilot with nothing left to pick is not
    /// waited on.
    pub fn pending_specialty(&self, logistics: &Logistics) -> Option<usize> {
        if self.stage != ResultsStage::PilotReview {
            return None;
        }
        let review = self.review.as_ref()?;
        review
            .active
            .iter()
            .filter(|r| r.promoted && !self.picked.contains(&r.pilot))
            .map(|r| r.pilot)
            .find(|&p| {
                logistics
                    .pilots()
                    .get(p)
                    .is_some_and(|q| !q.specialty_offers().is_empty())
            })
    }

    /// `PilotPromotionArea`'s Done: the picked skill becomes the pilot's
    /// (`setSpecialtySkill`). Only the pilot the review waits on can pick, from its offers.
    pub fn choose_specialty(
        &mut self,
        logistics: &mut Logistics,
        pilot: usize,
        skill: usize,
    ) -> LogisticsResult<()> {
        if self.pending_specialty(logistics) != Some(pilot) {
            return Err(LogisticsError::Failed);
        }
        let p = logistics
            .pilots
            .get_mut(pilot)
            .ok_or(LogisticsError::InvalidId)?;
        if !p.specialty_offers().contains(&skill) {
            return Err(LogisticsError::InvalidSkill);
        }
        p.set_specialty_skill(skill, true);
        self.picked.push(pilot);
        Ok(())
    }

    /// The pilot review closes (`PilotReviewScreen::updatePilots` is empty; its destructor
    /// clears every pilot's "just died" flag).
    pub fn finish_pilot_review(&mut self, logistics: &mut Logistics) {
        if self.stage != ResultsStage::PilotReview || self.pending_specialty(logistics).is_some() {
            return;
        }
        logistics.acknowledge_pilot_review();
        self.stage = ResultsStage::Done;
    }

    /// `MissionResults::end`: whether to write the autosave. Only once the flow is done (the
    /// autosave keeps the salvage and the promotions), not after the last mission of the
    /// campaign, and not for single missions.
    pub fn end(&self, logistics: &Logistics) -> Option<AutosaveRequest> {
        let mission = logistics.last_mission();
        if self.is_done()
            && !mission.is_empty()
            && !logistics.mission_info().is_single_mission()
            && !logistics.campaign_over()
        {
            Some(AutosaveRequest {
                mission: mission.to_string(),
            })
        } else {
            None
        }
    }
}

/// A mission on the selection screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionChoice {
    /// Mission file stem.
    pub name: String,
    /// Not completed: the button is enabled.
    pub available: bool,
    /// The mission logistics is currently set up for.
    pub selected: bool,
}

impl Logistics {
    /// `MissionResults::begin` for a mission that was just completed: builds the salvage list
    /// and the pilot review the original screens would show, unless the mission skips them
    /// (`skipSalvageScreen`, `skipPilotReview`: the flags of the mission just played).
    ///
    /// Building the pilot review promotes the pilots whose skill gain moved them up a rank
    /// (`promotePilot` is called for each active pilot when the screen is built).
    ///
    /// After the last mission of the campaign there are no results screens: the original plays
    /// the final video instead (`Logistics::start`, `logistics.cpp:143`), and a single mission
    /// goes straight back to the splash screen (`logistics.cpp:160`), so the flow starts done.
    pub fn begin_results(&mut self, outcome: &Outcome) -> ResultsFlow {
        if self.campaign_over() || self.mission_info().is_single_mission() {
            return ResultsFlow {
                stage: ResultsStage::Done,
                offers: Vec::new(),
                ticked: Vec::new(),
                review: None,
                picked: Vec::new(),
            };
        }
        let skip_salvage = self.skip_salvage_screen();
        let skip_review = self.skip_pilot_review();

        let offers = if skip_salvage {
            Vec::new()
        } else {
            self.salvage_offers(&outcome.salvage_candidates())
        };
        let review = (!skip_review).then(|| self.build_pilot_review());

        let stage = if !skip_salvage {
            ResultsStage::Salvage
        } else if review.is_some() {
            ResultsStage::PilotReview
        } else {
            ResultsStage::Done
        };
        ResultsFlow {
            stage,
            offers,
            ticked: Vec::new(),
            review,
            picked: Vec::new(),
        }
    }

    /// `PilotReviewScreen::init`: the dead and the active pilots, promoting as it goes.
    fn build_pilot_review(&mut self) -> PilotReview {
        let mut review = PilotReview::default();
        for i in 0..self.pilots.len() {
            if self.pilots[i].just_died && self.pilots[i].used {
                review.killed.push(i);
            }
        }
        for i in 0..self.pilots.len() {
            if !self.pilots[i].dead && self.pilots[i].used {
                let promoted = self.pilots[i].promote_pilot();
                let p = &self.pilots[i];
                review.active.push(ReviewedPilot {
                    pilot: i,
                    promoted,
                    gain: (p.new_gunnery, p.new_piloting),
                    new_medals: p
                        .medals_last_mission
                        .iter()
                        .enumerate()
                        .filter_map(|(m, &earned)| earned.then_some(m))
                        .collect(),
                    kills: p.killed_icons.clone(),
                    kill_units: p.killed_units.clone(),
                });
            }
        }
        review
    }

    /// The destructor of `PilotReviewScreen`: no pilot is "just died" any more.
    pub(crate) fn acknowledge_pilot_review(&mut self) {
        for p in &mut self.pilots {
            p.acknowledge_review();
        }
    }

    /// `MissionSelectionScreen::begin`: lists the missions the player can see in the current
    /// operation and sets up the one the screen presses: the current mission if it is still
    /// open, else (when none is chosen yet) the first open one. Setting it up is
    /// `setCurrentMission`, which refreshes the purchase lists, the resource points and the
    /// force (mechs over the new drop weight are taken out of it).
    pub fn begin_mission_selection(&mut self) -> LogisticsResult<Vec<MissionChoice>> {
        let names = self.current_missions();
        let selected_name = self.current_mission().to_string();
        let mut pressed: Option<String> = None;
        for name in &names {
            if !self.mission_available(name) {
                continue;
            }
            if selected_name.eq_ignore_ascii_case(name) {
                pressed = Some(name.clone());
                break;
            }
            if selected_name.is_empty() {
                pressed = Some(name.clone());
                break;
            }
        }
        if let Some(name) = &pressed {
            self.set_current_mission(name)?;
        }
        Ok(self.mission_choices())
    }

    /// The selection screen's buttons: every visible mission of the operation, enabled when it
    /// is not completed, with the current one marked.
    pub fn mission_choices(&self) -> Vec<MissionChoice> {
        let current = self.current_mission();
        self.current_missions()
            .into_iter()
            .map(|name| MissionChoice {
                available: self.mission_available(&name),
                selected: current.eq_ignore_ascii_case(&name),
                name,
            })
            .collect()
    }

    /// The player pressed a mission button (`MissionSelectionScreen::setMission`).
    pub fn choose_mission(&mut self, name: &str) -> LogisticsResult<Vec<MissionChoice>> {
        self.set_current_mission(name)?;
        Ok(self.mission_choices())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata;

    /// `SalvageListItem::update` enables a box on the float price: 80.8 is more than 80 C-Bills,
    /// though ticking would charge 80.
    #[test]
    fn a_box_waits_for_the_float_price_not_the_truncated_charge() {
        let mut l = Logistics::new(testdata::data()).unwrap();
        let variant = l.variant_by_name("Alpha Prime").expect("variant");
        let flow = ResultsFlow {
            stage: ResultsStage::Salvage,
            offers: vec![SalvageOffer {
                candidate: 0,
                variant,
                variant_name: "Alpha Prime".into(),
                cost: 80,
                exact_cost: 80.8,
                tonnage: 50.0,
                paint_scheme: [0; 3],
                armor: [(0.0, 0.0, false); 11],
            }],
            ticked: Vec::new(),
            review: None,
            picked: Vec::new(),
        };
        l.add_c_bills(80 - l.c_bills());
        assert!(!flow.offer_enabled(&l, 0), "80.8 is more than 80 C-Bills");
        l.add_c_bills(1);
        assert!(flow.offer_enabled(&l, 0), "81 C-Bills cover 80.8");
    }

    /// A box the screen leaves disabled cannot be ticked through `toggle_salvage` either: 80 C-Bills
    /// cover the truncated charge, but not the 80.8 the box compares.
    #[test]
    fn a_disabled_box_charges_nothing_when_ticked() {
        let mut l = Logistics::new(testdata::data()).unwrap();
        let variant = l.variant_by_name("Alpha Prime").expect("variant");
        let mut flow = ResultsFlow {
            stage: ResultsStage::Salvage,
            offers: vec![SalvageOffer {
                candidate: 0,
                variant,
                variant_name: "Alpha Prime".into(),
                cost: 80,
                exact_cost: 80.8,
                tonnage: 50.0,
                paint_scheme: [0; 3],
                armor: [(0.0, 0.0, false); 11],
            }],
            ticked: Vec::new(),
            review: None,
            picked: Vec::new(),
        };
        l.add_c_bills(80 - l.c_bills());
        assert!(
            flow.toggle_salvage(&mut l, 0).is_err(),
            "the box is refused"
        );
        assert_eq!(l.c_bills(), 80, "nothing is charged");
        assert!(flow.ticked().is_empty(), "the offer is not ticked");
    }
}

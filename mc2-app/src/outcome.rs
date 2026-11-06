//! The hand-over from a finished mission to the campaign: `mc2_sim`'s end-of-mission report as the
//! `mc2-campaign` [`Outcome`] the results flow (`LogisticsData::setMissionCompleted`,
//! `MissionResults`) reads.  The two types have the same fields (the sim's are documented as
//! matching the campaign's); this is the field-for-field copy between the crates.

use mc2_campaign::interface as campaign;
use mc2_campaign::pilot as campaign_pilot;
use mc2_sim::mission::objective::ObjectiveStatus as SimObjectiveStatus;
use mc2_sim::mission::outcome as sim;

fn result(r: sim::MissionResult) -> campaign::MissionResult {
    match r {
        sim::MissionResult::Playing => campaign::MissionResult::Playing,
        sim::MissionResult::PlayerLostBig => campaign::MissionResult::PlayerLostBig,
        sim::MissionResult::PlayerLostSmall => campaign::MissionResult::PlayerLostSmall,
        sim::MissionResult::PlayerDraw => campaign::MissionResult::PlayerDraw,
        sim::MissionResult::PlayerWinSmall => campaign::MissionResult::PlayerWinSmall,
        sim::MissionResult::PlayerWinBig => campaign::MissionResult::PlayerWinBig,
    }
}

fn objective_status(s: SimObjectiveStatus) -> campaign::ObjectiveStatus {
    match s {
        SimObjectiveStatus::Undetermined => campaign::ObjectiveStatus::Undetermined,
        SimObjectiveStatus::Successful => campaign::ObjectiveStatus::Successful,
        SimObjectiveStatus::Failed => campaign::ObjectiveStatus::Failed,
    }
}

fn warrior_status(s: sim::WarriorStatus) -> campaign_pilot::WarriorStatus {
    use campaign_pilot::WarriorStatus as C;
    use sim::WarriorStatus as S;
    match s {
        S::Normal => C::Normal,
        S::Withdrawing => C::Withdrawing,
        S::Withdrawn => C::Withdrawn,
        S::Ejected => C::Ejected,
        S::Dead => C::Dead,
        S::Mia => C::Mia,
        S::Captured => C::Captured,
        S::Basecamp => C::Basecamp,
    }
}

fn killed(k: sim::KilledKind) -> campaign_pilot::KilledKind {
    match k {
        sim::KilledKind::BattleMech { air } => campaign_pilot::KilledKind::BattleMech { air },
        sim::KilledKind::GroundVehicle => campaign_pilot::KilledKind::GroundVehicle,
        sim::KilledKind::Other => campaign_pilot::KilledKind::Other,
    }
}

fn pilot(p: sim::PilotReport) -> campaign_pilot::PilotReport {
    campaign_pilot::PilotReport {
        status: warrior_status(p.status),
        gunnery_rank: p.gunnery_rank,
        piloting_rank: p.piloting_rank,
        gunnery_points: p.gunnery_points,
        piloting_points: p.piloting_points,
        mission_kills: p.mission_kills,
        rank: p.rank,
        wounds: p.wounds,
        killed: p.killed.into_iter().map(killed).collect(),
        killed_units: p
            .killed_units
            .into_iter()
            .map(|u| campaign_pilot::KilledUnit {
                picture: u.picture,
                armor: u.armor,
            })
            .collect(),
        specialty_skills: p.specialty_skills,
    }
}

/// The campaign's [`Outcome`](campaign::Outcome) for the sim's end-of-mission report.
pub fn to_campaign(o: sim::Outcome) -> campaign::Outcome {
    campaign::Outcome {
        result: result(o.result),
        objectives: o
            .objectives
            .into_iter()
            .map(|r| campaign::ObjectiveReport {
                status: objective_status(r.status),
                resource_points: r.resource_points,
            })
            .collect(),
        unlocked_purchase_files: o.unlocked_purchase_files,
        movers: o
            .movers
            .into_iter()
            .map(|m| campaign::MoverReport {
                on_gui: m.on_gui,
                is_battle_mech: m.is_battle_mech,
                home_commander: m.home_commander,
                is_air: m.is_air,
                destroyed: m.destroyed,
                disabled: m.disabled,
                variant_name: m.variant_name,
                pilot_file: m.pilot_file,
                paint_scheme: m.paint_scheme,
                pilot: pilot(m.pilot),
            })
            .collect(),
        mechs: o
            .mechs
            .into_iter()
            .map(|m| campaign::MechSummary {
                variant_name: m.variant_name,
                tonnage: m.tonnage,
                destroyed: m.destroyed,
                disabled: m.disabled,
                move_level: m.move_level,
                paint_scheme: m.paint_scheme,
                armor: m.armor,
            })
            .collect(),
        mission_time: o.mission_time,
    }
}

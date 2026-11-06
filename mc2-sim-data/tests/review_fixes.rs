// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Regression tests for the Codex review of mc2-sim-data (synthetic FIT text; no data needed).
//!
//! - Counts read from FIT files are bounded by the block list, so a tiny file with a huge
//!   count is an error and does not loop or allocate.
//! - The campaign mission settings keep the `UseResourceString` flags and choose the
//!   displayed text as `cLoadString` does (logisticsmissioninfo.cpp:272-306).

use mc2_formats::ini::FitIni;
use mc2_sim_data::campaign::{Campaign, DisplayText, MissionSettings};
use mc2_sim_data::mission::Mission;

fn fit(text: &str) -> FitIni {
    FitIni::from_bytes(format!("FITini\r\n\r\n{text}\r\nFITend\r\n").into_bytes()).expect("parse")
}

#[test]
fn campaign_group_count_larger_than_the_file_is_an_error() {
    let mut f = fit("[Campaign]\r\nl GroupCount = 2000000000\r\nl CBills = 0\r\n");
    let err = Campaign::from_fit("campaign.fit", &mut f, |_| Ok(None))
        .expect_err("a count past the block list must not load");
    assert!(err.to_string().contains("GroupCount"), "{err}");
}

#[test]
fn campaign_mission_count_larger_than_the_file_is_an_error() {
    let mut f =
        fit("[Campaign]\r\nl GroupCount = 1\r\n[Group0]\r\nl MissionCount = 2000000000\r\n");
    let err = Campaign::from_fit("campaign.fit", &mut f, |_| Ok(None))
        .expect_err("a mission count past the block list must not load");
    assert!(err.to_string().contains("MissionCount"), "{err}");
}

#[test]
fn mission_part_count_larger_than_the_file_is_an_error() {
    let mut f = fit(
        "[MissionSettings]\r\nl ResourcePoints = 0\r\nuc scenarioTuneNum = 1\r\n\
         [Script]\r\nst ScenarioScript = \"none\"\r\n\
         [Warriors]\r\nul NumWarriors = 0\r\n\
         [Parts]\r\nul NumParts = 2000000000\r\n",
    );
    let err = Mission::load(&mut f, "test").expect_err("a part count past the block list");
    assert!(err.to_string().contains("NumParts"), "{err}");
}

#[test]
fn mission_name_uses_the_resource_when_the_flag_is_set() {
    let mut f = fit("[MissionSettings]\r\n\
         b MissionNameUseResourceString = TRUE\r\n\
         ul MissionNameResourceStringID = 5\r\n\
         st MissionName = \"Literal\"\r\n");
    let s = MissionSettings::from_fit(&mut f).expect("settings");
    assert!(s.mission_name_use_resource_string);
    assert_eq!(s.display_name(), Some(DisplayText::Resource(5)));
}

#[test]
fn mission_name_is_the_literal_when_the_flag_is_clear() {
    let mut f = fit("[MissionSettings]\r\n\
         b MissionNameUseResourceString = FALSE\r\n\
         ul MissionNameResourceStringID = 5\r\n\
         st MissionName = \"Literal\"\r\n");
    let s = MissionSettings::from_fit(&mut f).expect("settings");
    assert_eq!(
        s.display_name(),
        Some(DisplayText::Literal("Literal".to_string()))
    );
}

#[test]
fn blurb_uses_the_resource_when_its_flag_and_id_are_present() {
    let mut f = fit("[MissionSettings]\r\n\
         st Blurb2 = \"Text\"\r\n\
         b Blurb2UseResourceString = TRUE\r\n\
         ul Blurb2ResourceStringID = 6\r\n");
    let s = MissionSettings::from_fit(&mut f).expect("settings");
    assert!(s.blurb2_use_resource_string);
    assert_eq!(s.display_blurb(), Some(DisplayText::Resource(6)));
}

#[test]
fn blurb_falls_back_to_the_literal_without_a_resource_id() {
    let mut f = fit("[MissionSettings]\r\n\
         st Blurb2 = \"Text\"\r\n\
         b Blurb2UseResourceString = TRUE\r\n");
    let s = MissionSettings::from_fit(&mut f).expect("settings");
    assert_eq!(
        s.display_blurb(),
        Some(DisplayText::Literal("Text".to_string()))
    );
}

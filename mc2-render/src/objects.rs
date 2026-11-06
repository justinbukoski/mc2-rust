// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Object type table and appearance lookup.
//!
//! Ported from `ObjectTypeManager::load` (`code/objtype.cpp`: packet `n` of `object2.pak` is the
//! FIT of object type `n`, `[ObjectClass] ObjectTypeNum` is the type class),
//! `AppearanceType::init` and the `[TGLData]` readers in `BldgAppearanceType::init` /
//! `TreeAppearanceType::init` (`bdactor.cpp`) for the shape file names.

use crate::appearance::MAX_LODS;
use crate::error::{optional_key, RenderError};
use crate::game::GameData;
use crate::math::Vec3;
use mc2_formats::ini::FitIni;
use mc2_formats::pak::PakFile;
use std::collections::HashMap;

/// `ObjectTypeClass` (`dobjtype.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ObjectClass {
    Tree,
    Building,
    BattleMech,
    Vehicle,
    TerrainObject,
    Bridge,
    TreeBuilding,
    Turret,
    Gate,
    Other(i32),
}

impl ObjectClass {
    pub fn from_type_num(n: i32) -> Self {
        match n {
            0 => ObjectClass::Tree,
            1 => ObjectClass::Building,
            2 => ObjectClass::BattleMech,
            3 => ObjectClass::Vehicle,
            11 => ObjectClass::TerrainObject,
            15 => ObjectClass::Bridge,
            18 => ObjectClass::TreeBuilding,
            21 => ObjectClass::Turret,
            22 => ObjectClass::Gate,
            other => ObjectClass::Other(other),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectTypeInfo {
    pub number: i32,
    pub class: ObjectClass,
    pub name: String,
    pub appearance: String,
}

/// `object2.pak`.
pub struct ObjectTypes {
    pak: PakFile,
    cache: HashMap<i32, Option<ObjectTypeInfo>>,
}

impl ObjectTypes {
    pub fn open(game: &mut GameData) -> crate::Result<Self> {
        Ok(Self {
            pak: game.pak("data/objects/object2.pak")?,
            cache: HashMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.pak.packet_count()
    }

    pub fn is_empty(&self) -> bool {
        self.pak.packet_count() == 0
    }

    pub fn get(&mut self, number: i32) -> Option<&ObjectTypeInfo> {
        if !self.cache.contains_key(&number) {
            let info = self.read(number);
            self.cache.insert(number, info);
        }
        self.cache.get(&number)?.as_ref()
    }

    fn read(&self, number: i32) -> Option<ObjectTypeInfo> {
        // Object 0 is always NULL.
        if number <= 0 || number as usize >= self.pak.packet_count() {
            return None;
        }
        let bytes = self.pak.read_packet(number as usize).ok()?;
        if bytes.is_empty() {
            return None;
        }
        let mut fit = FitIni::from_bytes(bytes).ok()?;
        fit.seek_block("ObjectClass").ok()?;
        let class = ObjectClass::from_type_num(fit.read_i32("ObjectTypeNum").ok()?);
        fit.seek_block("ObjectType").ok()?;
        let appearance = fit.read_string("AppearanceName", 255).ok()?;
        let name = fit.read_string("Name", 255).unwrap_or_default();
        Some(ObjectTypeInfo {
            number,
            class,
            name,
            appearance,
        })
    }
}

/// A building's ground light for the night (`TerrainLightRGB` and friends in `[TGLData]`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainLightSpec {
    /// `0x00RRGGBB`.
    pub rgb: u32,
    pub intensity: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
}

/// Colours of the marker vertices (`HotPinkRGB` windows, `HotGreenRGB` base lights,
/// `HotYellowRGB` outside lights).  Defaults are the C++'s (`BldgAppearanceType::init`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GlowColors {
    pub pink: u32,
    pub yellow: u32,
    pub green: u32,
}

impl Default for GlowColors {
    fn default() -> Self {
        Self {
            pink: 0xffff_00ff,
            yellow: 0xffff_ff00,
            green: 0xff00_ff00,
        }
    }
}

/// One level of detail of an appearance: its shape file and the eye distance past which it is used.
#[derive(Clone, Debug, PartialEq)]
pub struct LodFile {
    pub file: String,
    /// `Distance<i>` (0 for the full detail shape).
    pub distance: f32,
}

/// Shape file names of an appearance (`data/tgl/<appearance>.ini`).
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub name: String,
    /// Shape files by level of detail.  `lods[0]` is the full detail shape: `FileName` when the
    /// appearance has a single shape (trees, most props), else `FileName0`.  `lods[i]` is `FileName<i>`
    /// with `Distance<i>` (`bdactor.cpp:153-185`, `mech3d.cpp:255-270`).
    pub lods: [Option<LodFile>; MAX_LODS],
    /// Flattened shadow shape (`ShadowName`).
    pub shadow: Option<String>,
    /// Damaged shape (`[TGLDamage] FileName`).
    pub damaged: Option<String>,
    /// Optional designer type box (`[3DBounds]`), in the Stuff frame.
    pub selection_bounds: Option<(Vec3, Vec3)>,
    pub glow: GlowColors,
    /// Night light on the ground around the building, if the appearance has one.
    pub terrain_light: Option<TerrainLightSpec>,
}

impl Appearance {
    /// The full detail shape file (`lods[0]`).
    pub fn shape(&self) -> &str {
        self.lods[0].as_ref().map_or("", |l| l.file.as_str())
    }

    /// Reads the `[TGLData]` and `[TGLDamage]` blocks.  An absent key takes its C++ default; a key that is present
    /// but malformed, a LOD without its `Distance<i>`, and a missing `[TGLData]` block or base shape are errors.
    pub fn parse(name: &str, fit: &mut FitIni) -> crate::Result<Self> {
        fit.seek_block("TGLData")?;
        // `readIdString("FileName")` first: a plain FileName is the only shape (`bdactor.cpp:153`).
        // Otherwise the LOD list is read (`FileName<i>` and `Distance<i>`, `bdactor.cpp:157-185`).
        let mut lods: [Option<LodFile>; MAX_LODS] = Default::default();
        if let Some(file) = optional_key(fit.read_string("FileName", 511))? {
            lods[0] = Some(LodFile {
                file,
                distance: 0.0,
            });
        } else {
            for (i, lod) in lods.iter_mut().enumerate() {
                let Some(file) = optional_key(fit.read_string(&format!("FileName{i}"), 511))?
                else {
                    continue;
                };
                // Every `FileName<i>` needs its `Distance<i>`, the base one included: the C++ reads it for each index
                // and fails rather than choosing a default (`bdactor.cpp:157-185`, `mech3d.cpp:255-270`).
                let distance = fit.read_f32(&format!("Distance{i}"))?;
                *lod = Some(LodFile { file, distance });
            }
        }
        // A base shape is required (`bdactor.cpp:183`, "No base LOD for shape").
        if lods[0].is_none() {
            return Err(RenderError::Other(format!(
                "{name}: no base LOD (FileName or FileName0)"
            )));
        }
        let shadow = optional_key(fit.read_string("ShadowName", 511))?;
        let mut glow = GlowColors::default();
        glow.pink = optional_key(fit.read_u32("HotPinkRGB"))?.unwrap_or(glow.pink);
        glow.green = optional_key(fit.read_u32("HotGreenRGB"))?.unwrap_or(glow.green);
        glow.yellow = optional_key(fit.read_u32("HotYellowRGB"))?.unwrap_or(glow.yellow);
        // `TerrainLightRGB` of 0xffffffff (the default) means no light; the other three
        // values only count when it is present, with the C++'s defaults.
        let terrain_light = match optional_key(fit.read_u32("TerrainLightRGB"))? {
            Some(rgb) if rgb != 0xffff_ffff => Some(TerrainLightSpec {
                rgb,
                intensity: optional_key(fit.read_f32("TerrainLightIntensity"))?.unwrap_or(0.5),
                inner_radius: optional_key(fit.read_f32("TerrainLightInnerRadius"))?
                    .unwrap_or(100.0),
                outer_radius: optional_key(fit.read_f32("TerrainLightOuterRadius"))?
                    .unwrap_or(250.0),
            }),
            _ => None,
        };
        // A `[TGLDamage]` block without its `FileName` is an error, as the C++ readers have it (`bdactor.cpp:215-220`,
        // `bdactor.cpp:3135-3140`, `gvactor.cpp:182-187`): the object would otherwise show its intact shape when destroyed.
        let damaged = match optional_key(fit.seek_block("TGLDamage"))? {
            Some(()) => Some(fit.read_string("FileName", 511)?),
            None => None,
        };
        // Once the C++ finds the block it requires all six signed-long fields
        // (`AppearanceType::init`, apprtype.cpp:80-121).
        let selection_bounds = match optional_key(fit.seek_block("3DBounds"))? {
            Some(()) => {
                let point = |prefix: &str| -> crate::Result<Vec3> {
                    Ok(Vec3::new(
                        fit.read_i64(&format!("{prefix}X"))? as f32,
                        fit.read_i64(&format!("{prefix}Y"))? as f32,
                        fit.read_i64(&format!("{prefix}Z"))? as f32,
                    ))
                };
                Some((point("UpperLeft")?, point("LowerRight")?))
            }
            None => None,
        };
        Ok(Appearance {
            name: name.to_string(),
            lods,
            shadow,
            damaged,
            selection_bounds,
            glow,
            terrain_light,
        })
    }

    /// `data/tgl/<name>.ini`: `None` when the appearance is not in the data, an error when it is there and fails.
    pub fn load(game: &mut GameData, name: &str) -> crate::Result<Option<Self>> {
        let Some(bytes) = game.read_opt(&format!("data/tgl/{name}.ini"))? else {
            return Ok(None);
        };
        let mut fit = FitIni::from_bytes(bytes)?;
        Self::parse(name, &mut fit).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(s: &str) -> FitIni {
        FitIni::from_bytes(s.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn classes_follow_the_enum_order() {
        assert_eq!(ObjectClass::from_type_num(0), ObjectClass::Tree);
        assert_eq!(ObjectClass::from_type_num(1), ObjectClass::Building);
        assert_eq!(ObjectClass::from_type_num(2), ObjectClass::BattleMech);
        assert_eq!(ObjectClass::from_type_num(11), ObjectClass::TerrainObject);
        assert_eq!(ObjectClass::from_type_num(18), ObjectClass::TreeBuilding);
        assert_eq!(ObjectClass::from_type_num(7), ObjectClass::Other(7));
    }

    #[test]
    fn building_appearance_reads_lods_shadow_and_damage() {
        let mut f = fit(
            "FITini\r\n[Bounds]\r\nl UpperLeftX=-20\r\n[TGLData]\r\nst FileName0=\"tinshack\"\r\nf Distance0 = 0.0\r\nst FileName1=\"TinShackL1\"\r\nf Distance1 = 1000.0\r\nst ShadowName=\"TinShackX\"\r\n[TGLDamage]\r\nst FileName=\"tinshackdam\"\r\nst ShadowName=\"TinShackDamX\"\r\nFITend\r\n",
        );
        let a = Appearance::parse("tinshack", &mut f).unwrap();
        assert_eq!(a.shape(), "tinshack");
        assert_eq!(a.lods[0].as_ref().unwrap().distance, 0.0);
        assert_eq!(
            a.lods[1],
            Some(LodFile {
                file: "TinShackL1".into(),
                distance: 1000.0
            })
        );
        assert!(a.lods[2].is_none());
        assert_eq!(a.shadow.as_deref(), Some("TinShackX"));
        assert_eq!(a.damaged.as_deref(), Some("tinshackdam"));
    }

    #[test]
    fn designer_selection_bounds_are_required_and_preserved() {
        // AppearanceType::init reads all six signed-long values once [3DBounds] exists
        // (mclib/apprtype.cpp:80-121); it does not silently fall back to the mesh box.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\n[3DBounds]\r\nl UpperLeftX=-20\r\nl UpperLeftY=30\r\nl UpperLeftZ=40\r\nl LowerRightX=50\r\nl LowerRightY=-60\r\nl LowerRightZ=-70\r\nFITend\r\n",
        );
        let a = Appearance::parse("base", &mut f).unwrap();
        assert_eq!(
            a.selection_bounds,
            Some((Vec3::new(-20.0, 30.0, 40.0), Vec3::new(50.0, -60.0, -70.0)))
        );

        let mut incomplete = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\n[3DBounds]\r\nl UpperLeftX=0\r\nFITend\r\n",
        );
        assert!(Appearance::parse("base", &mut incomplete).is_err());
    }

    #[test]
    fn every_lod_index_up_to_max_lods_is_read() {
        // bdactor.cpp:157 loops `i < MAX_LODS` over FileName<i>/Distance<i>.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName0=\"base\"\r\nf Distance0 = 0.0\r\nst FileName1=\"mid\"\r\nf Distance1 = 400.0\r\nst FileName2=\"far\"\r\nf Distance2 = 1500.0\r\nFITend\r\n",
        );
        let a = Appearance::parse("base", &mut f).unwrap();
        assert_eq!(a.shape(), "base");
        assert_eq!(a.lods[1].as_ref().unwrap().distance, 400.0);
        assert_eq!(a.lods[2].as_ref().unwrap().file, "far");
        assert_eq!(a.lods[2].as_ref().unwrap().distance, 1500.0);
    }

    #[test]
    fn a_plain_file_name_wins_over_the_lod_list() {
        // bdactor.cpp:153 reads `FileName` first; the LOD list is only read when it is absent.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"single\"\r\nst FileName1=\"ignored\"\r\nf Distance1 = 10.0\r\nFITend\r\n",
        );
        let a = Appearance::parse("single", &mut f).unwrap();
        assert_eq!(a.shape(), "single");
        assert!(a.lods[1].is_none());
    }

    #[test]
    fn tree_appearance_uses_the_plain_file_name_key() {
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"palm1\"\r\nb ForestClump=FALSE\r\nFITend\r\n",
        );
        let a = Appearance::parse("Palm1", &mut f).unwrap();
        assert_eq!(a.shape(), "palm1");
        assert!(a.lods[1].is_none() && a.shadow.is_none() && a.damaged.is_none());
    }

    #[test]
    fn a_damage_block_without_its_file_name_is_an_error() {
        // bdactor.cpp:215-220 (and 3135-3140, gvactor.cpp:182-187): a present [TGLDamage] without FileName fails the load.
        // Treating it as no damage shape would show the intact shape while destroyed.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\n[TGLDamage]\r\nst ShadowName=\"BaseDamX\"\r\nFITend\r\n",
        );
        let err = Appearance::parse("base", &mut f).unwrap_err();
        assert!(err.to_string().contains("FileName"), "{err}");
    }

    #[test]
    fn an_absent_damage_block_is_no_damage_shape() {
        // No [TGLDamage] block at all: the appearance has no damage shape (bdactor.cpp:215 `seekBlock` fails).
        let mut f = fit("FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\nFITend\r\n");
        let a = Appearance::parse("base", &mut f).unwrap();
        assert!(a.damaged.is_none());
    }

    #[test]
    fn missing_tgl_block_is_an_error() {
        let mut f = fit("FITini\r\n[Bounds]\r\nl UpperLeftX=-20\r\nFITend\r\n");
        assert!(Appearance::parse("x", &mut f).is_err());
    }

    #[test]
    fn a_lod_without_its_distance_is_an_error() {
        // bdactor.cpp:157-185: `Distance<i>` is read with each `FileName<i>`; there is no default to fall back on.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName0=\"base\"\r\nf Distance0 = 0.0\r\nst FileName1=\"mid\"\r\nFITend\r\n",
        );
        let err = Appearance::parse("base", &mut f).unwrap_err();
        assert!(err.to_string().contains("Distance1"), "{err}");
    }

    #[test]
    fn a_base_lod_without_its_distance_is_an_error() {
        // The base LOD is read like the others: `FileName0` without `Distance0` fails (bdactor.cpp:157-185,
        // mech3d.cpp:255-270), even with the other LODs complete.
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName0=\"base\"\r\nst FileName1=\"mid\"\r\nf Distance1 = 400.0\r\nFITend\r\n",
        );
        let err = Appearance::parse("base", &mut f).unwrap_err();
        assert!(err.to_string().contains("Distance0"), "{err}");
    }

    #[test]
    fn a_malformed_distance_is_an_error_not_a_default() {
        let mut f = fit(
            "FITini\r\n[TGLData]\r\nst FileName0=\"base\"\r\nf Distance0 = 0.0\r\nst FileName1=\"mid\"\r\nst Distance1=\"far\"\r\nFITend\r\n",
        );
        assert!(Appearance::parse("base", &mut f).is_err());
    }

    #[test]
    fn an_absent_optional_key_takes_its_default_and_a_present_one_is_read_as_the_c_reads_it() {
        // TerrainLightIntensity is absent: the C++ default 0.5 (`bdactor.cpp`).
        let mut absent = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\nul TerrainLightRGB=2000000000\r\nFITend\r\n",
        );
        assert_eq!(
            Appearance::parse("base", &mut absent)
                .unwrap()
                .terrain_light
                .unwrap()
                .intensity,
            0.5
        );
        // Present but not a number: `readIdFloat` is `atof`, so it reads 0 here, as the C++ does (FIT floats are
        // not an error the way a missing key or file is).
        let mut junk = fit(
            "FITini\r\n[TGLData]\r\nst FileName=\"base\"\r\nul TerrainLightRGB=2000000000\r\nf TerrainLightIntensity = bright\r\nFITend\r\n",
        );
        assert_eq!(
            Appearance::parse("base", &mut junk)
                .unwrap()
                .terrain_light
                .unwrap()
                .intensity,
            0.0
        );
    }

    #[test]
    fn no_base_shape_is_an_error() {
        // bdactor.cpp:183 "No base LOD for shape".
        let mut f =
            fit("FITini\r\n[TGLData]\r\nst FileName1=\"mid\"\r\nf Distance1 = 5.0\r\nFITend\r\n");
        assert!(Appearance::parse("x", &mut f).is_err());
    }

    #[test]
    fn appearance_file_absent_from_the_data_is_none_and_a_broken_one_is_an_error() {
        // The file system part of the same rule: `load` tells the two apart.
        let root =
            std::env::temp_dir().join(format!("mc2-render-appearance-{}", std::process::id()));
        std::fs::create_dir_all(root.join("data/tgl")).unwrap();
        std::fs::write(
            root.join("data/tgl/broken.ini"),
            "FITini\r\n[TGLData]\r\nst FileName0=\"base\"\r\nf Distance0 = 0.0\r\nst FileName1=\"mid\"\r\nFITend\r\n",
        )
        .unwrap();
        std::fs::write(
            root.join("data/tgl/ok.ini"),
            "FITini\r\n[TGLData]\r\nst FileName=\"single\"\r\nFITend\r\n",
        )
        .unwrap();
        let mut game = GameData {
            vfs: mc2_formats::vfs::Vfs::new(&root),
        };
        assert!(Appearance::load(&mut game, "absent").unwrap().is_none());
        assert_eq!(
            Appearance::load(&mut game, "ok").unwrap().unwrap().shape(),
            "single"
        );
        assert!(matches!(
            Appearance::load(&mut game, "broken"),
            Err(RenderError::Other(_) | RenderError::Format(_))
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }
}

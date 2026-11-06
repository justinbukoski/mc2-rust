// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MidLevelRenderer::MLRState` (`mclib/mlr/mlrstate.{hpp,cpp}`): the render and process state
//! words every effect carries, how a child's state is combined with its parent's, and how the
//! load reads them.

use crate::error::Result;
use crate::stream::Stream;

/// `Limits::Max_Number_Of_Texture_Bits`.
pub const TEXTURE_BITS: u32 = 14;
pub const TEXTURE_MASK: u32 = (1 << TEXTURE_BITS) - 1;
pub const ALPHA_BIT: u32 = TEXTURE_BITS;
pub const ALPHA_MASK: u32 = 0b111 << ALPHA_BIT;
pub const FILTER_BIT: u32 = ALPHA_BIT + 3;
pub const FILTER_MASK: u32 = 0b11 << FILTER_BIT;
pub const FOG_BIT: u32 = FILTER_BIT + 2;
pub const FOG_MASK: u32 = 0b11 << FOG_BIT;
pub const TEXTURE_WRAP_BIT: u32 = FOG_BIT + 2;
pub const TEXTURE_WRAP_MASK: u32 = 1 << TEXTURE_WRAP_BIT;
pub const DITHER_BIT: u32 = TEXTURE_WRAP_BIT + 1;
pub const DITHER_MASK: u32 = 1 << DITHER_BIT;
pub const TEXTURE_CORRECTION_BIT: u32 = DITHER_BIT + 1;
pub const TEXTURE_CORRECTION_MASK: u32 = 1 << TEXTURE_CORRECTION_BIT;
pub const WIREFRAME_BIT: u32 = TEXTURE_CORRECTION_BIT + 1;
pub const WIREFRAME_MASK: u32 = 0b11 << WIREFRAME_BIT;
pub const ZBUFFER_WRITE_BIT: u32 = WIREFRAME_BIT + 2;
pub const ZBUFFER_WRITE_MASK: u32 = 1 << ZBUFFER_WRITE_BIT;
pub const ZBUFFER_COMPARE_BIT: u32 = ZBUFFER_WRITE_BIT + 1;
pub const ZBUFFER_COMPARE_MASK: u32 = 1 << ZBUFFER_COMPARE_BIT;
pub const SPECULAR_BIT: u32 = ZBUFFER_COMPARE_BIT + 1;
pub const SPECULAR_MASK: u32 = 1 << SPECULAR_BIT;
pub const FLAT_COLORING_BIT: u32 = SPECULAR_BIT + 1;
pub const FLAT_COLORING_MASK: u32 = 1 << FLAT_COLORING_BIT;

pub const PRIORITY_MASK: u32 = 0xf;
pub const ALPHA_PRIORITY: u32 = 2;

/// `MLRState::AlphaMode`: how a pixel is blended into the frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlendMode {
    /// `OneZeroMode`: opaque.
    OneZero,
    /// `OneOneMode`: additive (`src + dst`).
    OneOne,
    /// `AlphaOneMode`: `src * alpha + dst`.
    AlphaOne,
    /// `OneAlphaMode`: `src + dst * alpha`.
    OneAlpha,
    /// `AlphaInvAlphaMode`: the usual alpha blend.
    AlphaInvAlpha,
    /// `OneInvAlphaMode`: premultiplied alpha.
    OneInvAlpha,
}

impl BlendMode {
    fn from_bits(v: u32) -> BlendMode {
        match v {
            1 => BlendMode::OneOne,
            2 => BlendMode::AlphaOne,
            3 => BlendMode::OneAlpha,
            4 => BlendMode::AlphaInvAlpha,
            5 => BlendMode::OneInvAlpha,
            _ => BlendMode::OneZero,
        }
    }
}

/// A texture an effect names: the file base name and the instance word the file stored (the
/// low four bits are the instance, the rest the MLR "hint").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureRef {
    pub name: String,
    pub instance: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlrState {
    pub render_state: u32,
    pub render_delta_mask: u32,
    pub render_permission_mask: u32,
    pub process_state: u32,
    pub process_delta_mask: u32,
    pub process_permission_mask: u32,
    /// The texture named by the render state's texture bits, when there is one.
    pub texture: Option<TextureRef>,
}

impl Default for MlrState {
    /// `MLRState()`: nothing set, everything permitted.
    fn default() -> Self {
        Self {
            render_state: 0,
            render_delta_mask: 0,
            render_permission_mask: 0xFFFF_FFFF,
            process_state: 0,
            process_delta_mask: 0,
            process_permission_mask: 0xFFFF_FFFF,
            texture: None,
        }
    }
}

/// `Stuff::FirstMLRClassID` (`mclib/stuff/stuff.hpp`): the block after 64 Stuff class ids.
pub const FIRST_MLR_CLASS_ID: i32 = 64;

const MLR_SIGNATURE: u32 = 0x4D4C_5223; // 'MLR#'
pub const CURRENT_MLR_VERSION: i32 = 9;

/// `MidLevelRenderer::ReadMLRVersion`: a signature then the version, or (no signature) version
/// -1 with the four bytes put back.
pub fn read_mlr_version(s: &mut Stream) -> Result<i32> {
    let sig = s.u32()?;
    let mut version = -1;
    if sig == MLR_SIGNATURE {
        version = s.i32()?;
    } else {
        s.rewind(4);
    }
    if version > CURRENT_MLR_VERSION {
        return Err(crate::error::FxError::new(format!(
            "MLR data version {version} is newer than {CURRENT_MLR_VERSION}"
        )));
    }
    Ok(version)
}

impl MlrState {
    /// `MLRState::Load`.
    pub fn load(s: &mut Stream, version: i32) -> Result<MlrState> {
        let mut st = MlrState {
            render_state: s.u32()?,
            render_delta_mask: s.u32()?,
            render_permission_mask: s.u32()?,
            process_state: s.u32()?,
            process_delta_mask: s.u32()?,
            process_permission_mask: s.u32()?,
            texture: None,
        };
        if version < 6 {
            let _near = s.f32()?;
            let _far = s.f32()?;
        }
        if version > 3 {
            if version < 6 {
                let _density = s.f32()?;
                let _color = s.i32()?;
            }
        } else {
            st.render_state &= !(TEXTURE_WRAP_MASK | SPECULAR_MASK);
            st.render_delta_mask &= !(TEXTURE_WRAP_MASK | SPECULAR_MASK);
            st.render_permission_mask |= TEXTURE_WRAP_MASK | SPECULAR_MASK;
        }
        if st.render_state & TEXTURE_MASK != 0 {
            let name = s.mstring()?;
            let instance = s.i32()?;
            st.texture = Some(TextureRef { name, instance });
        }
        Ok(st)
    }

    pub fn blend(&self) -> BlendMode {
        BlendMode::from_bits((self.render_state & ALPHA_MASK) >> ALPHA_BIT)
    }
    pub fn set_blend_bits(&mut self, bits: u32) {
        self.render_state = (self.render_state & !ALPHA_MASK) | (bits << ALPHA_BIT);
        self.render_delta_mask |= ALPHA_MASK;
    }
    pub fn priority(&self) -> u32 {
        self.process_state & PRIORITY_MASK
    }
    pub fn set_priority(&mut self, p: u32) {
        self.process_state = (self.process_state & !PRIORITY_MASK) | p;
        self.process_delta_mask |= PRIORITY_MASK;
    }
    pub fn z_write(&self) -> bool {
        self.render_state & ZBUFFER_WRITE_MASK != 0
    }
    pub fn z_compare(&self) -> bool {
        self.render_state & ZBUFFER_COMPARE_MASK != 0
    }
    pub fn clamp_texture(&self) -> bool {
        self.render_state & TEXTURE_WRAP_MASK != 0
    }
    pub fn set_z_write(&mut self, on: bool) {
        if on {
            self.render_state |= ZBUFFER_WRITE_MASK;
        } else {
            self.render_state &= !ZBUFFER_WRITE_MASK;
        }
        self.render_delta_mask |= ZBUFFER_WRITE_MASK;
    }
    pub fn set_z_compare(&mut self, on: bool) {
        if on {
            self.render_state |= ZBUFFER_COMPARE_MASK;
        } else {
            self.render_state &= !ZBUFFER_COMPARE_MASK;
        }
        self.render_delta_mask |= ZBUFFER_COMPARE_MASK;
    }

    /// The state `weaponbolt.cpp` and `terrobj.cpp` hand to `Effect::Draw`: dither, texture
    /// correction, z-compare and z-write on.
    pub fn game_draw_state() -> MlrState {
        let mut s = MlrState::default();
        s.render_state |= DITHER_MASK | TEXTURE_CORRECTION_MASK;
        s.render_delta_mask |= DITHER_MASK | TEXTURE_CORRECTION_MASK;
        s.set_z_compare(true);
        s.set_z_write(true);
        s
    }

    /// `MLRState::Combine(master, slave)`: the slave overrides the master only where its delta
    /// mask is set *and* the master permits it.
    pub fn combine(master: &MlrState, slave: &MlrState) -> MlrState {
        let render_permission_mask = master.render_permission_mask & slave.render_permission_mask;
        let render_delta_mask = slave.render_delta_mask & master.render_permission_mask;
        let render_state =
            (master.render_state & !render_delta_mask) | (slave.render_state & render_delta_mask);
        let process_permission_mask =
            master.process_permission_mask & slave.process_permission_mask;
        let process_delta_mask = slave.process_delta_mask & master.process_permission_mask;
        let process_state = (master.process_state & !process_delta_mask)
            | (slave.process_state & process_delta_mask);
        let texture = if render_delta_mask & TEXTURE_MASK != 0 {
            slave.texture.clone()
        } else {
            master.texture.clone()
        };
        MlrState {
            render_state,
            render_delta_mask,
            render_permission_mask,
            process_state,
            process_delta_mask,
            process_permission_mask,
            texture,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_layout_matches_the_header() {
        assert_eq!(TEXTURE_MASK, 0x3fff);
        assert_eq!(ALPHA_BIT, 14);
        assert_eq!(FILTER_BIT, 17);
        assert_eq!(FOG_BIT, 19);
        assert_eq!(TEXTURE_WRAP_BIT, 21);
        assert_eq!(DITHER_BIT, 22);
        assert_eq!(TEXTURE_CORRECTION_BIT, 23);
        assert_eq!(WIREFRAME_BIT, 24);
        assert_eq!(ZBUFFER_WRITE_BIT, 26);
        assert_eq!(ZBUFFER_COMPARE_BIT, 27);
        assert_eq!(SPECULAR_BIT, 28);
        assert_eq!(FLAT_COLORING_BIT, 29);
    }

    #[test]
    fn combine_respects_permission_and_delta() {
        let mut master = MlrState::game_draw_state();
        let mut slave = MlrState::default();
        slave.set_blend_bits(1); // additive
        slave.set_z_write(false);
        let c = MlrState::combine(&master, &slave);
        assert_eq!(c.blend(), BlendMode::OneOne);
        assert!(!c.z_write());
        assert!(c.z_compare());
        // a master that forbids z changes keeps its own z-write
        master.render_permission_mask &= !ZBUFFER_WRITE_MASK;
        let c = MlrState::combine(&master, &slave);
        assert!(c.z_write());
        assert_eq!(c.blend(), BlendMode::OneOne);
    }

    #[test]
    fn load_without_texture() {
        let words = [0u32, 0, 0xffff_ffff, 0, 0, 0xffff_ffff];
        let mut b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let st = MlrState::load(&mut Stream::new(&b), 9).unwrap();
        assert!(st.texture.is_none());
        assert_eq!(st.render_permission_mask, 0xffff_ffff);
        // with a texture: bit 0 set, then name + instance
        b[0] = 1;
        b.extend(3u32.to_le_bytes());
        b.extend(b"abc\0");
        b.extend(0x21i32.to_le_bytes());
        let st = MlrState::load(&mut Stream::new(&b), 9).unwrap();
        assert_eq!(
            st.texture,
            Some(TextureRef {
                name: "abc".into(),
                instance: 0x21
            })
        );
    }

    #[test]
    fn mlr_version_signature_is_optional() {
        let mut b = MLR_SIGNATURE.to_le_bytes().to_vec();
        b.extend(9i32.to_le_bytes());
        let mut s = Stream::new(&b);
        assert_eq!(read_mlr_version(&mut s).unwrap(), 9);
        assert_eq!(s.pos(), 8);
        let b = 5u32.to_le_bytes();
        let mut s = Stream::new(&b);
        assert_eq!(read_mlr_version(&mut s).unwrap(), -1);
        assert_eq!(s.pos(), 0);
    }
}

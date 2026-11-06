// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! gosFX library header reader (signature, version, effect count). The effects themselves are
//! parsed by the `mc2-fx` crate (`mc2_fx::library::EffectLibrary`), which follows each
//! polymorphic class payload exactly; this module only validates the header.
use crate::{AssetError, Reader};

pub const GFX_MAGIC: u32 = 0x4746_5823;
pub const CURRENT_GFX_VERSION: i32 = 17;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectLibraryHeader {
    pub version: i32,
    pub effect_count: u32,
}

pub fn decode_header(bytes: &[u8]) -> crate::reader::Result<EffectLibraryHeader> {
    let mut r = Reader::new(bytes);
    let magic = r.u32()?;
    if magic != GFX_MAGIC {
        return Err(AssetError(format!("invalid gosFX magic {magic:#010x}")));
    }
    let version = r.i32()?;
    if !(9..=CURRENT_GFX_VERSION).contains(&version) {
        return Err(AssetError(format!("unsupported gosFX version {version}")));
    }
    let effect_count = r.u32()?;
    if effect_count > 100_000 {
        return Err(AssetError(format!(
            "unreasonable effect count {effect_count}"
        )));
    }
    Ok(EffectLibraryHeader {
        version,
        effect_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_header() {
        let mut b = GFX_MAGIC.to_le_bytes().to_vec();
        b.extend(9i32.to_le_bytes());
        b.extend(2u32.to_le_bytes());
        assert_eq!(
            decode_header(&b).unwrap(),
            EffectLibraryHeader {
                version: 9,
                effect_count: 2
            }
        );
    }

    #[test]
    fn rejects_future_version() {
        let mut b = GFX_MAGIC.to_le_bytes().to_vec();
        b.extend((CURRENT_GFX_VERSION + 1).to_le_bytes());
        b.extend(0u32.to_le_bytes());
        assert!(decode_header(&b).is_err());
    }
}

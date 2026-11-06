// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! C-library-compatible text helpers shared by the FIT/INI and CSV readers.
//!
//! The game parses values with `atol`, `atof` and `strstr`, plus its own hex
//! branch (`0x...`). These helpers reproduce those semantics on byte slices.
//! Every helper first truncates its input at the first NUL byte, as the C
//! string functions do.

/// The C `isspace` set in the "C" locale.
pub(crate) fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Truncates a slice at its first NUL byte (C string view).
pub(crate) fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&b| b == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// C `strstr`: byte offset of the first occurrence of `needle` in `hay`.
pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// C `atol`: optional leading whitespace and sign, then decimal digits.
/// Overflow wraps (the C behaviour is undefined; wrapping is the closest
/// deterministic reading).
pub(crate) fn atol(s: &[u8]) -> i64 {
    let s = c_str(s);
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let mut value: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        value = value.wrapping_mul(10).wrapping_add(i64::from(s[i] - b'0'));
        i += 1;
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// C `atof`: leading whitespace, optional sign, decimal mantissa and exponent,
/// plus `inf`/`nan`. Returns 0.0 when no number is present.
pub(crate) fn atof(s: &[u8]) -> f64 {
    let s = c_str(s);
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let start = i;
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let rest = &s[i..];
    if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"inf") {
        return if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"nan") {
        return if negative { -f64::NAN } else { f64::NAN };
    }
    let mut digits = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    let mut end = i;
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            end = j;
        }
    }
    // The slice is ASCII by construction (sign, digits, '.', exponent).
    let text = std::str::from_utf8(&s[start..end]).unwrap_or("0");
    text.parse::<f64>().unwrap_or(0.0)
}

/// The hex branch of the game's `textToX` helpers.
///
/// Digits are read from just after the first `0x` up to the first character
/// that is not alphanumeric or is a letter past `F`; this matches the game's
/// in-place truncation.
pub(crate) fn hex_after_0x(s: &[u8]) -> u64 {
    let Some(pos) = find(s, b"0x") else {
        return 0;
    };
    let mut value: u64 = 0;
    for &c in &s[pos + 2..] {
        let upper = c.to_ascii_uppercase();
        if !c.is_ascii_alphanumeric() || (c.is_ascii_alphabetic() && upper > b'F') {
            break;
        }
        let digit = match upper {
            b'0'..=b'9' => upper - b'0',
            _ => upper - b'A' + 10,
        };
        value = value.wrapping_mul(16).wrapping_add(u64::from(digit));
    }
    value
}

/// Integer bits as the game reads them: hex when `0x` appears anywhere in the
/// value, otherwise `atol`. Callers cast to the target width, which mirrors the
/// C narrowing assignments.
pub(crate) fn int_bits(s: &[u8]) -> u64 {
    let s = c_str(s);
    if find(s, b"0x").is_some() {
        hex_after_0x(s)
    } else {
        atol(s) as u64
    }
}

/// `booleanToLong`: anything that starts with `F` or `0` is false (`N` is
/// also false in the CSV dialect); an empty value is true.
pub(crate) fn bool_value(s: &[u8], extra_false: &[u8]) -> bool {
    let s = c_str(s);
    let first = s
        .iter()
        .position(|&b| !is_c_space(b))
        .map(|i| s[i].to_ascii_uppercase());
    match first {
        Some(c) => !(c == b'F' || c == b'0' || extra_false.contains(&c)),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atol_matches_c_semantics() {
        assert_eq!(atol(b"  42abc"), 42);
        assert_eq!(atol(b"-7"), -7);
        assert_eq!(atol(b"+9"), 9);
        assert_eq!(atol(b"abc"), 0);
        assert_eq!(atol(b"12\0 34"), 12);
    }

    #[test]
    fn atof_matches_c_semantics() {
        assert_eq!(atof(b" 1.5e2x"), 150.0);
        assert_eq!(atof(b"-.25"), -0.25);
        assert_eq!(atof(b"3."), 3.0);
        assert_eq!(atof(b"1e"), 1.0);
        assert_eq!(atof(b"e5"), 0.0);
        assert!(atof(b"inf").is_infinite());
    }

    #[test]
    fn hex_branch_truncates_at_non_hex() {
        assert_eq!(int_bits(b"0x1F"), 31);
        assert_eq!(int_bits(b" 0xFFz junk"), 255);
        assert_eq!(int_bits(b"0x"), 0);
        assert_eq!(int_bits(b"0xg1"), 0);
        assert_eq!(int_bits(b"17 // see 0x10"), 16);
        assert_eq!(int_bits(b"-5"), (-5i64) as u64);
    }

    #[test]
    fn booleans() {
        assert!(!bool_value(b" false", b""));
        assert!(!bool_value(b"0", b""));
        assert!(bool_value(b"TRUE", b""));
        assert!(bool_value(b"", b""));
        assert!(!bool_value(b"n", b"N"));
    }
}

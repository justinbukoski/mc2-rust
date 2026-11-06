//! The per-frame state digest: the interchange format between the C++ oracle (`oracle/oracle_main.cpp`) and the Rust
//! simulation, plus a parser and a tolerant differ for differential tests. The format is documented in
//! `oracle/DIGEST.md`; this module only needs to know that a digest is a sequence of lines of whitespace separated
//! tokens, that a line starting with `F` begins a frame record, and that floats are printed like C's `%.9g` (which
//! round-trips an `f32` exactly).

use std::fmt::Write as _;

/// Format an `f32` exactly like the oracle's `F()` helper: C `printf("%.9g")`, with `-0` and `0` both printed as `0`.
pub fn fmt_f32(v: f32) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    fmt_g(v as f64, 9)
}

/// C `%.{prec}g` for a double.
pub fn fmt_g(v: f64, prec: usize) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    // d.ddddddde+XX with prec-1 decimals gives the correctly rounded decimal exponent.
    let sci = format!("{:.*e}", prec - 1, v);
    let (mant, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent value");
    if exp < -4 || exp >= prec as i32 {
        let mant = trim_zeros(mant);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{sign}{:02}", exp.abs())
    } else {
        let decimals = (prec as i32 - 1 - exp).max(0) as usize;
        trim_zeros(&format!("{:.*}", decimals, v))
    }
}

fn trim_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// One line: a tag (first token) and the remaining tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestLine {
    /// First token (`F`, `M`, `V`, `B`, `T`, `G`, `W`, `Z`, `VA`, `H`, ...).
    pub tag: String,
    /// The rest of the line, split on whitespace.
    pub tokens: Vec<String>,
}

impl DigestLine {
    /// Parse one line; `None` for a blank line.
    pub fn parse(line: &str) -> Option<DigestLine> {
        let mut it = line.split_whitespace();
        let tag = it.next()?.to_string();
        Some(DigestLine {
            tag,
            tokens: it.map(str::to_string).collect(),
        })
    }

    /// Render back to text.
    pub fn to_text(&self) -> String {
        let mut s = self.tag.clone();
        for t in &self.tokens {
            s.push(' ');
            s.push_str(t);
        }
        s
    }
}

/// Builds a digest line token by token.
#[derive(Clone, Debug)]
pub struct LineBuilder {
    line: String,
}

impl LineBuilder {
    /// Start a line with its tag.
    pub fn new(tag: &str) -> LineBuilder {
        LineBuilder {
            line: tag.to_string(),
        }
    }
    /// Append a float (`%.9g`).
    pub fn f(&mut self, v: f32) -> &mut Self {
        self.line.push(' ');
        self.line.push_str(&fmt_f32(v));
        self
    }
    /// Append an integer.
    pub fn i(&mut self, v: i64) -> &mut Self {
        let _ = write!(self.line, " {v}");
        self
    }
    /// Append an integer with a prefix glued on (`t0`, `c1`, `iv1`, `o16`...).
    pub fn pi(&mut self, prefix: &str, v: i64) -> &mut Self {
        let _ = write!(self.line, " {prefix}{v}");
        self
    }
    /// Append a raw token.
    pub fn s(&mut self, tok: &str) -> &mut Self {
        self.line.push(' ');
        self.line.push_str(tok);
        self
    }
    /// Append a hexadecimal value (no `0x`).
    pub fn x(&mut self, v: u64) -> &mut Self {
        let _ = write!(self.line, " {v:x}");
        self
    }
    /// Finish.
    pub fn build(&self) -> String {
        self.line.clone()
    }
}

/// One frame record: the `F` line and everything up to the next `F`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameDigest {
    /// Frame number from the `F` line (0 = right after mission load).
    pub frame: i64,
    /// All lines of the record, `F` line first.
    pub lines: Vec<DigestLine>,
}

/// A whole digest file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digest {
    /// Lines before the first frame (the `H` header).
    pub header: Vec<DigestLine>,
    /// Frame records in file order.
    pub frames: Vec<FrameDigest>,
}

impl Digest {
    /// Parse digest text.
    pub fn parse(text: &str) -> Result<Digest, String> {
        let mut d = Digest::default();
        for (n, raw) in text.lines().enumerate() {
            let Some(line) = DigestLine::parse(raw) else {
                continue;
            };
            if line.tag == "F" {
                let frame = line
                    .tokens
                    .first()
                    .and_then(|t| t.parse::<i64>().ok())
                    .ok_or_else(|| format!("line {}: bad F line", n + 1))?;
                d.frames.push(FrameDigest {
                    frame,
                    lines: vec![line],
                });
            } else if let Some(f) = d.frames.last_mut() {
                f.lines.push(line);
            } else {
                d.header.push(line);
            }
        }
        Ok(d)
    }

    /// Render back to text (round-trips [`Digest::parse`]).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for l in &self.header {
            s.push_str(&l.to_text());
            s.push('\n');
        }
        for f in &self.frames {
            for l in &f.lines {
                s.push_str(&l.to_text());
                s.push('\n');
            }
        }
        s
    }

    /// The frame record with this frame number.
    pub fn frame(&self, frame: i64) -> Option<&FrameDigest> {
        self.frames.iter().find(|f| f.frame == frame)
    }
}

/// How strictly float tokens are compared.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Allowed absolute difference for tokens that are floats (contain `.` or an exponent). 0 = exact.
    pub abs: f64,
    /// Allowed relative difference (applied in addition to `abs`).
    pub rel: f64,
}

impl Tolerance {
    /// Exact (as printed) comparison.
    pub const EXACT: Tolerance = Tolerance { abs: 0.0, rel: 0.0 };
    /// A small tolerance for work-in-progress ports (1e-3 absolute, 1e-6 relative).
    pub const LOOSE: Tolerance = Tolerance {
        abs: 1e-3,
        rel: 1e-6,
    };
}

/// One difference between two digests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// Frame number.
    pub frame: i64,
    /// Line index within the frame record.
    pub line: usize,
    /// Human readable description.
    pub what: String,
}

fn is_float_token(t: &str) -> bool {
    t.contains('.') || (t.contains('e') && t.parse::<f64>().is_ok())
}

fn tokens_equal(a: &str, b: &str, tol: Tolerance) -> bool {
    if a == b {
        return true;
    }
    if (tol.abs > 0.0 || tol.rel > 0.0) && (is_float_token(a) || is_float_token(b)) {
        if let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) {
            let d = (x - y).abs();
            return d <= tol.abs || d <= tol.rel * x.abs().max(y.abs());
        }
    }
    false
}

/// Compare two digests frame by frame (matched by frame number; only frames present in `expected` are checked, so a
/// Rust run may record more frames than the golden file). Returns at most `limit` mismatches.
pub fn diff(expected: &Digest, actual: &Digest, tol: Tolerance, limit: usize) -> Vec<Mismatch> {
    let mut out = Vec::new();
    // The header (`H mission fps seed rngMode`) is compared exactly: two runs with different seeds, RNG modes, rates or missions
    // are not comparable, however similar their first frames look.
    if expected.header.len() != actual.header.len() {
        out.push(Mismatch {
            frame: -1,
            line: 0,
            what: format!(
                "header line count differs: expected {} actual {}",
                expected.header.len(),
                actual.header.len()
            ),
        });
    }
    for (i, (eh, ah)) in expected.header.iter().zip(&actual.header).enumerate() {
        if eh != ah {
            out.push(Mismatch {
                frame: -1,
                line: i,
                what: format!(
                    "header: expected `{}` actual `{}`",
                    eh.to_text(),
                    ah.to_text()
                ),
            });
        }
    }
    for ef in &expected.frames {
        let Some(af) = actual.frame(ef.frame) else {
            out.push(Mismatch {
                frame: ef.frame,
                line: 0,
                what: "frame missing from actual".into(),
            });
            continue;
        };
        if ef.lines.len() != af.lines.len() {
            out.push(Mismatch {
                frame: ef.frame,
                line: 0,
                what: format!(
                    "line count differs: expected {} actual {}",
                    ef.lines.len(),
                    af.lines.len()
                ),
            });
        }
        for (i, (el, al)) in ef.lines.iter().zip(&af.lines).enumerate() {
            let bad = el.tag != al.tag
                || el.tokens.len() != al.tokens.len()
                || el
                    .tokens
                    .iter()
                    .zip(&al.tokens)
                    .any(|(a, b)| !tokens_equal(a, b, tol));
            if bad {
                out.push(Mismatch {
                    frame: ef.frame,
                    line: i,
                    what: format!("expected `{}` actual `{}`", el.to_text(), al.to_text()),
                });
            }
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::excessive_precision)]
mod tests {
    use super::*;

    #[test]
    fn g9_matches_c_printf() {
        // values taken from oracle digests
        assert_eq!(fmt_f32(2496.00024), "2496.00024");
        assert_eq!(fmt_f32(0.0333333351), "0.0333333351");
        assert_eq!(fmt_f32(-1941.33362), "-1941.33362");
        assert_eq!(fmt_f32(180.0), "180");
        assert_eq!(fmt_f32(-0.0), "0");
        assert_eq!(fmt_f32(1.0e-5), "9.99999975e-06");
        assert_eq!(fmt_f32(1.0e9), "1e+09");
        assert_eq!(fmt_f32(123456789.0), "123456792");
        assert_eq!(fmt_f32(0.001), "0.00100000005");
        assert_eq!(fmt_g(0.5, 9), "0.5");
    }

    #[test]
    fn parse_roundtrip_and_diff() {
        let text = "H mc2_01 30 1 0\nF 0 0 0 0.05 30 4db793af 262 484ad662\nM 512 2 1 2 3\nZ 0\nF 1 1 0 0.05 30 4db793af 262 484ad662\nM 512 2 1.5 2 3\nZ 0\n";
        let d = Digest::parse(text).unwrap();
        assert_eq!(d.header.len(), 1);
        assert_eq!(d.frames.len(), 2);
        assert_eq!(d.to_text(), text);
        let mut e = d.clone();
        e.frames[1].lines[1].tokens[2] = "1.5004".to_string();
        assert_eq!(diff(&d, &e, Tolerance::EXACT, 10).len(), 1);
        assert!(diff(&d, &e, Tolerance::LOOSE, 10).is_empty());
    }

    #[test]
    fn header_mismatch_is_reported() {
        let a =
            Digest::parse("H mc2_01 30 1 0\nF 0 0 0 0.05 30 4db793af 262 484ad662\nZ 0\n").unwrap();
        for other in [
            "H mc2_01 30 2 0",
            "H mc2_01 30 1 2",
            "H mc2_05 30 1 0",
            "H mc2_01 60 1 0",
        ] {
            let b = Digest::parse(&a.to_text().replace("H mc2_01 30 1 0", other)).unwrap();
            let d = diff(&a, &b, Tolerance::LOOSE, 10);
            assert_eq!(d.len(), 1, "{other}");
            assert_eq!(d[0].frame, -1);
        }
        let none = Digest::parse("F 0 0 0 0.05 30 4db793af 262 484ad662\nZ 0\n").unwrap();
        assert_eq!(diff(&a, &none, Tolerance::LOOSE, 10).len(), 1);
    }

    #[test]
    fn line_builder() {
        let mut b = LineBuilder::new("M");
        b.i(512).f(2496.0).pi("t", 0).x(0x7ac2b7b5);
        assert_eq!(b.build(), "M 512 2496 t0 7ac2b7b5");
    }
}

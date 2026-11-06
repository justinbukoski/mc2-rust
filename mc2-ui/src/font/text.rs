// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Text measuring and drawing with the semantics of the GameOS text calls the GUI uses
//! (`gos_TextSetAttributes`, `gos_TextSetRegion`, `gos_TextSetPosition`, `gos_TextStringLength`,
//! `gos_TextDraw`), written from their documentation in the public `GameOS/include/gameos.hpp`:
//!
//! * `WrapType` 0 left, 1 right, 2 centred, 3 centred in the region in X and Y;
//! * `WordWrap` wraps words at the region's right edge, otherwise lines are clipped;
//! * `\n` returns to the region's left edge one line down;
//! * embedded codes `/color=AARRGGBB`, `/bold=?`, `/proportional=?`, `/italic=?`, and `//` for a
//!   slash, unless codes are disabled;
//! * `gos_TextStringLength` gives the widest line and `line height * lines` (no wrapping);
//! * drawing is clipped to the region; bold draws the text twice, one pixel apart.
//!
//! Every glyph is placed at a whole UI pixel, so text stays crisp when the UI is scaled.

use super::Font;
use crate::cp1252;
use crate::draw::{Blend, DrawList, GuiRect};

/// `gos_TextSetAttributes` state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextAttrs {
    pub color: u32,
    /// `> 0`: scale in multiples of the font size; `<= 0` is drawn at the native size.
    pub size: f32,
    pub word_wrap: bool,
    pub proportional: bool,
    pub bold: bool,
    pub italic: bool,
    /// `WrapType`: 0 left, 1 right, 2 centred, 3 centred X and Y.
    pub align: u32,
    pub disable_codes: bool,
}

impl Default for TextAttrs {
    fn default() -> Self {
        TextAttrs {
            color: 0xffff_ffff,
            size: 1.0,
            word_wrap: true,
            proportional: true,
            bold: false,
            italic: false,
            align: 0,
            disable_codes: false,
        }
    }
}

/// One character to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    byte: u8,
    color: u32,
    bold: bool,
}

/// Parses embedded codes into styled characters plus line breaks (`None`).
fn spans(text: &str, attrs: &TextAttrs) -> Vec<Option<Span>> {
    let mut out = Vec::with_capacity(text.len());
    let mut color = attrs.color;
    let mut bold = attrs.bold;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let len = c.len_utf8();
        if c == '\n' {
            out.push(None);
            rest = &rest[len..];
            continue;
        }
        if c == '/' && !attrs.disable_codes {
            let after = &rest[1..];
            if let Some(stripped) = after.strip_prefix('/') {
                out.push(Some(Span {
                    byte: b'/',
                    color,
                    bold,
                }));
                rest = stripped;
                continue;
            }
            if let Some(hex) = after.strip_prefix("color=") {
                let digits: String = hex.chars().take(8).collect();
                if digits.len() == 8 {
                    if let Ok(v) = u32::from_str_radix(&digits, 16) {
                        color = v;
                        rest = &hex[8..];
                        continue;
                    }
                }
            }
            let mut matched = false;
            for key in ["bold=", "proportional=", "italic="] {
                if let Some(v) = after.strip_prefix(key) {
                    if let Some(flag) = v.chars().next() {
                        if key == "bold=" {
                            bold = flag == '1';
                        }
                        rest = &v[flag.len_utf8()..];
                        matched = true;
                        break;
                    }
                }
            }
            if matched {
                continue;
            }
        }
        if c != '\r' {
            out.push(Some(Span {
                byte: cp1252::char_to_byte(c).unwrap_or(b'?'),
                color,
                bold,
            }));
        }
        rest = &rest[len..];
    }
    out
}

fn scale_of(attrs: &TextAttrs) -> i32 {
    if attrs.size > 1.0 {
        attrs.size.round() as i32
    } else {
        1
    }
}

/// `gos_TextStringLength`: `(widest line, line height * line count)`.
pub fn string_size(font: &Font, text: &str, attrs: &TextAttrs) -> (u32, u32) {
    let s = scale_of(attrs);
    let mut widest = 0i32;
    let mut cur = 0i32;
    let mut lines = 1u32;
    for sp in spans(text, attrs) {
        match sp {
            None => {
                widest = widest.max(cur);
                cur = 0;
                lines += 1;
            }
            Some(sp) => cur += i32::from(font.glyph(sp.byte).advance) * s,
        }
    }
    widest = widest.max(cur);
    (widest.max(0) as u32, font.line_height * lines * s as u32)
}

/// One laid-out line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Pen x of the first character.
    pub x: i32,
    /// Top of the line.
    pub y: i32,
    /// Width without trailing spaces.
    pub width: i32,
    spans: Vec<Span>,
}

impl Line {
    pub fn text(&self) -> String {
        self.spans
            .iter()
            .map(|s| cp1252::byte_to_char(s.byte))
            .collect()
    }
}

fn width_of(font: &Font, s: &[Span], scale: i32) -> i32 {
    s.iter()
        .map(|sp| i32::from(font.glyph(sp.byte).advance) * scale)
        .sum()
}

fn trimmed_width(font: &Font, s: &[Span], scale: i32) -> i32 {
    let end = s
        .iter()
        .rposition(|sp| sp.byte != b' ')
        .map_or(0, |i| i + 1);
    width_of(font, &s[..end], scale)
}

/// Breaks text into lines for `region` and computes where each line starts.
pub fn layout(
    font: &Font,
    text: &str,
    attrs: &TextAttrs,
    region: GuiRect,
    pos: (i32, i32),
) -> Vec<Line> {
    let scale = scale_of(attrs);
    let lh = font.line_height as i32 * scale;
    // hard lines first
    let mut hard: Vec<Vec<Span>> = vec![Vec::new()];
    for sp in spans(text, attrs) {
        match sp {
            None => hard.push(Vec::new()),
            Some(s) => hard.last_mut().expect("one line").push(s),
        }
    }
    let mut lines: Vec<Vec<Span>> = Vec::new();
    for h in hard {
        if !attrs.word_wrap {
            lines.push(h);
            continue;
        }
        let mut cur: Vec<Span> = Vec::new();
        let mut i = 0;
        while i < h.len() {
            // next word: non-spaces then its trailing spaces
            let start = i;
            while i < h.len() && h[i].byte != b' ' {
                i += 1;
            }
            while i < h.len() && h[i].byte == b' ' {
                i += 1;
            }
            let word = &h[start..i];
            let first_line = lines.is_empty();
            let avail = if first_line && attrs.align == 0 {
                region.right - pos.0
            } else {
                region.width()
            };
            let mut candidate = cur.clone();
            candidate.extend_from_slice(word);
            if trimmed_width(font, &candidate, scale) <= avail || cur.is_empty() {
                if cur.is_empty() && attrs.align == 1 && trimmed_width(font, word, scale) > avail {
                    // a right aligned word that is a hair too wide (a five digit price
                    // in a four digit column) overhangs its region instead of losing its tail
                    cur = candidate;
                } else if cur.is_empty() && trimmed_width(font, word, scale) > avail {
                    // a word wider than the region breaks between characters
                    for &sp in word {
                        let mut c2 = cur.clone();
                        c2.push(sp);
                        if !cur.is_empty() && trimmed_width(font, &c2, scale) > avail {
                            lines.push(std::mem::take(&mut cur));
                        }
                        cur.push(sp);
                    }
                } else {
                    cur = candidate;
                }
            } else {
                lines.push(std::mem::take(&mut cur));
                // re-run this word on the fresh line
                i = start;
                continue;
            }
        }
        lines.push(cur);
    }
    let total_h = lh * lines.len() as i32;
    let mut y = match attrs.align {
        3 => region.top + (region.height() - total_h) / 2,
        _ => pos.1,
    };
    let mut out = Vec::with_capacity(lines.len());
    for (n, l) in lines.into_iter().enumerate() {
        let w = trimmed_width(font, &l, scale);
        let x = match attrs.align {
            1 => region.right - w,
            2 | 3 => region.left + (region.width() - w) / 2,
            _ if n == 0 => pos.0,
            _ => region.left,
        };
        out.push(Line {
            x,
            y,
            width: w,
            spans: l,
        });
        y += lh;
    }
    out
}

/// `gos_TextDraw`: lays out and records the glyph quads, clipped to `region`.
pub fn draw(
    dl: &mut DrawList,
    font: &Font,
    text: &str,
    attrs: &TextAttrs,
    region: GuiRect,
    pos: (i32, i32),
) {
    let Some(tex) = font.texture else { return };
    let scale = scale_of(attrs);
    let (aw, ah) = (font.atlas_width as f32, font.atlas_height as f32);
    let lines = layout(font, text, attrs, region, pos);
    // a right aligned word that overhangs its region (see `layout`) stays visible
    let mut clip = region;
    if attrs.align == 1 {
        for l in &lines {
            clip.left = clip.left.min(l.x);
        }
    }
    dl.push_clip(clip);
    for line in lines {
        let mut pen = line.x;
        for sp in &line.spans {
            let g = font.glyph(sp.byte);
            if g.present && g.w > 0 && g.h > 0 {
                let x0 = (pen + i32::from(g.dx) * scale) as f32;
                let y0 = (line.y + i32::from(g.dy) * scale) as f32;
                let x1 = x0 + f32::from(g.w) * scale as f32;
                let y1 = y0 + f32::from(g.h) * scale as f32;
                let uv = (
                    f32::from(g.x) / aw,
                    f32::from(g.y) / ah,
                    f32::from(g.x + g.w) / aw,
                    f32::from(g.y + g.h) / ah,
                );
                let passes = if sp.bold { 2 } else { 1 };
                for p in 0..passes {
                    let off = p as f32;
                    dl.rect_quad(
                        Some(tex),
                        Blend::Alpha,
                        (x0 + off, y0, x1 + off, y1),
                        uv,
                        sp.color,
                    );
                }
            }
            pen += i32::from(g.advance) * scale;
        }
    }
    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{FontFormat, Glyph};

    /// A fixed font: every printable character 6 wide, space 3, line height 10.
    pub(crate) fn test_font() -> Font {
        let mut glyphs = vec![Glyph::default(); 256];
        for (c, g) in glyphs.iter_mut().enumerate().skip(33) {
            *g = Glyph {
                present: true,
                x: 0,
                y: 0,
                w: 5,
                h: 10,
                dx: 0,
                dy: 0,
                advance: if c == b'i' as usize { 2 } else { 6 },
            };
        }
        glyphs[32].advance = 3;
        Font {
            name: "test".into(),
            format: FontFormat::GlyphBmp,
            line_height: 10,
            ascent: 8,
            glyphs,
            atlas_width: 8,
            atlas_height: 16,
            atlas: vec![255; 8 * 16 * 4],
            texture: Some(crate::textures::TexId(0)),
        }
    }

    #[test]
    fn string_length_counts_lines_without_wrapping() {
        let f = test_font();
        let a = TextAttrs::default();
        assert_eq!(string_size(&f, "ABC", &a), (18, 10));
        assert_eq!(string_size(&f, "AB\nABCD i", &a), (29, 20));
        assert_eq!(string_size(&f, "a//b/color=ff00ff00c", &a), (24, 10));
        let mut d = a;
        d.disable_codes = true;
        assert_eq!(string_size(&f, "a//b", &d).0, 24);
    }

    #[test]
    fn a_right_aligned_word_too_wide_overhangs_instead_of_breaking() {
        // a five digit price (30 wide) in a 24 wide column keeps its last digit
        let f = test_font();
        let a = TextAttrs {
            align: 1,
            ..TextAttrs::default()
        };
        let r = GuiRect::new(100, 50, 124, 60);
        let l = layout(&f, "56550", &a, r, (100, 50));
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].text(), l[0].x), ("56550".to_string(), 124 - 30));
        let mut dl = DrawList::new(800, 600);
        draw(&mut dl, &f, "56550", &a, r, (100, 50));
        assert!(
            dl.batches[0].clip.expect("clip").left <= 94,
            "not clipped to the column"
        );
    }

    #[test]
    fn wraps_words_and_aligns() {
        let f = test_font();
        let mut a = TextAttrs {
            align: 0,
            ..TextAttrs::default()
        };
        let r = GuiRect::new(100, 50, 150, 200);
        // "AAA BBB CCC": words 18 wide + space 3; region 50 wide fits two words
        let lines = layout(&f, "AAA BBB CCC", &a, r, (100, 50));
        let texts: Vec<_> = lines.iter().map(|l| l.text()).collect();
        assert_eq!(texts, ["AAA BBB ", "CCC"]);
        assert_eq!((lines[1].x, lines[1].y, lines[0].width), (100, 60, 39));
        a.align = 1;
        let l = layout(&f, "AAA", &a, r, (100, 50));
        assert_eq!(l[0].x, 150 - 18);
        a.align = 2;
        assert_eq!(
            layout(&f, "AAA", &a, r, (100, 50))[0].x,
            100 + (50 - 18) / 2
        );
        a.align = 3;
        let l = layout(&f, "AAA", &a, r, (100, 50));
        assert_eq!(l[0].y, 50 + (150 - 10) / 2);
        // a word wider than the region breaks by characters
        a.align = 0;
        let l = layout(&f, "ABCDEFGHIJ", &a, GuiRect::new(0, 0, 30, 100), (0, 0));
        assert_eq!(
            l.iter().map(|l| l.text()).collect::<Vec<_>>(),
            ["ABCDE", "FGHIJ"]
        );
        // no wrap: one line, clipped by the region when drawn
        a.word_wrap = false;
        assert_eq!(layout(&f, "AAA BBB CCC", &a, r, (100, 50)).len(), 1);
    }

    #[test]
    fn draw_records_clipped_glyphs() {
        let f = test_font();
        let mut dl = DrawList::new(800, 600);
        let a = TextAttrs {
            color: 0xffc6_6600,
            bold: true,
            ..TextAttrs::default()
        };
        draw(
            &mut dl,
            &f,
            "A /color=ff00ff00B",
            &a,
            GuiRect::new(10, 10, 100, 30),
            (10, 10),
        );
        // two glyphs, bold doubles them
        assert_eq!(dl.quad_count(), 4);
        assert_eq!(dl.vertices[0].x, 10.0);
        assert_eq!(dl.vertices[4].x, 11.0);
        assert_eq!(dl.vertices[8].x, 19.0);
        assert_eq!(dl.vertices[8].argb, 0xff00_ff00);
        assert_eq!(dl.batches[0].clip, Some(GuiRect::new(10, 10, 100, 30)));
    }
}

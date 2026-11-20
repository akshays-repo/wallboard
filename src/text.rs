//! Text: fonts, glyph cache, measuring, ellipsis, glyph blitting. No shaping (Latin only).
//!
//! Two faces (Regular, Bold) are embedded. fontdue parses every glyph outline at load, so
//! memory grows with glyph count; the embedded Inter files are subset to Latin (394 glyphs).
//! Glyph sizes are quantised to a small ladder so the cache stays small while zooming.

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;

/// Reference size used to measure advances (they scale linearly from here).
const REF: f32 = 64.0;
/// Below this many pixels text is drawn as bars instead of glyphs.
pub const MIN_GLYPH_PX: f32 = 5.0;
/// Glyph cache is cleared when its bitmaps exceed this many bytes.
const CACHE_LIMIT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Regular,
    Bold,
}

/// The type scale from the design spec (sizes in world units / logical px).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    HeroTitle, // 20 bold
    Title,     // 17 bold
    Row,       // 15 regular
    Chip,      // 13 regular
    ChipBold,  // 13 bold
    Hint,      // 11 regular
    Free,      // 20 regular, free text on the canvas
}

impl Style {
    pub fn face(self) -> Face {
        match self {
            Style::HeroTitle | Style::Title | Style::ChipBold => Face::Bold,
            _ => Face::Regular,
        }
    }
    pub fn size(self) -> f32 {
        match self {
            Style::HeroTitle => 20.0,
            Style::Title => 17.0,
            Style::Row => 15.0,
            Style::Chip | Style::ChipBold => 13.0,
            Style::Hint => 11.0,
            Style::Free => 20.0,
        }
    }
}

pub struct Glyph {
    pub xmin: i32,
    pub ymin: i32,
    pub w: usize,
    pub h: usize,
    pub cov: Vec<u8>,
}

pub struct Fonts {
    /// Images and stickers (kept here so every draw already has it).
    pub pics: crate::pics::Pics,
    regular: fontdue::Font,
    bold: fontdue::Font,
    adv: RefCell<HashMap<(Face, char), f32>>,
    glyphs: HashMap<(Face, char, u16), Glyph>,
    glyph_bytes: usize,
    /// (ascent, descent magnitude) at `REF` px, per face.
    metrics: [(f32, f32); 2],
}

/// Embedded default fonts: Inter (SIL OFL 1.1, see assets/LICENSE-Inter.txt), subset with
/// `hb-subset` to Basic Latin, Latin-1, Latin Extended-A and common punctuation.
static INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular-Latin.ttf");
static INTER_BOLD: &[u8] = include_bytes!("../assets/Inter-Bold-Latin.ttf");

fn parse(bytes: &[u8], what: &str) -> Result<fontdue::Font, String> {
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .map_err(|e| format!("parse {what}: {e}"))
}

/// `WALLBOARD_FONT=/path/font.ttf` uses that file for both faces, `WALLBOARD_FONT=system`
/// asks fontconfig for `sans`; anything else uses the embedded Inter pair.
fn font_sources() -> Result<(Vec<u8>, Vec<u8>, String), String> {
    let path = match std::env::var("WALLBOARD_FONT") {
        Ok(v) if v == "system" => {
            let out = Command::new("fc-match")
                .args(["-f", "%{file}", "sans"])
                .output()
                .map_err(|e| format!("fc-match: {e}"))?;
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(v) if !v.is_empty() => v,
        _ => {
            return Ok((
                INTER_REGULAR.to_vec(),
                INTER_BOLD.to_vec(),
                "embedded Inter Regular+Bold, Latin subset (OFL-1.1)".to_string(),
            ))
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
    Ok((bytes.clone(), bytes, path))
}

impl Fonts {
    pub fn load() -> Result<(Fonts, String), String> {
        let (r, b, label) = font_sources()?;
        let regular = parse(&r, &label)?;
        let bold = if r == b {
            parse(&r, &label)?
        } else {
            parse(&b, &label)?
        };
        let m = |f: &fontdue::Font| -> Result<(f32, f32), String> {
            let lm = f
                .horizontal_line_metrics(REF)
                .ok_or("font has no line metrics")?;
            Ok((lm.ascent, -lm.descent))
        };
        let metrics = [m(&regular)?, m(&bold)?];
        Ok((
            Fonts {
                pics: Default::default(),
                regular,
                bold,
                adv: RefCell::new(HashMap::new()),
                glyphs: HashMap::new(),
                glyph_bytes: 0,
                metrics,
            },
            label,
        ))
    }

    fn font(&self, face: Face) -> &fontdue::Font {
        match face {
            Face::Regular => &self.regular,
            Face::Bold => &self.bold,
        }
    }

    /// Advance width of one char for a font of `size` (any unit; the result is in that unit).
    pub fn advance(&self, face: Face, c: char, size: f32) -> f32 {
        let mut cache = self.adv.borrow_mut();
        let a = *cache
            .entry((face, c))
            .or_insert_with(|| self.font(face).metrics(c, REF).advance_width);
        a * size / REF
    }

    pub fn text_width(&self, face: Face, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.advance(face, c, size)).sum()
    }

    /// (ascent, descent magnitude) for `size`, same unit as `size`.
    pub fn ascent_descent(&self, face: Face, size: f32) -> (f32, f32) {
        let (a, d) = self.metrics[face as usize];
        (a * size / REF, d * size / REF)
    }

    /// Cached glyph at a quantised pixel size. The cache is cleared when it grows too big.
    pub fn glyph(&mut self, face: Face, c: char, px: u16) -> &Glyph {
        if self.glyph_bytes > CACHE_LIMIT_BYTES {
            self.glyphs.clear();
            self.glyph_bytes = 0;
        }
        let key = (face, c, px);
        if !self.glyphs.contains_key(&key) {
            let (m, cov) = self.font(face).rasterize(c, px as f32);
            self.glyph_bytes += cov.len() + std::mem::size_of::<Glyph>();
            self.glyphs.insert(
                key,
                Glyph {
                    xmin: m.xmin,
                    ymin: m.ymin,
                    w: m.width,
                    h: m.height,
                    cov,
                },
            );
        }
        &self.glyphs[&key]
    }

    pub fn cached_glyphs(&self) -> usize {
        self.glyphs.len()
    }

    /// `s` if it fits in `max_w`, else the longest prefix plus an ellipsis that fits.
    pub fn fit_ellipsis(&self, face: Face, s: &str, size: f32, max_w: f32) -> String {
        if self.text_width(face, s, size) <= max_w {
            return s.to_string();
        }
        let ell = self.advance(face, '…', size);
        let mut out = String::new();
        let mut w = 0.0;
        for c in s.chars() {
            let a = self.advance(face, c, size);
            if w + a + ell > max_w {
                break;
            }
            w += a;
            out.push(c);
        }
        out.push('…');
        out
    }
}

/// Snap a pixel size to a ladder: every integer up to 24, then steps of 4, then 8.
pub fn quantize_px(px: f32) -> u16 {
    let p = px.clamp(1.0, 192.0);
    let q = if p <= 24.0 {
        p.round()
    } else if p <= 64.0 {
        (p / 4.0).round() * 4.0
    } else {
        (p / 8.0).round() * 8.0
    };
    q as u16
}

/// A wrapped line: char range `[start, end)`, newline excluded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
}

/// Greedy word wrap. `adv` gives each char's width in the same unit as `max_w`.
/// Always returns at least one line; a trailing newline yields a final empty line.
pub fn wrap(chars: &[char], max_w: f32, adv: &dyn Fn(char) -> f32) -> Vec<Line> {
    let n = chars.len();
    let mut lines = Vec::new();
    let mut line_start = 0;
    let mut x = 0.0;
    let mut last_space: Option<usize> = None;
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '\n' {
            lines.push(Line {
                start: line_start,
                end: i,
            });
            i += 1;
            line_start = i;
            x = 0.0;
            last_space = None;
            continue;
        }
        let w = adv(c);
        if x + w > max_w && i > line_start {
            let brk = match last_space {
                Some(b) if b > line_start => b,
                _ => i,
            };
            lines.push(Line {
                start: line_start,
                end: brk,
            });
            line_start = brk;
            x = chars[brk..i].iter().map(|&c| adv(c)).sum();
            last_space = None;
            continue; // retry the same char on the new line
        }
        x += w;
        if c == ' ' {
            last_space = Some(i + 1);
        }
        i += 1;
    }
    lines.push(Line {
        start: line_start,
        end: n,
    });
    lines
}

/// Index of the line containing caret position `idx` (the last line starting at or before it).
pub fn line_of(lines: &[Line], idx: usize) -> usize {
    lines.iter().rposition(|l| l.start <= idx).unwrap_or(0)
}

/// Char index in `s` whose left edge is nearest to `x` (same unit as `adv`).
pub fn index_at_x(s: &str, x: f32, adv: &dyn Fn(char) -> f32) -> usize {
    let mut acc = 0.0;
    for (i, c) in s.chars().enumerate() {
        let w = adv(c);
        if x < acc + w / 2.0 {
            return i;
        }
        acc += w;
    }
    s.chars().count()
}

/// Alpha-blend a glyph's coverage onto an Xrgb8888 canvas (bytes B, G, R, X).
/// (x, y) is the glyph bitmap's top-left in buffer pixels. Pixels outside the canvas or
/// outside the horizontal clip range `[clip.0, clip.1)` are skipped.
pub fn blit(
    canvas: &mut [u8],
    w: i32,
    h: i32,
    g: &Glyph,
    x: i32,
    y: i32,
    rgb: (u8, u8, u8),
    clip: (i32, i32),
) {
    for row in 0..g.h as i32 {
        let py = y + row;
        if py < 0 || py >= h {
            continue;
        }
        for col in 0..g.w as i32 {
            let px = x + col;
            if px < 0 || px >= w || px < clip.0 || px >= clip.1 {
                continue;
            }
            let cov = g.cov[row as usize * g.w + col as usize] as u32;
            if cov == 0 {
                continue;
            }
            let i = ((py * w + px) * 4) as usize;
            let mix = |dst: u8, src: u8| -> u8 {
                ((dst as u32 * (255 - cov) + src as u32 * cov + 127) / 255) as u8
            };
            canvas[i] = mix(canvas[i], rgb.2);
            canvas[i + 1] = mix(canvas[i + 1], rgb.1);
            canvas[i + 2] = mix(canvas[i + 2], rgb.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Fonts {
        Fonts::load().expect("embedded fonts load").0
    }

    #[test]
    fn quantize_ladder() {
        assert_eq!(quantize_px(0.2), 1);
        assert_eq!(quantize_px(17.4), 17);
        assert_eq!(quantize_px(30.0), 32);
        assert_eq!(quantize_px(99.0), 96);
        assert_eq!(quantize_px(9999.0), 192);
        let mut set = std::collections::HashSet::new();
        for i in 0..2000 {
            set.insert(quantize_px(1.8 + i as f32 * 0.07));
        }
        assert!(set.len() < 60, "{}", set.len());
    }

    #[test]
    fn blit_clips_canvas_and_range() {
        let g = Glyph {
            xmin: 0,
            ymin: 0,
            w: 2,
            h: 2,
            cov: vec![255, 0, 128, 255],
        };
        let mut cv = vec![0u8; 4 * 4 * 4];
        blit(&mut cv, 4, 4, &g, 3, 3, (200, 100, 50), (0, 4));
        blit(&mut cv, 4, 4, &g, -1, -1, (200, 100, 50), (0, 4));
        assert_eq!(&cv[0..3], &[50, 100, 200]);
        // clip range excludes everything: nothing changes
        let mut cv2 = vec![0u8; 4 * 4 * 4];
        blit(&mut cv2, 4, 4, &g, 0, 0, (200, 100, 50), (10, 12));
        assert!(cv2.iter().all(|&b| b == 0));
    }

    #[test]
    fn both_faces_rasterise_real_glyphs() {
        let mut f = fonts();
        for face in [Face::Regular, Face::Bold] {
            for c in ['H', 'g', 'é', '!', 'Ž', '…', '✓'] {
                let g = f.glyph(face, c, 18);
                assert!(g.w > 0 && g.h > 0, "{face:?} {c:?} has an empty outline");
                assert!(
                    g.cov.iter().any(|&v| v > 0),
                    "{face:?} {c:?} rendered blank"
                );
            }
        }
    }

    #[test]
    fn bold_is_wider_than_regular() {
        let f = fonts();
        let r = f.text_width(Face::Regular, "Groceries and more", 17.0);
        let b = f.text_width(Face::Bold, "Groceries and more", 17.0);
        assert!(b > r, "bold {b} vs regular {r}");
    }

    #[test]
    fn advance_scales_linearly() {
        let f = fonts();
        let a = f.advance(Face::Regular, 'm', 10.0);
        let b = f.advance(Face::Regular, 'm', 20.0);
        assert!((b - 2.0 * a).abs() < 1e-3);
    }

    #[test]
    fn ellipsis_fits_and_marks_truncation() {
        let f = fonts();
        let s = "A fairly long todo that will not fit";
        assert_eq!(f.fit_ellipsis(Face::Regular, "short", 15.0, 500.0), "short");
        let t = f.fit_ellipsis(Face::Regular, s, 15.0, 120.0);
        assert!(t.ends_with('…') && t.chars().count() < s.chars().count());
        assert!(f.text_width(Face::Regular, &t, 15.0) <= 120.0 + 1e-3);
    }

    #[test]
    fn index_at_x_picks_nearest_edge() {
        let adv = |_: char| 10.0;
        assert_eq!(index_at_x("abcd", 0.0, &adv), 0);
        assert_eq!(index_at_x("abcd", 14.0, &adv), 1);
        assert_eq!(index_at_x("abcd", 16.0, &adv), 2);
        assert_eq!(index_at_x("abcd", 999.0, &adv), 4);
    }

    #[test]
    fn glyph_cache_is_capped() {
        let mut f = fonts();
        for px in 8..120u16 {
            for c in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
                f.glyph(Face::Regular, c, px);
            }
        }
        assert!(f.cached_glyphs() < 5000, "{}", f.cached_glyphs());
        assert!(f.glyph_bytes <= CACHE_LIMIT_BYTES + 64 * 1024);
    }

    #[test]
    fn style_scale_matches_spec() {
        assert_eq!(Style::HeroTitle.size(), 20.0);
        assert_eq!(Style::Title.size(), 17.0);
        assert_eq!(Style::Row.size(), 15.0);
        assert_eq!(Style::Chip.size(), 13.0);
        assert_eq!(Style::Hint.size(), 11.0);
        assert_eq!(Style::Title.face(), Face::Bold);
        assert_eq!(Style::Row.face(), Face::Regular);
    }

    fn texts(s: &str, max_w: f32) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        wrap(&c, max_w, &|_| 10.0)
            .iter()
            .map(|l| c[l.start..l.end].iter().collect())
            .collect()
    }

    #[test]
    fn wrap_basics() {
        assert_eq!(texts("", 100.0), vec![""]);
        assert_eq!(texts("hello", 100.0), vec!["hello"]);
        assert_eq!(texts("one two three", 80.0), vec!["one two ", "three"]);
        assert_eq!(texts("abcdefghij", 30.0), vec!["abc", "def", "ghi", "j"]);
        assert_eq!(texts("a\nb", 100.0), vec!["a", "b"]);
        assert_eq!(texts("a\n", 100.0), vec!["a", ""]);
        assert_eq!(texts("\n\n", 100.0), vec!["", "", ""]);
    }

    #[test]
    fn caret_line_mapping() {
        let c: Vec<char> = "abcdef".chars().collect();
        let lines = wrap(&c, 30.0, &|_| 10.0);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_of(&lines, 2), 0);
        assert_eq!(
            line_of(&lines, 3),
            1,
            "wrap boundary belongs to the next line"
        );
        assert_eq!(line_of(&lines, 6), 1);
    }
}

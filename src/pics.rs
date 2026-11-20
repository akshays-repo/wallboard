//! Pictures on the board: PNG images (kept in the data folder's `images/`) and emoji stickers.
//!
//! Memory: images are decoded on demand, shrunk to `MAX_SIDE`, and the cache is dropped when
//! the board goes idle. Emoji are rasterised as one-colour glyphs from a system emoji font;
//! the font is loaded only to fill a cache miss and is dropped again straight away.

use crate::text::Glyph;
use std::collections::HashMap;
use std::path::PathBuf;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// Longest side kept in memory for a decoded image.
const MAX_SIDE: u32 = 1024;
/// Decoded images kept at once.
const MAX_IMAGES: usize = 8;
const MAX_EMOJI_GLYPHS: usize = 64;
/// Largest PNG accepted (bytes).
pub const MAX_PNG_BYTES: usize = 20 * 1024 * 1024;

pub fn images_dir() -> PathBuf {
    crate::store::data_dir().join("images")
}

/// Validate PNG bytes, store them under a content-derived name, and return (name, width, height).
pub fn import_png(bytes: &[u8]) -> Result<(String, u32, u32), String> {
    use std::hash::{Hash, Hasher};
    if bytes.len() > MAX_PNG_BYTES {
        return Err("image is larger than 20 MB".into());
    }
    let pm = Pixmap::decode_png(bytes).map_err(|e| format!("not a PNG image: {e}"))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let name = format!("{:016x}.png", h.finish());
    let dir = images_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(&name);
    if !path.exists() {
        std::fs::write(&path, bytes)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok((name, pm.width(), pm.height()))
}

const EMOJI_FONTS: [&str; 5] = [
    "/usr/share/fonts/google-noto-emoji-fonts/NotoEmoji-Regular.ttf",
    "/usr/share/fonts/noto/NotoEmoji-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoEmoji-Regular.ttf",
    "/usr/share/fonts/NotoEmoji-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoEmoji-Regular.ttf",
];

fn load_emoji_font() -> Option<fontdue::Font> {
    let mut paths: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("WALLBOARD_EMOJI_FONT") {
        paths.push(p);
    }
    paths.extend(EMOJI_FONTS.iter().map(|s| s.to_string()));
    paths.iter().find_map(|p| {
        let bytes = std::fs::read(p).ok()?;
        fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
    })
}

#[derive(Default)]
pub struct Pics {
    images: HashMap<String, Option<Pixmap>>,
    emoji: HashMap<(char, u16), Option<Glyph>>,
}

impl Pics {
    /// Drop decoded images (the emoji glyphs are tiny and stay).
    pub fn clear(&mut self) {
        self.images.clear();
    }

    /// The decoded image in canvas byte order (B, G, R, A), or None if it cannot be read.
    pub fn image(&mut self, name: &str) -> Option<&Pixmap> {
        if !self.images.contains_key(name) {
            if self.images.len() >= MAX_IMAGES {
                // Evict one entry, not all: images still on screen stay decoded.
                if let Some(k) = self.images.keys().next().cloned() {
                    self.images.remove(&k);
                }
            }
            let pm = decode(name);
            self.images.insert(name.to_string(), pm);
        }
        self.images[name].as_ref()
    }

    /// One-colour glyph of a sticker at a pixel size; None without an emoji font.
    pub fn emoji(&mut self, e: &str, px: u16) -> Option<&Glyph> {
        let ch = e.chars().next()?;
        let key = (ch, px);
        if !self.emoji.contains_key(&key) {
            if self.emoji.len() >= MAX_EMOJI_GLYPHS {
                self.emoji.clear();
            }
            let g = load_emoji_font().map(|f| {
                let (m, cov) = f.rasterize(ch, px as f32);
                Glyph {
                    xmin: m.xmin,
                    ymin: m.ymin,
                    w: m.width,
                    h: m.height,
                    cov,
                }
            });
            self.emoji.insert(key, g);
        }
        self.emoji[&key].as_ref().filter(|g| g.w > 0 && g.h > 0)
    }
}

fn decode(name: &str) -> Option<Pixmap> {
    let bytes = std::fs::read(images_dir().join(name)).ok()?;
    let src = Pixmap::decode_png(&bytes).ok()?;
    let (w, h) = (src.width(), src.height());
    let k = (MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
    let mut pm = if k < 1.0 {
        let (nw, nh) = (
            ((w as f32 * k) as u32).max(1),
            ((h as f32 * k) as u32).max(1),
        );
        let mut dst = Pixmap::new(nw, nh)?;
        let paint = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        dst.draw_pixmap(
            0,
            0,
            src.as_ref(),
            &paint,
            Transform::from_scale(k, k),
            None,
        );
        dst
    } else {
        src
    };
    // The canvas is B,G,R,A in memory; tiny-skia pixmaps are R,G,B,A.
    for px in pm.data_mut().chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sticker_has_a_glyph_when_an_emoji_font_exists() {
        if load_emoji_font().is_none() {
            return; // no emoji font here: stickers fall back to an outline box
        }
        let mut p = Pics::default();
        for e in crate::board::STICKERS {
            assert!(p.emoji(e, 48).is_some(), "no glyph for {e}");
        }
    }

    #[test]
    fn missing_image_is_none_and_import_rejects_garbage() {
        let mut p = Pics::default();
        assert!(p.image("nope.png").is_none());
        assert!(import_png(b"not a png").is_err());
    }
}

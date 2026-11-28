//! CPU rendering into a shm buffer with tiny-skia, in an Apple (Reminders / Freeform) style.
//!
//! The shm format is Xrgb8888 (little-endian bytes B, G, R, X) while tiny-skia writes bytes as
//! R, G, B, A. Colours are therefore given to tiny-skia with red and blue swapped (see `bgr`),
//! so the screen shows the intended colour.

use crate::board::{
    Area, Background, Board, Note, Shape, ShapeKind, Stroke, TextBox, Theme, MAX_HERO_OPEN,
};
use crate::camera::Camera;
use crate::editor::{lines_of, scroll_x, style_of, EditTarget, Editor};
use crate::layout::{
    area_geom, connector_path, handles, note_geom, radius, text_geom, AreaGeom, Hover, Overlay,
    Rect as LRect, Sel, Tool, ViewBtn, BODY_LINE_H, CIRCLE, NOTE_LINE_H, NOTE_RADIUS, NOTE_TITLE_H,
    PAD, ROW_H, TEXT_GAP,
};
use crate::layout::{FmtBtn, TextBar};
use crate::md::Block;
use crate::text::{blit, blit_slanted, line_of, quantize_px, Face, Fonts, Style, MIN_GLYPH_PX};
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut,
    PixmapPaint, Rect, Stroke as SkStroke, StrokeDash, Transform,
};
use tiny_skia::{GradientStop, Point, RadialGradient, SpreadMode};

type Rgb = (u8, u8, u8);

/// A theme's colours. Dark and light follow the Apple system palettes.
#[derive(Clone, Copy)]
pub struct Colors {
    pub canvas: Rgb,
    pub card: Rgb,
    pub hover: Rgb,
    pub sep: Rgb,
    pub text: Rgb,
    pub secondary: Rgb,
    pub tertiary: Rgb,
    /// The ten area colours: blue, green, orange, red, yellow, teal, purple, pink, indigo, gray.
    pub accent: [Rgb; 10],
    pub dark: bool,
    /// How cards cast shadows (a theme's material).
    pub shadow: Shadow,
    /// Card outline colour override (else the separator colour).
    pub edge: Option<Rgb>,
    /// Multipliers on the card outline width and corner radius.
    pub border_scale: f32,
    pub radius_scale: f32,
    pub grid: Grid,
    /// Three soft light colours drawn as an aurora behind everything.
    pub aurora: Option<[Rgb; 3]>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shadow {
    Soft,
    Hard,
    Glow,
    Flat,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Grid {
    Dots,
    Lines,
    None,
}

pub fn colors(theme: Theme) -> Colors {
    match theme {
        Theme::Dark => Colors {
            canvas: (0x14, 0x14, 0x16),
            card: (0x1c, 0x1c, 0x1e),
            hover: (0x2c, 0x2c, 0x2e),
            sep: (0x38, 0x38, 0x3a),
            text: (0xff, 0xff, 0xff),
            secondary: (0x98, 0x98, 0x9f),
            tertiary: (0x63, 0x63, 0x66),
            accent: [
                (0x0a, 0x84, 0xff),
                (0x30, 0xd1, 0x58),
                (0xff, 0x9f, 0x0a),
                (0xff, 0x45, 0x3a),
                (0xff, 0xd6, 0x0a),
                (0x40, 0xc8, 0xe0),
                (0xbf, 0x5a, 0xf2),
                (0xff, 0x37, 0x5f),
                (0x5e, 0x5c, 0xe6),
                (0x98, 0x98, 0x9d),
            ],
            dark: true,
            shadow: Shadow::Soft,
            edge: None,
            border_scale: 1.0,
            radius_scale: 1.0,
            grid: Grid::Dots,
            aurora: None,
        },
        Theme::Light => Colors {
            canvas: (0xf2, 0xf2, 0xf7),
            card: (0xff, 0xff, 0xff),
            hover: (0xf2, 0xf2, 0xf7),
            sep: (0xe5, 0xe5, 0xea),
            text: (0x00, 0x00, 0x00),
            secondary: (0x6c, 0x6c, 0x70),
            tertiary: (0xae, 0xae, 0xb2),
            accent: [
                (0x00, 0x7a, 0xff),
                (0x34, 0xc7, 0x59),
                (0xff, 0x95, 0x00),
                (0xff, 0x3b, 0x30),
                (0xff, 0xcc, 0x00),
                (0x30, 0xb0, 0xc7),
                (0xaf, 0x52, 0xde),
                (0xff, 0x2d, 0x55),
                (0x58, 0x56, 0xd6),
                (0x8e, 0x8e, 0x93),
            ],
            dark: false,
            shadow: Shadow::Soft,
            edge: None,
            border_scale: 1.0,
            radius_scale: 1.0,
            grid: Grid::Dots,
            aurora: None,
        },
        Theme::Aurora => Colors {
            canvas: (0x0a, 0x0e, 0x1f),
            card: (0x13, 0x19, 0x2e),
            hover: (0x1f, 0x28, 0x46),
            sep: (0x2b, 0x35, 0x5c),
            text: (0xf4, 0xf6, 0xff),
            secondary: (0xa6, 0xb2, 0xd6),
            tertiary: (0x66, 0x72, 0x9a),
            shadow: Shadow::Glow,
            edge: Some((0x3a, 0x48, 0x7a)),
            radius_scale: 1.25,
            grid: Grid::None,
            aurora: Some([(0x2d, 0x5b, 0xff), (0x9b, 0x3d, 0xff), (0x00, 0xd4, 0xb4)]),
            ..colors(Theme::Dark)
        },
        Theme::Neon => Colors {
            canvas: (0x07, 0x06, 0x0d),
            card: (0x0e, 0x0c, 0x1a),
            hover: (0x1c, 0x17, 0x33),
            sep: (0x2a, 0x22, 0x4d),
            text: (0xf2, 0xf0, 0xff),
            secondary: (0x9a, 0x94, 0xc8),
            tertiary: (0x5c, 0x56, 0x88),
            accent: [
                (0x00, 0xe5, 0xff),
                (0x39, 0xff, 0x14),
                (0xff, 0x9e, 0x00),
                (0xff, 0x2d, 0x55),
                (0xff, 0xee, 0x00),
                (0x00, 0xff, 0xc8),
                (0xb3, 0x4d, 0xff),
                (0xff, 0x2d, 0xd1),
                (0x6a, 0x6a, 0xff),
                (0x9a, 0x9a, 0xb8),
            ],
            shadow: Shadow::Glow,
            edge: Some((0x00, 0xe5, 0xff)),
            border_scale: 1.3,
            radius_scale: 0.7,
            grid: Grid::Dots,
            ..colors(Theme::Dark)
        },
        Theme::Blueprint => Colors {
            canvas: (0x0d, 0x2b, 0x55),
            card: (0x12, 0x35, 0x66),
            hover: (0x1b, 0x47, 0x82),
            sep: (0x4f, 0x86, 0xc6),
            text: (0xe8, 0xf4, 0xff),
            secondary: (0x9d, 0xc4, 0xea),
            tertiary: (0x6a, 0x93, 0xc2),
            accent: [
                (0x7d, 0xd3, 0xfc),
                (0x86, 0xef, 0xac),
                (0xfd, 0xba, 0x74),
                (0xfc, 0xa5, 0xa5),
                (0xfd, 0xe0, 0x47),
                (0x5e, 0xea, 0xd4),
                (0xc4, 0xb5, 0xfd),
                (0xf9, 0xa8, 0xd4),
                (0xa5, 0xb4, 0xfc),
                (0xcb, 0xd5, 0xe1),
            ],
            shadow: Shadow::Flat,
            border_scale: 1.2,
            radius_scale: 0.35,
            grid: Grid::Lines,
            ..colors(Theme::Dark)
        },
        Theme::Paper => Colors {
            canvas: (0xf4, 0xee, 0xe0),
            card: (0xfd, 0xfa, 0xf2),
            hover: (0xee, 0xe6, 0xd2),
            sep: (0xdd, 0xd3, 0xbb),
            text: (0x2b, 0x25, 0x1c),
            secondary: (0x7a, 0x6f, 0x5c),
            tertiary: (0xb3, 0xa8, 0x92),
            accent: [
                (0x3b, 0x6e, 0xa8),
                (0x5a, 0x8f, 0x4e),
                (0xd0, 0x80, 0x2a),
                (0xc2, 0x4a, 0x3a),
                (0xd4, 0xa8, 0x1c),
                (0x3d, 0x8f, 0x8f),
                (0x8a, 0x5a, 0xa8),
                (0xc2, 0x55, 0x7a),
                (0x56, 0x5a, 0xa8),
                (0x8a, 0x82, 0x72),
            ],
            dark: false,
            radius_scale: 0.6,
            ..colors(Theme::Light)
        },
        Theme::Brutal => Colors {
            canvas: (0xff, 0xe6, 0x6d),
            card: (0xff, 0xff, 0xff),
            hover: (0xff, 0xf1, 0xb0),
            sep: (0x00, 0x00, 0x00),
            text: (0x00, 0x00, 0x00),
            secondary: (0x2e, 0x2e, 0x2e),
            tertiary: (0x77, 0x77, 0x77),
            accent: [
                (0x2f, 0x6b, 0xff),
                (0x00, 0xc8, 0x6e),
                (0xff, 0x7a, 0x00),
                (0xff, 0x3b, 0x3b),
                (0xff, 0xd6, 0x00),
                (0x00, 0xc2, 0xd1),
                (0x9b, 0x4d, 0xff),
                (0xff, 0x4d, 0xa6),
                (0x4a, 0x4a, 0xff),
                (0x66, 0x66, 0x66),
            ],
            dark: false,
            shadow: Shadow::Hard,
            edge: Some((0, 0, 0)),
            border_scale: 2.2,
            radius_scale: 0.35,
            grid: Grid::None,
            aurora: None,
        },
    }
}

/// Blend two themes for the switch animation: colours and metrics glide, while the discrete
/// material (shadow style, grid, aurora) flips at the halfway point.
pub fn lerp_colors(a: &Colors, b: &Colors, t: f32) -> Colors {
    let t = t.clamp(0.0, 1.0);
    let late = if t < 0.5 { a } else { b };
    let f = |x: f32, y: f32| x + (y - x) * t;
    let mut accent = a.accent;
    for (i, c) in accent.iter_mut().enumerate() {
        *c = mix(a.accent[i], b.accent[i], t);
    }
    Colors {
        canvas: mix(a.canvas, b.canvas, t),
        card: mix(a.card, b.card, t),
        hover: mix(a.hover, b.hover, t),
        sep: mix(a.sep, b.sep, t),
        text: mix(a.text, b.text, t),
        secondary: mix(a.secondary, b.secondary, t),
        tertiary: mix(a.tertiary, b.tertiary, t),
        accent,
        dark: late.dark,
        shadow: late.shadow,
        edge: match (a.edge, b.edge) {
            (Some(x), Some(y)) => Some(mix(x, y, t)),
            _ => late.edge,
        },
        border_scale: f(a.border_scale, b.border_scale),
        radius_scale: f(a.radius_scale, b.radius_scale),
        grid: late.grid,
        aurora: late.aurora,
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let f = |x: u8, y: u8| {
        (x as f32 * (1.0 - t) + y as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

fn luminance(c: Rgb) -> f32 {
    0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32
}

/// World units between grid dots at the base level.
const GRID: f32 = 32.0;
/// Do not draw dots closer than this many buffer pixels.
const MIN_DOT_PITCH: f32 = 14.0;
const WHITE: Rgb = (0xff, 0xff, 0xff);

fn bgr(c: Rgb) -> Rgb {
    (c.2, c.1, c.0)
}

fn paint(c: Rgb, alpha: u8, aa: bool) -> Paint<'static> {
    let (r, g, b) = bgr(c);
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, alpha);
    p.anti_alias = aa;
    p
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// World -> buffer pixel mapping for one frame.
#[derive(Clone, Copy)]
struct View {
    cam: Camera,
    /// World unit -> buffer px (zoom * scale).
    z: f32,
    scale: f32,
    w: u32,
    h: u32,
}

impl View {
    fn x(&self, wx: f32) -> f32 {
        (wx - self.cam.x) * self.z
    }
    fn y(&self, wy: f32) -> f32 {
        (wy - self.cam.y) * self.z
    }
    /// A rectangle in buffer px.
    fn rect(&self, r: LRect) -> (f32, f32, f32, f32) {
        (self.x(r.x), self.y(r.y), r.w * self.z, r.h * self.z)
    }
    /// 1 logical px hairline in buffer px.
    fn hair(&self) -> f32 {
        self.scale.max(1.0)
    }
    fn visible(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        !(x > self.w as f32 || y > self.h as f32 || x + w < 0.0 || y + h < 0.0)
    }
}

/// Everything a frame needs besides the pixel buffer.
pub struct Scene<'a> {
    pub board: &'a Board,
    pub editor: &'a Editor,
    pub theme: Theme,
    /// The colours to draw with: a blend while the theme is animating (else `colors(theme)`).
    pub palette: Option<Colors>,
    pub background: &'a Background,
    /// Pre-scaled, pre-dimmed wallpaper in buffer byte order (see `build_wallpaper`).
    pub wallpaper: Option<&'a [u8]>,
    pub overlay: &'a Overlay,
    pub current_area: Option<usize>,
    /// Draw the keyboard shortcuts popup.
    pub show_help: bool,
    /// The todo detail page, when open.
    pub detail: Option<&'a crate::detail::Detail>,
    /// The popup naming a space, when open.
    pub name_box: Option<&'a crate::detail::NameBox>,
    /// The theme panel is open.
    pub theme_open: bool,
    /// What the pointer is over in the overlay.
    pub hover: Hover,
    /// The format bar and grip of the focused text box.
    pub text_bar: Option<&'a TextBar>,
}

/// Draw the whole frame. `canvas` is `w * h * 4` bytes, `scale` the integer buffer scale.
pub fn draw(
    canvas: &mut [u8],
    w: u32,
    h: u32,
    cam: &Camera,
    scale: u32,
    scene: &Scene,
    fonts: &mut Fonts,
) {
    let c = scene.palette.unwrap_or_else(|| colors(scene.theme));
    let v = View {
        cam: *cam,
        z: cam.zoom * scale as f32,
        scale: scale as f32,
        w,
        h,
    };

    // Background: wallpaper, else a solid colour with a dot grid.
    let canvas_len = canvas.len();
    match scene.wallpaper {
        Some(wp) if wp.len() == canvas_len => canvas.copy_from_slice(wp),
        _ => {
            let bg = scene.background.solid().unwrap_or(c.canvas);
            let Some(mut pm) = PixmapMut::from_bytes(canvas, w, h) else {
                return;
            };
            let (r, g, b) = bgr(bg);
            pm.fill(Color::from_rgba8(r, g, b, 0xff));
            if let Some(cols) = c.aurora {
                draw_aurora(&mut pm, &v, cols);
            }
            draw_grid(&mut pm, &v, bg, c.grid);
        }
    }
    let Some(mut pm) = PixmapMut::from_bytes(canvas, w, h) else {
        return;
    };

    // Shapes sit under the cards so cards stay clickable on top of them.
    for s in &scene.board.shapes {
        draw_shape(&mut pm, &v, fonts, s, &c);
    }
    for cn in &scene.board.connectors {
        let f: &Fonts = fonts;
        let path = connector_path(scene.board, cn, &|s, ch| f.advance(s.face(), ch, s.size()));
        draw_connector(&mut pm, &v, &path, cn.color, &c);
    }
    for (ai, a) in scene.board.areas.iter().enumerate() {
        draw_area(&mut pm, &v, fonts, a, ai, scene, &c);
    }
    for n in &scene.board.notes {
        draw_note(&mut pm, &v, fonts, n, scene, &c);
    }
    for t in &scene.board.texts {
        draw_free_text(&mut pm, &v, fonts, t, scene, &c);
    }
    for s in &scene.board.strokes {
        draw_stroke(&mut pm, &v, s, &c);
    }
    draw_selection(&mut pm, &v, fonts, scene, &c);
    draw_marquee(&mut pm, &v, scene, &c);
    draw_text_bar(&mut pm, &v, fonts, scene, &c);
    draw_overlay(&mut pm, &v, fonts, scene, &c);
}

/// Three big soft radial lights, drifting a little against the pan for depth. Drawn once per
/// frame into the existing buffer; no extra memory.
fn draw_aurora(pm: &mut PixmapMut, v: &View, cols: [Rgb; 3]) {
    let (w, h) = (v.w as f32, v.h as f32);
    let spots = [
        (0.18_f32, 0.15_f32, 0.55_f32),
        (0.85, 0.30, 0.50),
        (0.45, 0.95, 0.60),
    ];
    for (i, (fx, fy, fr)) in spots.iter().enumerate() {
        let drift = 0.04 * v.z * (1.0 + i as f32 * 0.5);
        let (cx, cy) = (fx * w - v.cam.x * drift, fy * h - v.cam.y * drift);
        let r = fr * w.max(h);
        let (cr, cg, cb) = bgr(cols[i]);
        let stops = vec![
            GradientStop::new(0.0, Color::from_rgba8(cr, cg, cb, 90)),
            GradientStop::new(1.0, Color::from_rgba8(cr, cg, cb, 0)),
        ];
        let Some(shader) = RadialGradient::new(
            Point::from_xy(cx, cy),
            0.0,
            Point::from_xy(cx, cy),
            r,
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            continue;
        };
        let mut p = Paint::default();
        p.shader = shader;
        p.anti_alias = false;
        if let Some(rect) = Rect::from_xywh(0.0, 0.0, w, h) {
            pm.fill_rect(rect, &p, Transform::identity(), None);
        }
    }
}

fn draw_grid(pm: &mut PixmapMut, v: &View, bg: Rgb, kind: Grid) {
    if kind == Grid::None {
        return;
    }
    let (cam, z) = (&v.cam, v.z);
    let toward = if luminance(bg) < 128.0 {
        WHITE
    } else {
        (0, 0, 0)
    };
    let minor_c = mix(bg, toward, 0.09);
    let major_c = mix(bg, toward, 0.20);

    // Double the spacing until dots are far enough apart (keeps density bounded).
    let mut step = GRID;
    while step * z < MIN_DOT_PITCH {
        step *= 2.0;
    }
    let wx1 = cam.x + v.w as f32 / z;
    let wy1 = cam.y + v.h as f32 / z;
    let (i0, i1) = ((cam.x / step).floor() as i64, (wx1 / step).ceil() as i64);
    let (j0, j1) = ((cam.y / step).floor() as i64, (wy1 / step).ceil() as i64);
    let size = if z >= 1.0 {
        2.0 * v.scale.max(1.0)
    } else {
        v.scale.max(1.0)
    };
    let (minor, major) = (paint(minor_c, 0xff, false), paint(major_c, 0xff, false));
    let tf = Transform::identity();
    if kind == Grid::Lines {
        let th = v.hair();
        for j in j0..=j1 {
            let p = if j % 4 == 0 { &major } else { &minor };
            if let Some(r) = Rect::from_xywh(0.0, v.y(j as f32 * step), v.w as f32, th) {
                pm.fill_rect(r, p, tf, None);
            }
        }
        for i in i0..=i1 {
            let p = if i % 4 == 0 { &major } else { &minor };
            if let Some(r) = Rect::from_xywh(v.x(i as f32 * step), 0.0, th, v.h as f32) {
                pm.fill_rect(r, p, tf, None);
            }
        }
        return;
    }
    for j in j0..=j1 {
        let sy = v.y(j as f32 * step);
        for i in i0..=i1 {
            let sx = v.x(i as f32 * step);
            let p = if i % 4 == 0 && j % 4 == 0 {
                &major
            } else {
                &minor
            };
            if let Some(rect) = Rect::from_xywh(sx - size / 2.0, sy - size / 2.0, size, size) {
                pm.fill_rect(rect, p, tf, None);
            }
        }
    }
}

/// Draw one line of text in buffer px. `x` is the left edge, `cy` the vertical centre, and
/// glyphs outside `clip` (buffer x range) are not drawn. Returns the text width in px.
#[allow(clippy::too_many_arguments)]
fn text_px(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    face: Face,
    size_px: f32,
    x: f32,
    cy: f32,
    s: &str,
    color: Rgb,
    clip: (f32, f32),
    strike: bool,
) -> f32 {
    let width = fonts.text_width(face, s, size_px);
    if s.is_empty() {
        return 0.0;
    }
    let tf = Transform::identity();
    if size_px < MIN_GLYPH_PX {
        // Too small to read: a faint bar instead of glyphs.
        let (x0, x1) = (x.max(clip.0), (x + width).min(clip.1));
        if x1 > x0 {
            if let Some(r) =
                Rect::from_xywh(x0, cy - size_px * 0.15, x1 - x0, (size_px * 0.3).max(1.0))
            {
                pm.fill_rect(r, &paint(color, 90, false), tf, None);
            }
        }
        return width;
    }
    let (asc, desc) = fonts.ascent_descent(face, size_px);
    let baseline = cy + (asc - desc) / 2.0;
    let qpx = quantize_px(size_px);
    let (wi, hi) = (v.w as i32, v.h as i32);
    let clip_i = (clip.0.floor() as i32, clip.1.ceil() as i32);
    let mut pen = x;
    for ch in s.chars() {
        let adv = fonts.advance(face, ch, size_px);
        if ch != ' ' && pen + adv >= clip.0 && pen <= clip.1 {
            let g = fonts.glyph(face, ch, qpx);
            let gx = (pen + g.xmin as f32).round() as i32;
            let gy = (baseline - (g.ymin + g.h as i32) as f32).round() as i32;
            blit(pm.data_mut(), wi, hi, g, gx, gy, color, clip_i);
        }
        pen += adv;
    }
    if strike {
        let (x0, x1) = (x.max(clip.0), (x + width).min(clip.1));
        if x1 > x0 {
            if let Some(r) = Rect::from_xywh(x0, cy, x1 - x0, (1.2 * v.z).max(1.0)) {
                pm.fill_rect(r, &paint(color, 0xff, false), tf, None);
            }
        }
    }
    width
}

fn card_shadow(pm: &mut PixmapMut, v: &View, x: f32, y: f32, w: f32, h: f32, r: f32, c: &Colors) {
    let tf = Transform::identity();
    match c.shadow {
        Shadow::Flat => {}
        Shadow::Hard => {
            // A solid offset block, no blur.
            if let Some(p) = rounded_rect(x + 5.0 * v.z, y + 5.0 * v.z, w, h, r) {
                pm.fill_path(
                    &p,
                    &paint((0, 0, 0), 0xff, true),
                    FillRule::Winding,
                    tf,
                    None,
                );
            }
        }
        Shadow::Glow => {
            // Stacked translucent halos in the edge colour, widest first.
            let col = c.edge.unwrap_or(c.accent[0]);
            for (grow, a) in [(12.0, 10u8), (7.0, 16), (3.0, 28)] {
                let g = grow * v.z;
                if let Some(p) = rounded_rect(x - g, y - g, w + 2.0 * g, h + 2.0 * g, r + g) {
                    pm.fill_path(&p, &paint(col, a, true), FillRule::Winding, tf, None);
                }
            }
        }
        Shadow::Soft => {
            // Two hard-edged layers (no blur): a tight contact shadow and a wider, fainter one.
            let (a1, a2) = if c.dark { (60, 34) } else { (26, 14) };
            if let Some(p) = rounded_rect(x + 3.0 * v.z, y + 10.0 * v.z, w - 6.0 * v.z, h, r) {
                pm.fill_path(&p, &paint((0, 0, 0), a2, true), FillRule::Winding, tf, None);
            }
            if let Some(p) = rounded_rect(x, y + 1.5 * v.z, w, h, r) {
                pm.fill_path(&p, &paint((0, 0, 0), a1, true), FillRule::Winding, tf, None);
            }
        }
    }
}

fn draw_card(
    pm: &mut PixmapMut,
    v: &View,
    rect: LRect,
    r: f32,
    fill: Rgb,
    border: Rgb,
    border_w: f32,
    c: &Colors,
) {
    let (x, y, w, h) = v.rect(rect);
    let r = r * c.radius_scale;
    let border_w = border_w * c.border_scale;
    let border = match c.edge {
        Some(e) if border == c.sep => e,
        _ => border,
    };
    card_shadow(pm, v, x, y, w, h, r * v.z, c);
    if let Some(p) = rounded_rect(x, y, w, h, r * v.z) {
        pm.fill_path(
            &p,
            &paint(fill, 0xff, true),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let st = SkStroke {
            width: border_w,
            ..SkStroke::default()
        };
        pm.stroke_path(
            &p,
            &paint(border, 0xff, true),
            &st,
            Transform::identity(),
            None,
        );
    }
}

fn draw_check_circle(
    pm: &mut PixmapMut,
    v: &View,
    circle: LRect,
    color: Rgb,
    done: bool,
    c: &Colors,
) {
    let (x, y, s, _) = v.rect(circle);
    let (cx, cy, r) = (x + s / 2.0, y + s / 2.0, s / 2.0);
    let tf = Transform::identity();
    let ring = (1.75 * v.z).max(v.hair());
    if done {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r - ring / 2.0) {
            pm.fill_path(&p, &paint(color, 0xff, true), FillRule::Winding, tf, None);
        }
        let mut pb = PathBuilder::new();
        pb.move_to(x + s * 0.27, y + s * 0.53);
        pb.line_to(x + s * 0.43, y + s * 0.69);
        pb.line_to(x + s * 0.74, y + s * 0.32);
        if let Some(path) = pb.finish() {
            let st = SkStroke {
                width: (2.2 * v.z).max(1.5),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..SkStroke::default()
            };
            pm.stroke_path(&path, &paint(WHITE, 0xff, true), &st, tf, None);
        }
    } else if let Some(p) = PathBuilder::from_circle(cx, cy, r - ring / 2.0) {
        let st = SkStroke {
            width: ring,
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(color, 0xff, true), &st, tf, None);
        let _ = c;
    }
}

/// Where the caret of the field being edited goes, in buffer px: (x, y_centre, height).
fn caret_for(
    v: &View,
    fonts: &Fonts,
    board: &Board,
    target: EditTarget,
    caret: usize,
    g_text: Option<(f32, f32, f32)>,
) -> Option<(f32, f32, f32)> {
    let style = style_of(board, target);
    let size_px = style.size() * v.z;
    let adv = |s: Style, ch: char| fonts.advance(s.face(), ch, s.size());
    let text: Vec<char> = crate::editor::text_of(board, target)?.chars().collect();
    let caret = caret.min(text.len());
    let h = size_px * 1.25;
    if let Some(lines) = lines_of(board, target, &adv) {
        let li = line_of(&lines, caret);
        let (bx, by, line_h) = g_text?;
        let pre: f32 = text[lines[li].start..caret]
            .iter()
            .map(|&ch| adv(style, ch))
            .sum();
        return Some((v.x(bx + pre), v.y(by + (li as f32 + 0.5) * line_h), h));
    }
    let (tx, cy, tw) = g_text?;
    let cx: f32 = text[..caret].iter().map(|&ch| adv(style, ch)).sum();
    let scroll = scroll_x(tw, cx);
    Some((v.x(tx + cx - scroll), v.y(cy), h))
}

/// Highlight of the selected text of the field being edited (drawn under the caret).
fn draw_text_sel(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &Fonts,
    scene: &Scene,
    target: EditTarget,
    g_text: Option<(f32, f32, f32)>,
    color: Rgb,
) {
    let Some((s0, e0)) = scene.editor.selection() else {
        return;
    };
    let Some(text) = crate::editor::text_of(scene.board, target) else {
        return;
    };
    let text: Vec<char> = text.chars().collect();
    let (s0, e0) = (s0.min(text.len()), e0.min(text.len()));
    let style = style_of(scene.board, target);
    let adv = |s: Style, ch: char| fonts.advance(s.face(), ch, s.size());
    let mut spans: Vec<(usize, usize)> = Vec::new();
    match lines_of(scene.board, target, &adv) {
        Some(lines) => {
            for l in &lines {
                let (a, b) = (s0.max(l.start), e0.min(l.end));
                if a < b {
                    spans.push((a, b));
                }
            }
        }
        None => spans.push((s0, e0)),
    }
    for (a, b) in spans {
        let Some((x, cy, h)) = caret_for(v, fonts, scene.board, target, a, g_text) else {
            continue;
        };
        let w: f32 = text[a..b].iter().map(|&ch| adv(style, ch)).sum::<f32>() * v.z;
        if let Some(r) = Rect::from_xywh(x, cy - h / 2.0, w, h) {
            pm.fill_rect(r, &paint(color, 0x66, false), Transform::identity(), None);
        }
    }
}

fn draw_caret(pm: &mut PixmapMut, v: &View, x: f32, cy: f32, h: f32, color: Rgb) {
    let w = (1.5 * v.z).max(v.scale);
    if let Some(r) = Rect::from_xywh(x, cy - h / 2.0, w, h) {
        pm.fill_rect(r, &paint(color, 0xff, false), Transform::identity(), None);
    }
}

fn draw_area(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    a: &Area,
    ai: usize,
    scene: &Scene,
    c: &Colors,
) {
    let g: AreaGeom = {
        let f: &Fonts = fonts;
        area_geom(a, &|s, ch| f.advance(s.face(), ch, s.size()))
    };
    let (cx, cy, cw, chh) = v.rect(g.card);
    if !v.visible(cx, cy, cw, chh) {
        return;
    }
    let accent = c.accent[a.color as usize % 10];
    let editing = scene.editor.editing;
    let (border, bw) = if a.hero {
        (accent, (2.0 * v.scale).max(2.0))
    } else {
        (c.sep, v.hair())
    };
    draw_card(pm, v, g.card, radius(a), c.card, border, bw, c);

    // Header: title, open count, colour dot.
    let tstyle = if a.hero {
        Style::HeroTitle
    } else {
        Style::Title
    };
    let hdr_cy = v.y(g.header.y + g.header.h / 2.0);
    let t_clip = (v.x(g.title.x), v.x(g.title.right()));
    let editing_title = editing.is_some_and(|e| e.target == EditTarget::AreaTitle(a.id));
    let size_px = tstyle.size() * v.z;
    if editing_title {
        let e = editing.unwrap();
        let pre: f32 = a
            .name
            .chars()
            .take(e.caret)
            .map(|ch| fonts.advance(tstyle.face(), ch, tstyle.size()))
            .sum();
        let scroll = scroll_x(g.title.w, pre);
        text_px(
            pm,
            v,
            fonts,
            tstyle.face(),
            size_px,
            v.x(g.title.x - scroll),
            hdr_cy,
            &a.name,
            c.text,
            t_clip,
            false,
        );
        draw_text_sel(
            pm,
            v,
            fonts,
            scene,
            e.target,
            Some((g.title.x, g.header.y + g.header.h / 2.0, g.title.w)),
            c.accent[0],
        );
        if let Some((x, cy2, h)) = caret_for(
            v,
            fonts,
            scene.board,
            e.target,
            e.caret,
            Some((g.title.x, g.header.y + g.header.h / 2.0, g.title.w)),
        ) {
            draw_caret(pm, v, x, cy2, h, c.text);
        }
    } else {
        let shown = fonts.fit_ellipsis(tstyle.face(), &a.name, tstyle.size(), g.title.w);
        text_px(
            pm,
            v,
            fonts,
            tstyle.face(),
            size_px,
            v.x(g.title.x),
            hdr_cy,
            &shown,
            c.text,
            t_clip,
            false,
        );
    }
    let open = a.open_count().to_string();
    let hint_px = Style::Hint.size() * v.z;
    let ow = fonts.text_width(Face::Regular, &open, hint_px);
    let count_x = v.x(g.dot.x) - 10.0 * v.z - ow;
    let count_col = if a.is_full() { c.accent[2] } else { c.tertiary };
    text_px(
        pm,
        v,
        fonts,
        Face::Regular,
        hint_px,
        count_x,
        hdr_cy,
        &open,
        count_col,
        (0.0, v.w as f32),
        false,
    );
    let (dx, dy, ds, _) = v.rect(g.dot);
    if let Some(p) = PathBuilder::from_circle(dx + ds / 2.0, dy + ds / 2.0, ds / 2.0) {
        pm.fill_path(
            &p,
            &paint(accent, 0xff, true),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    hairline(pm, v, g.card.x + PAD, g.card.right() - PAD, g.sep_y, c.sep);

    // Rows.
    let row_px = Style::Row.size() * v.z;
    let prio_orange = c.accent[2];
    let today = crate::detail::today();
    for (ri, (t, row)) in a.todos.iter().zip(&g.rows).enumerate() {
        let edit_here = editing.is_some_and(|e| matches!(e.target, EditTarget::TodoText(id, r) | EditTarget::TodoBody(id, r) if id == a.id && r == ri));
        if edit_here {
            let (x, y, w, h) = v.rect(LRect::new(
                row.rect.x + 6.0,
                row.rect.y + 2.0,
                row.rect.w - 12.0,
                row.rect.h - 4.0,
            ));
            if let Some(p) = rounded_rect(x, y, w, h, 10.0 * v.z) {
                pm.fill_path(
                    &p,
                    &paint(c.hover, 0xff, true),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
        draw_check_circle(pm, v, row.circle, accent, t.done, c);
        let due_badge = t.due.as_deref().and_then(|d| {
            crate::detail::badge(d, today).map(|b| (b, crate::detail::overdue(d, today)))
        });
        let badge_w = due_badge.as_ref().map_or(0.0, |(b, _)| {
            fonts.text_width(Face::Regular, b, Style::Chip.size()) + 10.0
        });
        let row_cy = v.y(row.rect.y + ROW_H / 2.0);
        let clip = (v.x(row.text.x), v.x(row.text.right()));
        let col = if t.done { c.tertiary } else { c.text };
        let editing_text = editing.is_some_and(|e| e.target == EditTarget::TodoText(a.id, ri));
        if editing_text {
            let e = editing.unwrap();
            let pre: f32 = t
                .text
                .chars()
                .take(e.caret)
                .map(|ch| fonts.advance(Face::Regular, ch, Style::Row.size()))
                .sum();
            let scroll = scroll_x(row.text.w, pre);
            text_px(
                pm,
                v,
                fonts,
                Face::Regular,
                row_px,
                v.x(row.text.x - scroll),
                row_cy,
                &t.text,
                col,
                clip,
                t.done,
            );
            draw_text_sel(
                pm,
                v,
                fonts,
                scene,
                e.target,
                Some((row.text.x, row.rect.y + ROW_H / 2.0, row.text.w)),
                c.accent[0],
            );
            if let Some((x, cy2, h)) = caret_for(
                v,
                fonts,
                scene.board,
                e.target,
                e.caret,
                Some((row.text.x, row.rect.y + ROW_H / 2.0, row.text.w)),
            ) {
                draw_caret(pm, v, x, cy2, h, c.text);
            }
        } else if t.text.is_empty() {
            // placeholder only while it is the (empty) edited row; nothing otherwise
        } else if crate::md::has_markup(&t.text, false) {
            let chars: Vec<char> = t.text.chars().collect();
            let cs = crate::md::char_styles(&t.text, false);
            let clip = (clip.0, clip.1.min(v.x(row.text.right() - badge_w)));
            draw_md_chars(
                pm,
                v,
                fonts,
                Face::Regular,
                row_px,
                v.x(row.text.x),
                row_cy,
                &chars,
                &cs,
                col,
                clip,
                t.done,
                c,
            );
        } else {
            let shown = fonts.fit_ellipsis(
                Face::Regular,
                &t.text,
                Style::Row.size(),
                row.text.w - badge_w,
            );
            text_px(
                pm,
                v,
                fonts,
                Face::Regular,
                row_px,
                v.x(row.text.x),
                row_cy,
                &shown,
                col,
                clip,
                t.done,
            );
        }
        if t.priority > 0 {
            let marks = "!".repeat(t.priority as usize);
            let mw = fonts.text_width(Face::Bold, &marks, row_px);
            text_px(
                pm,
                v,
                fonts,
                Face::Bold,
                row_px,
                v.x(row.prio.right() - PAD) - mw,
                row_cy,
                &marks,
                prio_orange,
                (0.0, v.w as f32),
                false,
            );
        }
        if let Some((label, late)) = &due_badge {
            let mut right = row.prio.right() - PAD;
            if t.priority > 0 {
                let mw = fonts.text_width(
                    Face::Bold,
                    &"!".repeat(t.priority as usize),
                    Style::Row.size(),
                );
                right -= mw + 8.0;
            }
            let bw = fonts.text_width(Face::Regular, label, Style::Chip.size());
            let col = if t.done {
                c.tertiary
            } else if *late {
                c.accent[3]
            } else {
                c.secondary
            };
            text_px(
                pm,
                v,
                fonts,
                Face::Regular,
                Style::Chip.size() * v.z,
                v.x(right - bw),
                row_cy,
                label,
                col,
                (0.0, v.w as f32),
                false,
            );
        }
        if let Some(body) = &row.body {
            let src = t.body.as_deref().unwrap_or("");
            let chars: Vec<char> = src.chars().collect();
            let body_px = Style::Chip.size() * v.z;
            let editing_body = editing.is_some_and(|e| e.target == EditTarget::TodoBody(a.id, ri));
            let md = (!editing_body && crate::md::has_markup(src, true))
                .then(|| crate::md::char_styles(src, true));
            for (li, line) in body.lines.iter().enumerate() {
                let s: String = chars[line.start..line.end].iter().collect();
                let ly = v.y(body.rect.y + (li as f32 + 0.5) * BODY_LINE_H);
                if let Some(cs) = &md {
                    draw_md_chars(
                        pm,
                        v,
                        fonts,
                        Face::Regular,
                        body_px,
                        v.x(body.rect.x),
                        ly,
                        &chars[line.start..line.end],
                        &cs[line.start..line.end],
                        c.secondary,
                        (v.x(body.rect.x), v.x(body.rect.right())),
                        false,
                        c,
                    );
                    continue;
                }
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    body_px,
                    v.x(body.rect.x),
                    ly,
                    &s,
                    c.secondary,
                    (v.x(body.rect.x), v.x(body.rect.right())),
                    false,
                );
            }
            if editing_body {
                let e = editing.unwrap();
                draw_text_sel(
                    pm,
                    v,
                    fonts,
                    scene,
                    e.target,
                    Some((body.rect.x, body.rect.y, BODY_LINE_H)),
                    c.accent[0],
                );
                if let Some((x, cy2, h)) = caret_for(
                    v,
                    fonts,
                    scene.board,
                    e.target,
                    e.caret,
                    Some((body.rect.x, body.rect.y, BODY_LINE_H)),
                ) {
                    draw_caret(pm, v, x, cy2, h, c.text);
                }
            }
        }
        if ri + 1 < a.todos.len() {
            hairline(
                pm,
                v,
                row.text.x,
                g.card.right() - PAD,
                row.rect.bottom(),
                c.sep,
            );
        }
    }

    // Footer: "+ New todo" (or a note when the Focus card is full).
    let f_cy = v.y(g.footer.y + g.footer.h / 2.0 - 2.0);
    let (fx, fy, _, _) = v.rect(LRect::new(g.card.x + PAD, g.footer.y, CIRCLE, g.footer.h));
    let plus_cx = fx + CIRCLE * v.z / 2.0;
    let _ = fy;
    if a.is_full() {
        let msg = format!("{MAX_HERO_OPEN} open: finish one first");
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            Style::Chip.size() * v.z,
            v.x(g.card.x + PAD + CIRCLE + TEXT_GAP),
            f_cy,
            &msg,
            c.tertiary,
            (0.0, v.w as f32),
            false,
        );
    } else {
        draw_plus(pm, v, plus_cx, f_cy, 6.0 * v.z, accent);
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            row_px,
            v.x(g.card.x + PAD + CIRCLE + TEXT_GAP),
            f_cy,
            "New todo",
            c.tertiary,
            (0.0, v.w as f32),
            false,
        );
    }
    let _ = ai;
}

fn draw_plus(pm: &mut PixmapMut, v: &View, cx: f32, cy: f32, r: f32, color: Rgb) {
    let mut pb = PathBuilder::new();
    pb.move_to(cx - r, cy);
    pb.line_to(cx + r, cy);
    pb.move_to(cx, cy - r);
    pb.line_to(cx, cy + r);
    if let Some(p) = pb.finish() {
        let st = SkStroke {
            width: (2.0 * v.z).max(1.5),
            line_cap: LineCap::Round,
            ..SkStroke::default()
        };
        pm.stroke_path(
            &p,
            &paint(color, 0xff, true),
            &st,
            Transform::identity(),
            None,
        );
    }
}

fn hairline(pm: &mut PixmapMut, v: &View, x0: f32, x1: f32, y: f32, color: Rgb) {
    if let Some(r) = Rect::from_xywh(v.x(x0), v.y(y), (x1 - x0) * v.z, v.hair()) {
        pm.fill_rect(r, &paint(color, 0xff, false), Transform::identity(), None);
    }
}

fn draw_note(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, n: &Note, scene: &Scene, c: &Colors) {
    let g = {
        let f: &Fonts = fonts;
        note_geom(n, &|s, ch| f.advance(s.face(), ch, s.size()))
    };
    let (x, y, w, h) = v.rect(g.card);
    if !v.visible(x, y, w, h) {
        return;
    }
    let accent = c.accent[n.color as usize % 10];
    // Tint the card with its colour so it reads as a coloured note in both themes.
    let fill = mix(c.card, accent, if c.dark { 0.30 } else { 0.22 });
    draw_card(
        pm,
        v,
        g.card,
        NOTE_RADIUS,
        fill,
        mix(fill, accent, 0.5),
        v.hair(),
        c,
    );

    let editing = scene.editor.editing;
    let clip_t = (v.x(g.title.x), v.x(g.title.right()));
    let t_cy = v.y(g.title.y + NOTE_TITLE_H / 2.0);
    let tpx = Style::Title.size() * v.z;
    let editing_title = editing.is_some_and(|e| e.target == EditTarget::NoteTitle(n.id));
    if editing_title {
        let e = editing.unwrap();
        let pre: f32 = n
            .title
            .chars()
            .take(e.caret)
            .map(|ch| fonts.advance(Face::Bold, ch, Style::Title.size()))
            .sum();
        let scroll = scroll_x(g.title.w, pre);
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            tpx,
            v.x(g.title.x - scroll),
            t_cy,
            &n.title,
            c.text,
            clip_t,
            false,
        );
        draw_text_sel(
            pm,
            v,
            fonts,
            scene,
            e.target,
            Some((g.title.x, g.title.y + NOTE_TITLE_H / 2.0, g.title.w)),
            c.accent[0],
        );
        if let Some((cx, cy, h)) = caret_for(
            v,
            fonts,
            scene.board,
            e.target,
            e.caret,
            Some((g.title.x, g.title.y + NOTE_TITLE_H / 2.0, g.title.w)),
        ) {
            draw_caret(pm, v, cx, cy, h, c.text);
        }
    } else if n.title.is_empty() {
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            tpx,
            v.x(g.title.x),
            t_cy,
            "Title",
            c.tertiary,
            clip_t,
            false,
        );
    } else if crate::md::has_markup(&n.title, false) {
        let chars: Vec<char> = n.title.chars().collect();
        let cs = crate::md::char_styles(&n.title, false);
        draw_md_chars(
            pm,
            v,
            fonts,
            Face::Bold,
            tpx,
            v.x(g.title.x),
            t_cy,
            &chars,
            &cs,
            c.text,
            clip_t,
            false,
            c,
        );
    } else {
        let shown = fonts.fit_ellipsis(Face::Bold, &n.title, Style::Title.size(), g.title.w);
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            tpx,
            v.x(g.title.x),
            t_cy,
            &shown,
            c.text,
            clip_t,
            false,
        );
    }
    let (dx, dy, ds, _) = v.rect(g.dot);
    if let Some(p) = PathBuilder::from_circle(dx + ds / 2.0, dy + ds / 2.0, ds / 2.0) {
        pm.fill_path(
            &p,
            &paint(accent, 0xff, true),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    let chars: Vec<char> = n.body.chars().collect();
    let bpx = Style::Row.size() * v.z;
    let clip_b = (v.x(g.body.rect.x), v.x(g.body.rect.right()));
    let editing_body = editing.is_some_and(|e| e.target == EditTarget::NoteBody(n.id));
    if n.body.is_empty() && !editing_body {
        let cy = v.y(g.body.rect.y + NOTE_LINE_H / 2.0);
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            bpx,
            v.x(g.body.rect.x),
            cy,
            "Write something",
            c.tertiary,
            clip_b,
            false,
        );
    }
    let md = (!editing_body && crate::md::has_markup(&n.body, true))
        .then(|| crate::md::char_styles(&n.body, true));
    for (li, line) in g.body.lines.iter().enumerate() {
        let s: String = chars[line.start..line.end].iter().collect();
        let cy = v.y(g.body.rect.y + (li as f32 + 0.5) * NOTE_LINE_H);
        if let Some(cs) = &md {
            draw_md_chars(
                pm,
                v,
                fonts,
                Face::Regular,
                bpx,
                v.x(g.body.rect.x),
                cy,
                &chars[line.start..line.end],
                &cs[line.start..line.end],
                c.text,
                clip_b,
                false,
                c,
            );
            continue;
        }
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            bpx,
            v.x(g.body.rect.x),
            cy,
            &s,
            c.text,
            clip_b,
            false,
        );
    }
    if editing_body {
        let e = editing.unwrap();
        draw_text_sel(
            pm,
            v,
            fonts,
            scene,
            e.target,
            Some((g.body.rect.x, g.body.rect.y, NOTE_LINE_H)),
            c.accent[0],
        );
        if let Some((cx, cy, h)) = caret_for(
            v,
            fonts,
            scene.board,
            e.target,
            e.caret,
            Some((g.body.rect.x, g.body.rect.y, NOTE_LINE_H)),
        ) {
            draw_caret(pm, v, cx, cy, h, c.text);
        }
    }
}

fn draw_stroke(pm: &mut PixmapMut, v: &View, s: &Stroke, c: &Colors) {
    // Cull before allocating: only strokes that touch the screen get their points mapped.
    let Some(&first) = s.pts.first() else {
        return;
    };
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for p in &s.pts {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let pad = s.width * v.z + 2.0;
    let (sx0, sy0) = (v.x(x0), v.y(y0));
    if !v.visible(
        sx0 - pad,
        sy0 - pad,
        (x1 - x0) * v.z + 2.0 * pad,
        (y1 - y0) * v.z + 2.0 * pad,
    ) {
        return;
    }
    let pts: Vec<(f32, f32)> = s.pts.iter().map(|&(x, y)| (v.x(x), v.y(y))).collect();
    let first = pts[0];
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    // A dot (all points equal) needs a zero-length segment to get a round cap.
    if pts.iter().all(|p| *p == first) {
        pb.line_to(first.0 + 0.01, first.1);
    }
    if let Some(path) = pb.finish() {
        let st = SkStroke {
            width: (s.width * v.z).max(v.hair()),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..SkStroke::default()
        };
        pm.stroke_path(
            &path,
            &paint(c.accent[s.color as usize % 10], 0xff, true),
            &st,
            Transform::identity(),
            None,
        );
    }
}

fn draw_shape(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, s: &Shape, c: &Colors) {
    let col = c.accent[s.color as usize % 10];
    let tf = Transform::identity();
    let width = (2.5 * v.z).max(v.hair() * 1.5);
    let stroke = SkStroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..SkStroke::default()
    };
    let (bx, by, bw, bh) = s.bbox();
    let (x, y, w, h) = (v.x(bx), v.y(by), bw * v.z, bh * v.z);
    if !v.visible(x - width, y - width, w + 2.0 * width, h + 2.0 * width) {
        return;
    }
    if matches!(s.kind, ShapeKind::Image | ShapeKind::Sticker) {
        draw_picture(pm, v, fonts, s, (x, y, w, h), col);
        return;
    }
    let path = match s.kind {
        ShapeKind::Image | ShapeKind::Sticker => None,
        ShapeKind::Rect => rounded_rect(x, y, w.max(1.0), h.max(1.0), 6.0 * v.z),
        ShapeKind::Ellipse => {
            Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)).and_then(PathBuilder::from_oval)
        }
        ShapeKind::Line => {
            let mut pb = PathBuilder::new();
            pb.move_to(v.x(s.x1), v.y(s.y1));
            pb.line_to(v.x(s.x2), v.y(s.y2));
            pb.finish()
        }
    };
    let Some(p) = path else {
        return;
    };
    if s.kind != ShapeKind::Line {
        pm.fill_path(&p, &paint(col, 30, true), FillRule::Winding, tf, None);
    }
    pm.stroke_path(&p, &paint(col, 0xff, true), &stroke, tf, None);
}

/// An image or an emoji sticker inside its box (screen px).
fn draw_picture(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    s: &Shape,
    (x, y, w, h): (f32, f32, f32, f32),
    col: Rgb,
) {
    if w < 1.0 || h < 1.0 {
        return;
    }
    if s.kind == ShapeKind::Image {
        if let Some(img) = fonts.pics.image(&s.src) {
            let t = Transform::from_row(
                w / img.width() as f32,
                0.0,
                0.0,
                h / img.height() as f32,
                x,
                y,
            );
            let paint = PixmapPaint {
                quality: FilterQuality::Bilinear,
                ..PixmapPaint::default()
            };
            pm.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
            return;
        }
    } else {
        let px = quantize_px(w.min(h) * 0.9);
        if let Some(g) = fonts.pics.emoji(&s.src, px) {
            let (gx, gy) = (x + (w - g.w as f32) / 2.0, y + (h - g.h as f32) / 2.0);
            let (cw, ch) = (pm.width() as i32, pm.height() as i32);
            blit(pm.data_mut(), cw, ch, g, gx as i32, gy as i32, col, (0, cw));
            return;
        }
    }
    // Missing image or no emoji font: an outlined box so the item can still be found.
    if let Some(p) = rounded_rect(x, y, w, h, 6.0 * v.z) {
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(
            &p,
            &paint(col, 0xff, true),
            &st,
            Transform::identity(),
            None,
        );
    }
}

/// A connector arrow: the routed polyline with an arrowhead at its end.
fn draw_connector(pm: &mut PixmapMut, v: &View, path: &[(f32, f32)], color: u8, c: &Colors) {
    let pts: Vec<(f32, f32)> = path.iter().map(|&(x, y)| (v.x(x), v.y(y))).collect();
    if pts.len() < 2 {
        return;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (pts[0].0, pts[0].1, pts[0].0, pts[0].1);
    for p in &pts {
        (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
    }
    let pad = 16.0 * v.z + 4.0;
    if !v.visible(x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad) {
        return;
    }
    let col = c.accent[color as usize % 10];
    let width = (2.5 * v.z).max(v.hair() * 1.5);
    let stroke = SkStroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..SkStroke::default()
    };
    let tf = Transform::identity();
    // Stop the line a little short so the round cap does not poke through the arrowhead.
    let n = pts.len();
    let (tip, prev) = (pts[n - 1], pts[n - 2]);
    let (dx, dy) = (tip.0 - prev.0, tip.1 - prev.1);
    let len = dx.hypot(dy).max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let head = (11.0 * v.z).max(6.0);
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..n - 1] {
        pb.line_to(p.0, p.1);
    }
    pb.line_to(tip.0 - ux * head * 0.6, tip.1 - uy * head * 0.6);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint(col, 0xff, true), &stroke, tf, None);
    }
    let mut hb = PathBuilder::new();
    hb.move_to(tip.0, tip.1);
    hb.line_to(
        tip.0 - ux * head + uy * head * 0.45,
        tip.1 - uy * head - ux * head * 0.45,
    );
    hb.line_to(
        tip.0 - ux * head - uy * head * 0.45,
        tip.1 - uy * head + ux * head * 0.45,
    );
    hb.close();
    if let Some(p) = hb.finish() {
        pm.fill_path(&p, &paint(col, 0xff, true), FillRule::Winding, tf, None);
    }
}

/// One run of text with optional synthetic italic, underline and strike-through (buffer px).
/// Returns its width.
#[allow(clippy::too_many_arguments)]
fn draw_run(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    face: Face,
    size_px: f32,
    x: f32,
    cy: f32,
    s: &str,
    color: Rgb,
    (italic, underline, strike): (bool, bool, bool),
) -> f32 {
    draw_run_clip(pm, v, fonts, face, size_px, x, cy, s, color, (italic, underline, strike), (0.0, v.w as f32))
}

/// `draw_run` limited to the horizontal range `clip` (buffer px).
#[allow(clippy::too_many_arguments)]
fn draw_run_clip(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    face: Face,
    size_px: f32,
    x: f32,
    cy: f32,
    s: &str,
    color: Rgb,
    (italic, underline, strike): (bool, bool, bool),
    clip: (f32, f32),
) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    if size_px < MIN_GLYPH_PX || !italic {
        let w = text_px(pm, v, fonts, face, size_px, x, cy, s, color, clip, false);
        decorate(pm, v, x, w, cy, size_px, color, fonts, face, underline, strike);
        return w;
    }
    let width = fonts.text_width(face, s, size_px);
    let (asc, desc) = fonts.ascent_descent(face, size_px);
    let baseline = cy + (asc - desc) / 2.0;
    let qpx = quantize_px(size_px);
    let (wi, hi) = (v.w as i32, v.h as i32);
    let clip_i = (clip.0.floor() as i32, clip.1.ceil() as i32);
    let mut pen = x;
    for ch in s.chars() {
        let adv = fonts.advance(face, ch, size_px);
        if ch != ' ' && pen + adv + size_px * 0.3 >= clip.0 && pen <= clip.1 {
            let g = fonts.glyph(face, ch, qpx);
            let gx = (pen + g.xmin as f32).round() as i32;
            let gy = (baseline - (g.ymin + g.h as i32) as f32).round() as i32;
            let slant = (0.2, baseline.round() as i32);
            blit_slanted(pm.data_mut(), wi, hi, g, gx, gy, color, clip_i, slant);
        }
        pen += adv;
    }
    let (x0, x1) = (x.max(clip.0), (x + width).min(clip.1));
    if x1 > x0 {
        decorate(pm, v, x0, x1 - x0, cy, size_px, color, fonts, face, underline, strike);
    }
    width
}

/// Draw chars of a field with Markdown rendered in place (`cs` from `md::char_styles`, one per
/// char): markers hidden, list / task / quote / rule symbols drawn where their prefix was.
/// `strike_all` strikes everything (a done todo). Returns the width drawn.
#[allow(clippy::too_many_arguments)]
fn draw_md_chars(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    base: Face,
    px: f32,
    x: f32,
    cy: f32,
    chars: &[char],
    cs: &[crate::md::Cs],
    color: Rgb,
    clip: (f32, f32),
    strike_all: bool,
    c: &Colors,
) -> f32 {
    let tf = Transform::identity();
    let n = chars.len().min(cs.len());
    let mut pen = x;
    let mut i = 0;
    while i < n {
        let st = cs[i];
        if let Some(b) = st.prefix {
            match b {
                Block::Bullet => {
                    if let Some(p) = PathBuilder::from_circle(pen + px * 0.3, cy, px * 0.14) {
                        pm.fill_path(&p, &paint(color, 0xff, true), FillRule::Winding, tf, None);
                    }
                    pen += px * 0.9;
                }
                Block::Task(done) => {
                    let sz = px * 0.8;
                    let (bx, by) = (pen, cy - sz / 2.0);
                    if let Some(p) = rounded_rect(bx, by, sz, sz, sz * 0.25) {
                        if done {
                            pm.fill_path(&p, &paint(c.accent[0], 0xff, true), FillRule::Winding, tf, None);
                            let mut pb = PathBuilder::new();
                            pb.move_to(bx + sz * 0.24, by + sz * 0.52);
                            pb.line_to(bx + sz * 0.43, by + sz * 0.7);
                            pb.line_to(bx + sz * 0.77, by + sz * 0.3);
                            if let Some(ck) = pb.finish() {
                                let sk = SkStroke {
                                    width: (sz * 0.13).max(1.0),
                                    line_cap: LineCap::Round,
                                    line_join: LineJoin::Round,
                                    ..SkStroke::default()
                                };
                                pm.stroke_path(&ck, &paint(WHITE, 0xff, true), &sk, tf, None);
                            }
                        } else {
                            let sk = SkStroke {
                                width: (px * 0.08).max(1.0),
                                ..SkStroke::default()
                            };
                            pm.stroke_path(&p, &paint(c.secondary, 0xff, true), &sk, tf, None);
                        }
                    }
                    pen += px * 1.2;
                }
                Block::Quote => {
                    let r = Rect::from_xywh(pen + px * 0.05, cy - px * 0.7, (px * 0.14).max(1.5), px * 1.4);
                    if let Some(r) = r {
                        pm.fill_rect(r, &paint(c.accent[0], 0xff, false), tf, None);
                    }
                    pen += px * 0.6;
                }
                Block::Rule => {
                    if let Some(r) = Rect::from_xywh(clip.0, cy, clip.1 - clip.0, v.hair()) {
                        pm.fill_rect(r, &paint(c.sep, 0xff, false), tf, None);
                    }
                    return clip.1 - x;
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        if st.hidden {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && !cs[j].hidden && cs[j].prefix.is_none() && cs[j].same_run(&st) {
            j += 1;
        }
        let run: String = chars[i..j].iter().filter(|ch| **ch != '\n').collect();
        let face = if base == Face::Bold || st.bold || st.heading > 0 {
            Face::Bold
        } else {
            base
        };
        let col = if st.done {
            c.tertiary
        } else if st.dim || st.quote {
            c.secondary
        } else if st.code {
            c.accent[3]
        } else {
            color
        };
        if st.code {
            let w = fonts.text_width(face, &run, px);
            let pad = px * 0.2;
            if let Some(p) = rounded_rect(pen - pad, cy - px * 0.7, w + 2.0 * pad, px * 1.4, px * 0.25) {
                let a = if c.dark { 60 } else { 30 };
                pm.fill_path(&p, &paint(c.secondary, a, true), FillRule::Winding, tf, None);
            }
        }
        let deco = (st.italic || st.quote, false, st.strike || st.done || strike_all);
        pen += draw_run_clip(pm, v, fonts, face, px, pen, cy, &run, col, deco, clip);
        i = j;
    }
    pen - x
}

/// Underline and strike-through lines under / through a run.
#[allow(clippy::too_many_arguments)]
fn decorate(
    pm: &mut PixmapMut,
    v: &View,
    x: f32,
    w: f32,
    cy: f32,
    size_px: f32,
    color: Rgb,
    fonts: &Fonts,
    face: Face,
    underline: bool,
    strike: bool,
) {
    let th = (size_px * 0.065).max(1.0);
    let (asc, desc) = fonts.ascent_descent(face, size_px);
    let baseline = cy + (asc - desc) / 2.0;
    let mut line = |y: f32| {
        if let Some(r) = Rect::from_xywh(x, y, w, th) {
            pm.fill_rect(r, &paint(color, 0xff, false), Transform::identity(), None);
        }
    };
    if underline {
        line(baseline + size_px * 0.1);
    }
    if strike {
        line(baseline - asc * 0.32 - th / 2.0);
    }
    let _ = v;
}

fn text_color(t: &TextBox, c: &Colors) -> Rgb {
    if t.color == 0 {
        c.text
    } else {
        c.accent[t.color as usize % 10]
    }
}

fn draw_free_text(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    t: &TextBox,
    scene: &Scene,
    c: &Colors,
) {
    let g = {
        let f: &Fonts = fonts;
        text_geom(t, &|s, ch| f.advance(s.face(), ch, s.size()))
    };
    let (x, y, w, h) = v.rect(g.rect);
    if !v.visible(x, y, w, h) {
        return;
    }
    let col = text_color(t, c);
    let editing = scene
        .editor
        .editing
        .is_some_and(|e| e.target == EditTarget::FreeText(t.id));
    if editing || t.plain {
        // Raw text: what you type is what you see.
        let st = t.style();
        let chars: Vec<char> = t.text.chars().collect();
        let px = st.size() * v.z;
        for (li, line) in g.lines.iter().enumerate() {
            let s: String = chars[line.start..line.end].iter().collect();
            let cy = v.y(g.rect.y + (li as f32 + 0.5) * g.line_h);
            let deco = (t.italic, t.underline, t.strike);
            draw_run(pm, v, fonts, st.face(), px, x, cy, &s, col, deco);
        }
    } else {
        draw_markdown(pm, v, fonts, t, g.rect.w, col, c);
    }
    if editing {
        let Some(e) = scene.editor.editing else {
            return;
        };
        // A faint box shows where the text is while you type.
        if let Some(p) = rounded_rect(
            x - 4.0 * v.z,
            y - 2.0 * v.z,
            w + 8.0 * v.z,
            h + 4.0 * v.z,
            6.0 * v.z,
        ) {
            let st = SkStroke {
                width: v.hair(),
                ..SkStroke::default()
            };
            pm.stroke_path(
                &p,
                &paint(c.accent[0], 0xff, true),
                &st,
                Transform::identity(),
                None,
            );
        }
        let gt = Some((t.x, t.y, g.line_h));
        draw_text_sel(pm, v, fonts, scene, e.target, gt, c.accent[0]);
        if let Some((cx, cy, ch)) = caret_for(v, fonts, scene.board, e.target, e.caret, gt) {
            draw_caret(pm, v, cx, cy, ch, col);
        }
    }
}

/// A text box's Markdown, rendered (see `md`).
fn draw_markdown(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    t: &TextBox,
    box_w: f32,
    col: Rgb,
    c: &Colors,
) {
    let md = {
        let f: &Fonts = fonts;
        crate::md::layout(&t.text, t.size, t.bold, &|s, ch| {
            f.advance(s.face(), ch, s.size())
        })
    };
    let tf = Transform::identity();
    for l in &md.lines {
        let (top, bot) = (v.y(t.y + l.y), v.y(t.y + l.y + l.h));
        if bot < 0.0 || top > v.h as f32 {
            continue;
        }
        let cy = (top + bot) / 2.0;
        let px = l.size.px() * v.z;
        let px0 = v.x(t.x + l.prefix_x);
        let mut tcol = col;
        let (mut italic, mut strike) = (t.italic, t.strike);
        match l.block {
            Block::Rule => {
                let r = Rect::from_xywh(v.x(t.x), cy, box_w * v.z, v.hair());
                if let Some(r) = r {
                    pm.fill_rect(r, &paint(c.sep, 0xff, false), tf, None);
                }
                continue;
            }
            Block::Bullet => {
                let r = px * 0.15;
                if let Some(p) = PathBuilder::from_circle(px0 + px * 0.45, cy, r) {
                    if l.prefix_x > 0.0 && (l.prefix_x / (px * 1.2)).round() as i32 % 2 == 1 {
                        let st = SkStroke {
                            width: (px * 0.07).max(1.0),
                            ..SkStroke::default()
                        };
                        pm.stroke_path(&p, &paint(col, 0xff, true), &st, tf, None);
                    } else {
                        pm.fill_path(&p, &paint(col, 0xff, true), FillRule::Winding, tf, None);
                    }
                }
            }
            Block::Number => {
                let face = Style::Free(l.size, t.bold).face();
                draw_run(pm, v, fonts, face, px, px0, cy, &l.marker, c.secondary, (false, false, false));
            }
            Block::Task(done) => {
                let sz = px * 0.78;
                let (bx, by) = (px0 + px * 0.05, cy - sz / 2.0);
                if let Some(p) = rounded_rect(bx, by, sz, sz, sz * 0.25) {
                    if done {
                        pm.fill_path(&p, &paint(c.accent[0], 0xff, true), FillRule::Winding, tf, None);
                        let mut pb = PathBuilder::new();
                        pb.move_to(bx + sz * 0.24, by + sz * 0.52);
                        pb.line_to(bx + sz * 0.43, by + sz * 0.7);
                        pb.line_to(bx + sz * 0.77, by + sz * 0.3);
                        if let Some(ck) = pb.finish() {
                            let st = SkStroke {
                                width: (sz * 0.13).max(1.0),
                                line_cap: LineCap::Round,
                                line_join: LineJoin::Round,
                                ..SkStroke::default()
                            };
                            pm.stroke_path(&ck, &paint(WHITE, 0xff, true), &st, tf, None);
                        }
                    } else {
                        let st = SkStroke {
                            width: (px * 0.08).max(1.0),
                            ..SkStroke::default()
                        };
                        pm.stroke_path(&p, &paint(c.secondary, 0xff, true), &st, tf, None);
                    }
                }
                if done {
                    tcol = c.tertiary;
                    strike = true;
                }
            }
            Block::Quote => {
                if let Some(r) = Rect::from_xywh(px0 + px * 0.1, top + 2.0 * v.z, (px * 0.14).max(1.5), bot - top - 4.0 * v.z) {
                    pm.fill_rect(r, &paint(c.accent[0], 0xff, false), tf, None);
                }
                tcol = c.secondary;
                italic = true;
            }
            Block::Heading(_) | Block::Para => {}
        }
        for r in &l.runs {
            let rx = v.x(t.x + r.x);
            let rp = r.style.size() * v.z;
            if r.code {
                let pad = rp * 0.25;
                if let Some(p) = rounded_rect(rx - pad, cy - rp * 0.7, r.w * v.z + 2.0 * pad, rp * 1.4, rp * 0.25) {
                    let a = if c.dark { 60 } else { 30 };
                    pm.fill_path(&p, &paint(c.secondary, a, true), FillRule::Winding, tf, None);
                }
            }
            let deco = (italic || r.italic, t.underline, strike || r.strike);
            let rc = if r.code && tcol == col { c.accent[3] } else { tcol };
            draw_run(pm, v, fonts, r.style.face(), rp, rx, cy, &r.text, rc, deco);
        }
    }
}

/// A list icon (three rows of bullet + line, or checkbox + line) centred on (cx, cy).
fn draw_list_icon(pm: &mut PixmapMut, v: &View, task: bool, cx: f32, cy: f32, color: Rgb) {
    let u = v.scale;
    let tf = Transform::identity();
    for row in 0..3 {
        let y = cy + (row as f32 - 1.0) * 5.5 * u;
        let x0 = cx - 7.0 * u;
        if task {
            if let Some(p) = rounded_rect(x0 - 1.8 * u, y - 1.8 * u, 3.6 * u, 3.6 * u, 0.8 * u) {
                let st = SkStroke {
                    width: 1.1 * u,
                    ..SkStroke::default()
                };
                pm.stroke_path(&p, &paint(color, 0xff, true), &st, tf, None);
            }
        } else if let Some(p) = PathBuilder::from_circle(x0, y, 1.5 * u) {
            pm.fill_path(&p, &paint(color, 0xff, true), FillRule::Winding, tf, None);
        }
        if let Some(r) = Rect::from_xywh(x0 + 4.0 * u, y - 0.75 * u, 10.0 * u, 1.5 * u) {
            pm.fill_rect(r, &paint(color, 0xff, false), tf, None);
        }
    }
}

/// The floating format bar and the drag grip of the focused text box (screen space).
fn draw_text_bar(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, scene: &Scene, c: &Colors) {
    let Some(bar) = scene.text_bar else {
        return;
    };
    let t = bar.text_id().and_then(|id| scene.board.text(id));
    let s = v.scale;
    let tf = Transform::identity();
    // Grip: a small card with two columns of dots.
    if let Some(g) = bar.grip {
    let grip_hot = scene.hover == Hover::Grip;
    if let Some(p) = rounded_rect(g.x * s, g.y * s, g.w * s, g.h * s, 6.0 * s) {
        let fill = if grip_hot { c.hover } else { c.card };
        pm.fill_path(&p, &paint(fill, 235, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
    let dot = if grip_hot { c.text } else { c.secondary };
    for col in 0..2 {
        for row in 0..3 {
            let cx = (g.x + g.w / 2.0 + (col as f32 - 0.5) * 6.0) * s;
            let cy = (g.y + g.h / 2.0 + (row as f32 - 1.0) * 6.0) * s;
            if let Some(p) = PathBuilder::from_circle(cx, cy, 1.7 * s) {
                pm.fill_path(&p, &paint(dot, 0xff, true), FillRule::Winding, tf, None);
            }
        }
    }
    }

    pill(pm, v, bar.bar, c);
    for x in &bar.seps {
        let r = Rect::from_xywh(x * s, (bar.bar.y + 9.0) * s, v.hair(), (bar.bar.h - 18.0) * s);
        if let Some(r) = r {
            pm.fill_rect(r, &paint(c.sep, 0xff, false), tf, None);
        }
    }
    for (btn, r) in &bar.btns {
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let hot = scene.hover == Hover::Fmt(*btn);
        if let FmtBtn::Color(k) = btn {
            let fill = if *k == 0 { c.text } else { c.accent[*k as usize % 10] };
            let ring = if t.is_some_and(|t| t.color == *k) {
                c.accent[0]
            } else if hot {
                c.secondary
            } else {
                c.card
            };
            draw_swatch(pm, v, cx, cy, fill, ring);
            continue;
        }
        let active = t.is_some_and(|t| match btn {
            FmtBtn::Bold => t.bold,
            FmtBtn::Italic => t.italic,
            FmtBtn::Underline => t.underline,
            FmtBtn::Strike => t.strike,
            FmtBtn::Size(sz) => t.size == *sz,
            FmtBtn::Markdown => !t.plain,
            _ => false,
        });
        if active || hot {
            if let Some(p) = rounded_rect(x, y, w, h, 8.0 * s) {
                let fill = if active { c.accent[0] } else { c.hover };
                pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
            }
        }
        let ink = if active {
            WHITE
        } else if hot {
            c.text
        } else {
            c.secondary
        };
        let (label, face, px, deco) = match btn {
            FmtBtn::Bold => ("B", Face::Bold, 15.0, (false, false, false)),
            FmtBtn::Italic => ("I", Face::Regular, 15.0, (true, false, false)),
            FmtBtn::Underline => ("U", Face::Regular, 15.0, (false, true, false)),
            FmtBtn::Strike => ("S", Face::Regular, 15.0, (false, false, true)),
            FmtBtn::Size(sz) => {
                let px = match sz {
                    crate::board::TextSize::Small => 10.0,
                    crate::board::TextSize::Medium => 13.0,
                    crate::board::TextSize::Heading => 16.0,
                    crate::board::TextSize::Title => 20.0,
                };
                ("A", Face::Bold, px, (false, false, false))
            }
            FmtBtn::Markdown => ("MD", Face::Bold, 11.0, (false, false, false)),
            FmtBtn::Code => ("<>", Face::Bold, 12.0, (false, false, false)),
            FmtBtn::Heading => ("H", Face::Bold, 15.0, (false, false, false)),
            FmtBtn::Bullet | FmtBtn::Task => {
                draw_list_icon(pm, v, *btn == FmtBtn::Task, cx, cy, ink);
                continue;
            }
            FmtBtn::Color(_) => continue,
        };
        let px = px * s;
        let tw = fonts.text_width(face, label, px);
        draw_run(pm, v, fonts, face, px, cx - tw / 2.0, cy, label, ink, deco);
    }
}

/// The rubber-band rectangle while dragging on empty canvas.
fn draw_marquee(pm: &mut PixmapMut, v: &View, scene: &Scene, c: &Colors) {
    let Some((x, y, w, h)) = scene.editor.marquee_rect() else {
        return;
    };
    let (x, y, w, h) = v.rect(LRect::new(x, y, w, h));
    let Some(p) = rounded_rect(x, y, w, h, 2.0 * v.z) else {
        return;
    };
    let tf = Transform::identity();
    pm.fill_path(
        &p,
        &paint(c.accent[0], 40, true),
        FillRule::Winding,
        tf,
        None,
    );
    let st = SkStroke {
        width: v.hair(),
        ..SkStroke::default()
    };
    pm.stroke_path(&p, &paint(c.accent[0], 0xff, true), &st, tf, None);
}

/// Outline and resize handles of the selected item.
fn draw_selection(pm: &mut PixmapMut, v: &View, fonts: &Fonts, scene: &Scene, c: &Colors) {
    let Some(sel) = scene.editor.selected else {
        return;
    };
    let adv = |s: Style, ch: char| fonts.advance(s.face(), ch, s.size());
    let tf = Transform::identity();
    let accent = c.accent[0];
    // Outline every other item in the multi-selection or in the selected item's lock group.
    let g = scene.board.group_of(sel);
    let mut others: Vec<Sel> = scene.editor.extra.clone();
    others.extend(
        scene
            .board
            .group_members(g)
            .into_iter()
            .filter(|m| *m != sel),
    );
    for o in others {
        if let Some(r) = crate::layout::selection_rect(scene.board, o, &adv) {
            let (x, y, w, h) = v.rect(r);
            if let Some(p) = rounded_rect(
                x - 3.0 * v.z,
                y - 3.0 * v.z,
                w + 6.0 * v.z,
                h + 6.0 * v.z,
                8.0 * v.z,
            ) {
                let st = SkStroke {
                    width: v.hair(),
                    dash: StrokeDash::new(vec![6.0 * v.z, 4.0 * v.z], 0.0),
                    ..SkStroke::default()
                };
                pm.stroke_path(&p, &paint(accent, 0xff, true), &st, tf, None);
            }
        }
    }
    // A thin outline around cards, notes and texts (shapes already show their own edge).
    if !matches!(sel, Sel::Shape(_) | Sel::Connector(_)) && scene.editor.editing.is_none() {
        if let Some(r) = crate::layout::selection_rect(scene.board, sel, &adv) {
            let (x, y, w, h) = v.rect(r);
            if let Some(p) = rounded_rect(
                x - 3.0 * v.z,
                y - 3.0 * v.z,
                w + 6.0 * v.z,
                h + 6.0 * v.z,
                8.0 * v.z,
            ) {
                let st = SkStroke {
                    width: v.hair(),
                    ..SkStroke::default()
                };
                pm.stroke_path(&p, &paint(accent, 0xff, true), &st, tf, None);
            }
        }
    }
    let s = 10.0 * v.scale;
    for (_, (hx, hy)) in handles(scene.board, sel, &adv) {
        let (px, py) = (v.x(hx), v.y(hy));
        if let Some(p) = rounded_rect(px - s / 2.0, py - s / 2.0, s, s, 2.5 * v.scale) {
            pm.fill_path(&p, &paint(WHITE, 0xff, true), FillRule::Winding, tf, None);
            let st = SkStroke {
                width: (1.6 * v.scale).max(1.5),
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(accent, 0xff, true), &st, tf, None);
        }
    }
}

// ---- overlay (screen space)

fn pill(pm: &mut PixmapMut, v: &View, r: LRect, c: &Colors) {
    let s = v.scale;
    let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
    let tf = Transform::identity();
    let rad = (h.min(w)) / 2.0;
    if let Some(p) = rounded_rect(x, y + 3.0 * s, w, h, rad) {
        pm.fill_path(
            &p,
            &paint((0, 0, 0), if c.dark { 50 } else { 22 }, true),
            FillRule::Winding,
            tf,
            None,
        );
    }
    if let Some(p) = rounded_rect(x, y, w, h, rad) {
        // Translucent: the canvas shows through slightly.
        pm.fill_path(&p, &paint(c.card, 226, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
}

fn draw_overlay(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, scene: &Scene, c: &Colors) {
    let o = scene.overlay;
    let s = v.scale;
    let tf = Transform::identity();
    let full = (0.0, v.w as f32);

    // Toolbar.
    pill(pm, v, o.tools_pill, c);
    for (tool, r) in &o.tools {
        let active = scene.editor.tool == *tool;
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let hot = !active && scene.hover == Hover::Tool(*tool);
        if active || hot {
            if let Some(p) = rounded_rect(x, y, w, h, 10.0 * s) {
                let fill = if active { c.accent[0] } else { c.hover };
                pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
            }
        }
        let col = if active {
            WHITE
        } else if hot {
            c.text
        } else {
            c.secondary
        };
        draw_tool_icon(pm, v, *tool, x + w / 2.0, y + h / 2.0, col);
    }

    // View menu (right edge): zoom in, zoom out, background colour.
    pill(pm, v, o.view_pill, c);
    for (btn, rc) in &o.view_btns {
        let (cx, cy) = ((rc.x + rc.w / 2.0) * s, (rc.y + rc.h / 2.0) * s);
        let open = *btn == ViewBtn::Theme && scene.theme_open;
        let hot = scene.hover == Hover::View(*btn);
        if open || hot {
            if let Some(p) = rounded_rect(rc.x * s, rc.y * s, rc.w * s, rc.h * s, 10.0 * s) {
                let fill = if open { c.accent[0] } else { c.hover };
                pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
            }
        }
        let ic = if open {
            WHITE
        } else if hot {
            c.text
        } else {
            c.secondary
        };
        match btn {
            ViewBtn::Theme => draw_theme_icon(pm, v, cx, cy, ic),
            ViewBtn::Help => draw_help_icon(pm, v, fonts, cx, cy, ic),
            ViewBtn::Background => {
                // A swatch of the current background: wallpaper shows as the accent colour.
                let fill = if scene.background.image_on {
                    c.accent[0]
                } else {
                    scene.background.solid().unwrap_or(c.canvas)
                };
                draw_swatch(pm, v, cx, cy, fill, ic);
            }
            _ => draw_zoom_icon(pm, v, *btn, cx, cy, ic),
        }
    }

    // Area chips.
    if let Some(p) = o.chips_pill {
        pill(pm, v, p, c);
    }
    for (ai, r) in &o.chips {
        let Some(a) = scene.board.spaces.get(*ai) else {
            continue;
        };
        let current = scene.current_area == Some(*ai);
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let px = Style::ChipBold.size() * s;
        let tw = fonts.text_width(Face::Bold, &a.name, px);
        let hot = !current && scene.hover == Hover::Chip(*ai);
        if current || hot {
            if let Some(p) = rounded_rect(x, y, w, h, h / 2.0) {
                let fill = if current { c.accent[*ai % 10] } else { c.hover };
                pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
            }
        }
        let col = if current {
            WHITE
        } else if hot {
            c.text
        } else {
            c.secondary
        };
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            px,
            x + (w - tw) / 2.0,
            y + h / 2.0,
            &a.name,
            col,
            full,
            false,
        );
    }
    // The "+" button (adds a space) and chevrons when the menu is scrolled.
    if let Some(r) = o.plus {
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        if scene.hover == Hover::AddSpace {
            if let Some(p) = rounded_rect(x, y, w, h, h / 2.0) {
                pm.fill_path(&p, &paint(c.hover, 0xff, true), FillRule::Winding, tf, None);
            }
        }
        let col = if scene.hover == Hover::AddSpace {
            c.text
        } else {
            c.secondary
        };
        let (cx, cy, arm) = (x + w / 2.0, y + h / 2.0, 6.0 * s);
        let mut pb = PathBuilder::new();
        pb.move_to(cx - arm, cy);
        pb.line_to(cx + arm, cy);
        pb.move_to(cx, cy - arm);
        pb.line_to(cx, cy + arm);
        if let Some(p) = pb.finish() {
            let st = SkStroke {
                width: 1.8 * s,
                line_cap: tiny_skia::LineCap::Round,
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(col, 0xff, true), &st, tf, None);
        }
    }
    if let Some(p) = o.chips_pill {
        for (show, left) in [(o.more_left, true), (o.more_right, false)] {
            if show {
                let (glyph, x) = if left {
                    ("‹", p.x - 14.0)
                } else {
                    ("›", p.right() + 4.0)
                };
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Bold,
                    18.0 * s,
                    x * s,
                    (p.y + p.h / 2.0) * s,
                    glyph,
                    c.secondary,
                    full,
                    false,
                );
            }
        }
    }
    if scene.theme_open {
        draw_theme_panel(pm, v, fonts, scene, c);
    }
    if let Some(d) = scene.detail {
        draw_detail(pm, v, fonts, c, d);
    }
    if let Some(n) = scene.name_box {
        draw_name_box(pm, v, fonts, c, n);
    }
    if scene.show_help {
        draw_help(pm, v, fonts, c);
    }
}

/// A palette glyph: a ring with the right half filled, centred on (cx, cy).
fn draw_theme_icon(pm: &mut PixmapMut, v: &View, cx: f32, cy: f32, color: Rgb) {
    let u = v.scale;
    let tf = Transform::identity();
    let r = 9.0 * u;
    if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
        let st = SkStroke {
            width: 1.8 * u,
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(color, 0xff, true), &st, tf, None);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    pb.cubic_to(cx + r * 0.55, cy - r, cx + r, cy - r * 0.55, cx + r, cy);
    pb.cubic_to(cx + r, cy + r * 0.55, cx + r * 0.55, cy + r, cx, cy + r);
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(&p, &paint(color, 0xff, true), FillRule::Winding, tf, None);
    }
}

/// The theme picker: one row per theme with a live miniature (that theme's canvas, a card, an
/// accent dot and its edge/shadow style), its name and tagline.
fn draw_theme_panel(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, scene: &Scene, c: &Colors) {
    let o = scene.overlay;
    let s = v.scale;
    let tf = Transform::identity();
    let full = (0.0, v.w as f32);
    let pr = o.theme_panel;
    let (px, py, pw, ph) = (pr.x * s, pr.y * s, pr.w * s, pr.h * s);
    if let Some(p) = rounded_rect(px, py + 4.0 * s, pw, ph, 18.0 * s) {
        pm.fill_path(
            &p,
            &paint((0, 0, 0), if c.dark { 70 } else { 30 }, true),
            FillRule::Winding,
            tf,
            None,
        );
    }
    if let Some(p) = rounded_rect(px, py, pw, ph, 18.0 * s) {
        pm.fill_path(&p, &paint(c.card, 244, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
    text_px(
        pm,
        v,
        fonts,
        Face::Bold,
        15.0 * s,
        px + 16.0 * s,
        py + 20.0 * s,
        "Theme",
        c.text,
        full,
        false,
    );
    for (t, r) in &o.theme_items {
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let current = scene.theme == *t;
        let hot = scene.hover == Hover::Theme(*t);
        if current || hot {
            if let Some(p) = rounded_rect(x, y, w, h, 12.0 * s) {
                let fill = if current {
                    mix(c.card, c.accent[0], 0.22)
                } else {
                    c.hover
                };
                pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
                if current {
                    let st = SkStroke {
                        width: 1.5 * s,
                        ..SkStroke::default()
                    };
                    pm.stroke_path(&p, &paint(c.accent[0], 0xff, true), &st, tf, None);
                }
            }
        }
        // Miniature. Hovering lifts it slightly.
        let tc = colors(*t);
        let lift = if hot { -1.5 * s } else { 0.0 };
        let (mx, my, mw, mh) = (x + 8.0 * s, y + 7.0 * s + lift, 40.0 * s, h - 14.0 * s);
        if let Some(p) = rounded_rect(mx, my, mw, mh, 8.0 * s) {
            pm.fill_path(
                &p,
                &paint(tc.canvas, 0xff, true),
                FillRule::Winding,
                tf,
                None,
            );
            if let Some(cols) = tc.aurora {
                if let Some(g) = PathBuilder::from_circle(mx + mw * 0.28, my + mh * 0.3, mh * 0.2) {
                    pm.fill_path(&g, &paint(cols[0], 120, true), FillRule::Winding, tf, None);
                }
                if let Some(g) = PathBuilder::from_circle(mx + mw * 0.82, my + mh * 0.72, mh * 0.18)
                {
                    pm.fill_path(&g, &paint(cols[1], 110, true), FillRule::Winding, tf, None);
                }
            }
            let st = SkStroke {
                width: v.hair(),
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
        }
        let (cx0, cy0, cw0, ch0) = (mx + 6.0 * s, my + 6.0 * s, mw - 12.0 * s, mh - 12.0 * s);
        if tc.shadow == Shadow::Hard {
            if let Some(p) = rounded_rect(cx0 + 2.0 * s, cy0 + 2.0 * s, cw0, ch0, 3.0 * s) {
                pm.fill_path(
                    &p,
                    &paint((0, 0, 0), 0xff, true),
                    FillRule::Winding,
                    tf,
                    None,
                );
            }
        }
        if let Some(p) = rounded_rect(cx0, cy0, cw0, ch0, 4.0 * s * tc.radius_scale.max(0.5)) {
            pm.fill_path(&p, &paint(tc.card, 0xff, true), FillRule::Winding, tf, None);
            let edge = tc.edge.unwrap_or(tc.sep);
            let st = SkStroke {
                width: (v.hair() * tc.border_scale).max(1.0),
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(edge, 0xff, true), &st, tf, None);
        }
        if let Some(p) = PathBuilder::from_circle(cx0 + 5.0 * s, cy0 + ch0 / 2.0, 2.5 * s) {
            pm.fill_path(
                &p,
                &paint(tc.accent[0], 0xff, true),
                FillRule::Winding,
                tf,
                None,
            );
        }
        // Name and tagline.
        let tx = x + 60.0 * s;
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            14.0 * s,
            tx,
            y + h * 0.34,
            t.label(),
            c.text,
            full,
            false,
        );
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            11.0 * s,
            tx,
            y + h * 0.68,
            t.tagline(),
            c.secondary,
            full,
            false,
        );
    }
}

const HELP_LEFT: [(&str, &str); 22] = [
    ("V", "Select"),
    ("G", "Group card tool"),
    ("T", "Todo tool"),
    ("N", "Note tool"),
    ("S", "Sticker tool"),
    ("M", "Paste image"),
    ("X", "Text tool"),
    ("R / O / I", "Rectangle / ellipse / line"),
    ("P / E", "Pen / eraser"),
    ("1-9", "Jump to a group"),
    ("0", "Fit everything"),
    ("+ / -", "Zoom in / out"),
    ("C", "Capture"),
    ("L", "Light / dark"),
    ("Shift+L", "Theme picker"),
    ("B", "Cycle background / wallpaper"),
    ("W", "Wallpaper rotation"),
    ("F / Shift+F", "Wallpaper folder: pick / open"),
    ("Ctrl+B / I / U", "Bold / italic / underline"),
    ("Ctrl+Shift+S / Ctrl+E", "Strike-through / code"),
    ("Ctrl+[ / ]", "Text: smaller / bigger"),
    ("?", "This help"),
];
const HELP_RIGHT: [(&str, &str); 22] = [
    ("Shift+click", "Add to selection"),
    ("Ctrl+G", "Lock selection together"),
    ("Ctrl+Shift+G", "Unlock group"),
    ("Delete", "Remove selected"),
    ("Esc", "Deselect / close"),
    ("Ctrl+C / Ctrl+X", "Copy / cut text or text box"),
    ("Ctrl+V", "Paste text box / text / PNG"),
    ("Ctrl+Z", "Undo"),
    ("Ctrl+Y", "Redo"),
    ("Ctrl+Enter", "Toggle todo done"),
    ("Ctrl+P", "Cycle priority"),
    ("Ctrl+D", "Todo details / duplicate text"),
    ("Drag text grip", "Move text, even while typing"),
    ("Ctrl+Left/Right", "Previous / next space"),
    ("Double-click", "Edit / new group"),
    ("Right-click", "Remove item"),
    ("Space+drag / mid-drag", "Pan (also scroll / arrows)"),
    ("Drag empty space", "Select items to group/delete"),
    ("Top menu +", "New space (dbl-click name: rename)"),
    ("Ctrl+scroll", "Zoom"),
    ("Dbl-click sticker", "Next emoji"),
    ("Ctrl+Q", "Quit"),
];

/// A question mark drawn as text, centred on (cx, cy).
fn draw_help_icon(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, cx: f32, cy: f32, color: Rgb) {
    let px = 20.0 * v.scale;
    let tw = fonts.text_width(Face::Bold, "?", px);
    text_px(
        pm,
        v,
        fonts,
        Face::Bold,
        px,
        cx - tw / 2.0,
        cy,
        "?",
        color,
        (0.0, v.w as f32),
        false,
    );
}

/// The centred keyboard shortcuts popup.
/// The small popup that names a space.
fn draw_name_box(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    c: &Colors,
    n: &crate::detail::NameBox,
) {
    let s = v.scale;
    let tf = Transform::identity();
    let (card, field) = crate::detail::name_panel(v.w as f32 / s, v.h as f32 / s);
    if let Some(p) = rounded_rect(0.0, 0.0, v.w as f32, v.h as f32, 0.0) {
        pm.fill_path(
            &p,
            &paint((0, 0, 0), 110, true),
            FillRule::Winding,
            tf,
            None,
        );
    }
    if let Some(p) = rounded_rect(card.0 * s, card.1 * s, card.2 * s, card.3 * s, 16.0 * s) {
        pm.fill_path(&p, &paint(c.card, 0xff, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
    let full = (0.0, v.w as f32);
    text_px(
        pm,
        v,
        fonts,
        Face::Bold,
        17.0 * s,
        field.0 * s,
        (card.1 + 34.0) * s,
        n.title(),
        c.text,
        full,
        false,
    );
    let (x, y, w, h) = field;
    if let Some(p) = rounded_rect(x * s, y * s, w * s, h * s, 8.0 * s) {
        pm.fill_path(&p, &paint(c.hover, 0xff, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: 2.0 * s,
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.accent[0], 0xff, true), &st, tf, None);
    }
    let size = 14.0 * s;
    let clip = ((x + 10.0) * s, (x + w - 10.0) * s);
    let cy = (y + h / 2.0) * s;
    let pre: f32 = n
        .text
        .chars()
        .take(n.caret)
        .map(|ch| fonts.advance(Face::Regular, ch, size))
        .sum();
    let scroll = scroll_x((w - 20.0) * s, pre);
    if n.fresh && !n.text.is_empty() {
        // the suggested name is shown selected
        let tw = fonts
            .text_width(Face::Regular, &n.text, size)
            .min(clip.1 - clip.0);
        if let Some(p) = rounded_rect(
            clip.0 - 2.0 * s,
            cy - 10.0 * s,
            tw + 4.0 * s,
            20.0 * s,
            4.0 * s,
        ) {
            pm.fill_path(
                &p,
                &paint(c.accent[0], 70, true),
                FillRule::Winding,
                tf,
                None,
            );
        }
    }
    text_px(
        pm,
        v,
        fonts,
        Face::Regular,
        size,
        clip.0 - scroll,
        cy,
        &n.text,
        c.text,
        clip,
        false,
    );
    draw_caret(pm, v, clip.0 + pre - scroll, cy, 16.0 * s, c.text);
    text_px(
        pm,
        v,
        fonts,
        Face::Regular,
        12.0 * s,
        field.0 * s,
        (card.1 + card.3 - 18.0) * s,
        "Enter save · Esc cancel",
        c.tertiary,
        full,
        false,
    );
}

/// The todo detail page: dimmed backdrop, card, and three fields (title, description, due date).
fn draw_detail(
    pm: &mut PixmapMut,
    v: &View,
    fonts: &mut Fonts,
    c: &Colors,
    d: &crate::detail::Detail,
) {
    use crate::detail::{self as dt, Field};
    let s = v.scale;
    let tf = Transform::identity();
    let (vw, vh) = (v.w as f32 / s, v.h as f32 / s);
    let pn = dt::panel(vw, vh);
    if let Some(p) = rounded_rect(0.0, 0.0, v.w as f32, v.h as f32, 0.0) {
        pm.fill_path(
            &p,
            &paint((0, 0, 0), 110, true),
            FillRule::Winding,
            tf,
            None,
        );
    }
    let (cx, cy, cw, ch) = pn.card;
    if let Some(p) = rounded_rect(cx * s, cy * s, cw * s, ch * s, 16.0 * s) {
        pm.fill_path(&p, &paint(c.card, 0xff, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
    let full = (0.0, v.w as f32);
    text_px(
        pm,
        v,
        fonts,
        Face::Bold,
        17.0 * s,
        (cx + dt::PANEL_PAD) * s,
        (cy + dt::PANEL_PAD + 10.0) * s,
        "Todo details",
        c.text,
        full,
        false,
    );
    let size = 14.0 * s;
    let line_h = 20.0;
    let today = dt::today();
    for (field, rect, label) in [
        (Field::Title, pn.title, "Title"),
        (Field::Desc, pn.desc, "Description"),
        (Field::Due, pn.due, "Due date (YYYY-MM-DD)"),
    ] {
        let (x, y, w, h) = rect;
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            12.0 * s,
            x * s,
            (y - dt::LABEL_H / 2.0) * s,
            label,
            c.secondary,
            full,
            false,
        );
        let active = d.field == field;
        let bad = field == Field::Due && !d.due_valid();
        let border = if bad {
            c.accent[3]
        } else if active {
            c.accent[0]
        } else {
            c.sep
        };
        if let Some(p) = rounded_rect(x * s, y * s, w * s, h * s, 8.0 * s) {
            pm.fill_path(&p, &paint(c.hover, 0xff, true), FillRule::Winding, tf, None);
            let st = SkStroke {
                width: if active { 2.0 * s } else { v.hair() },
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(border, 0xff, true), &st, tf, None);
        }
        let text = d.text(field);
        let inner_w = (w - 20.0) * s;
        let clip = ((x + 10.0) * s, (x + w - 10.0) * s);
        let chars: Vec<char> = text.chars().collect();
        // Widths are looked up once so the closure does not hold `fonts` while text is drawn.
        let widths: std::collections::HashMap<char, f32> = chars
            .iter()
            .map(|&ch| (ch, fonts.advance(Face::Regular, ch, size)))
            .collect();
        let adv = |ch: char| widths.get(&ch).copied().unwrap_or(0.0);
        if field == Field::Desc {
            let lines = crate::text::wrap(&chars, inner_w, &adv);
            let rows_fit = ((h - 12.0) / line_h).floor().max(1.0) as usize;
            let caret_line = if active { line_of(&lines, d.caret) } else { 0 };
            let first = (caret_line + 1).saturating_sub(rows_fit);
            if chars.is_empty() && !active {
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    size,
                    clip.0,
                    (y + 6.0 + line_h / 2.0) * s,
                    "Add details…",
                    c.tertiary,
                    clip,
                    false,
                );
            }
            let md = (!active && crate::md::has_markup(text, true))
                .then(|| crate::md::char_styles(text, true));
            for (i, ln) in lines.iter().enumerate().skip(first).take(rows_fit) {
                let t: String = chars[ln.start..ln.end]
                    .iter()
                    .filter(|c| **c != '\n')
                    .collect();
                let ly = (y + 6.0 + (i - first) as f32 * line_h + line_h / 2.0) * s;
                if let Some(cs) = &md {
                    let (a0, b0) = (ln.start, ln.end.min(cs.len()));
                    draw_md_chars(
                        pm, v, fonts, Face::Regular, size, clip.0, ly, &chars[a0..b0], &cs[a0..b0],
                        c.text, clip, false, c,
                    );
                    continue;
                }
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    size,
                    clip.0,
                    ly,
                    &t,
                    c.text,
                    clip,
                    false,
                );
                if active && i == caret_line {
                    let pre: f32 = chars[ln.start..d.caret.clamp(ln.start, ln.end)]
                        .iter()
                        .map(|&ch| adv(ch))
                        .sum();
                    draw_caret(pm, v, clip.0 + pre, ly, line_h * s * 0.8, c.text);
                }
            }
        } else {
            let cy_mid = (y + h / 2.0) * s;
            let pre: f32 = chars[..d.caret.min(chars.len())]
                .iter()
                .map(|&ch| adv(ch))
                .sum::<f32>();
            let scroll = if active { scroll_x(inner_w, pre) } else { 0.0 };
            if chars.is_empty() {
                let hint = if field == Field::Due { "none" } else { "" };
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    size,
                    clip.0,
                    cy_mid,
                    hint,
                    c.tertiary,
                    clip,
                    false,
                );
            } else if !active && field == Field::Title && crate::md::has_markup(text, false) {
                let cs = crate::md::char_styles(text, false);
                draw_md_chars(
                    pm, v, fonts, Face::Regular, size, clip.0, cy_mid, &chars, &cs, c.text, clip,
                    false, c,
                );
            } else {
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    size,
                    clip.0 - scroll,
                    cy_mid,
                    text,
                    c.text,
                    clip,
                    false,
                );
            }
            if active {
                draw_caret(
                    pm,
                    v,
                    clip.0 + pre - scroll,
                    cy_mid,
                    line_h * s * 0.8,
                    c.text,
                );
            }
        }
    }
    // Footer: what the due date means today, and the keys.
    let (fx, fy) = (
        pn.card.0 + dt::PANEL_PAD,
        pn.card.1 + pn.card.3 - dt::FOOT_H + 6.0,
    );
    let note = match dt::days_until(d.due.trim(), today) {
        _ if !d.due_valid() => "Not a valid date".to_string(),
        Some(n) if n < 0 => format!("Overdue by {} day(s)", -n),
        Some(0) => "Due today".to_string(),
        Some(n) => format!("Due in {n} day(s)"),
        None => String::new(),
    };
    text_px(
        pm,
        v,
        fonts,
        Face::Regular,
        12.0 * s,
        fx * s,
        fy * s,
        "Tab next · Enter/Ctrl+Enter save · Esc cancel",
        c.tertiary,
        full,
        false,
    );
    if !note.is_empty() {
        let late = !d.due_valid() || dt::overdue(d.due.trim(), today);
        let col = if late { c.accent[3] } else { c.secondary };
        let w = fonts.text_width(Face::Regular, &note, 12.0 * s);
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            12.0 * s,
            (cx + cw - dt::PANEL_PAD) * s - w,
            fy * s,
            &note,
            col,
            full,
            false,
        );
    }
}

fn draw_help(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, c: &Colors) {
    let s = v.scale;
    let tf = Transform::identity();
    let (vw, vh) = (v.w as f32 / s, v.h as f32 / s);
    let (col_w, row_h, pad) = (300.0_f32, 26.0_f32, 24.0_f32);
    let pw = (2.0 * col_w + 3.0 * pad).min(vw - 20.0);
    let ph =
        (HELP_LEFT.len().max(HELP_RIGHT.len()) as f32 * row_h + 2.0 * pad + 40.0).min(vh - 20.0);
    let (px, py) = ((vw - pw) / 2.0, (vh - ph) / 2.0);
    if let Some(p) = rounded_rect(0.0, 0.0, v.w as f32, v.h as f32, 0.0) {
        pm.fill_path(&p, &paint((0, 0, 0), 90, true), FillRule::Winding, tf, None);
    }
    if let Some(p) = rounded_rect(px * s, py * s, pw * s, ph * s, 16.0 * s) {
        pm.fill_path(&p, &paint(c.card, 0xff, true), FillRule::Winding, tf, None);
        let st = SkStroke {
            width: v.hair(),
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(c.sep, 0xff, true), &st, tf, None);
    }
    let full = (0.0, v.w as f32);
    let size = 14.0 * s;
    text_px(
        pm,
        v,
        fonts,
        Face::Bold,
        17.0 * s,
        (px + pad) * s,
        (py + pad + 10.0) * s,
        "Keyboard shortcuts",
        c.text,
        full,
        false,
    );
    for (ci, list) in [&HELP_LEFT[..], &HELP_RIGHT[..]].into_iter().enumerate() {
        let x = px + pad + ci as f32 * (col_w + pad);
        for (i, (k, d)) in list.iter().enumerate() {
            let y = py + pad + 44.0 + i as f32 * row_h;
            text_px(
                pm,
                v,
                fonts,
                Face::Bold,
                size,
                x * s,
                y * s,
                k,
                c.accent[0],
                full,
                false,
            );
            text_px(
                pm,
                v,
                fonts,
                Face::Regular,
                size,
                (x + 112.0) * s,
                y * s,
                d,
                c.secondary,
                full,
                false,
            );
        }
    }
}

/// The background button: a ring around a filled swatch of the current background colour.
fn draw_swatch(pm: &mut PixmapMut, v: &View, cx: f32, cy: f32, fill: Rgb, ring: Rgb) {
    let u = v.scale;
    let tf = Transform::identity();
    if let Some(p) = PathBuilder::from_circle(cx, cy, 7.0 * u) {
        pm.fill_path(&p, &paint(fill, 0xff, true), FillRule::Winding, tf, None);
    }
    if let Some(p) = PathBuilder::from_circle(cx, cy, 9.5 * u) {
        let st = SkStroke {
            width: 1.8 * u,
            ..SkStroke::default()
        };
        pm.stroke_path(&p, &paint(ring, 0xff, true), &st, tf, None);
    }
}

/// A magnifier with a plus (zoom in) or a minus (zoom out), in a 24x24 box centred on (cx, cy).
fn draw_zoom_icon(pm: &mut PixmapMut, v: &View, btn: ViewBtn, cx: f32, cy: f32, color: Rgb) {
    let u = v.scale;
    let tf = Transform::identity();
    let stroke = SkStroke {
        width: 1.8 * u,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..SkStroke::default()
    };
    // Lens centred a little up-left, handle to the lower right.
    let (lx, ly, r) = (cx - 2.0 * u, cy - 2.0 * u, 7.5 * u);
    if let Some(p) = PathBuilder::from_circle(lx, ly, r) {
        pm.stroke_path(&p, &paint(color, 0xff, true), &stroke, tf, None);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(lx + r * 0.72, ly + r * 0.72);
    pb.line_to(cx + 9.0 * u, cy + 9.0 * u);
    // the horizontal bar of the +/- inside the lens
    pb.move_to(lx - 3.6 * u, ly);
    pb.line_to(lx + 3.6 * u, ly);
    if btn == ViewBtn::ZoomIn {
        pb.move_to(lx, ly - 3.6 * u);
        pb.line_to(lx, ly + 3.6 * u);
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
    }
}

/// Simple line icons, drawn in a 24x24 box centred on (cx, cy).
fn draw_tool_icon(pm: &mut PixmapMut, v: &View, tool: Tool, cx: f32, cy: f32, color: Rgb) {
    let s = v.scale;
    let u = s; // one icon unit in buffer px
    let (ox, oy) = (cx - 12.0 * u, cy - 12.0 * u);
    let p = |x: f32, y: f32| (ox + x * u, oy + y * u);
    let tf = Transform::identity();
    let stroke = SkStroke {
        width: 1.8 * u,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..SkStroke::default()
    };
    let line = |pm: &mut PixmapMut, pts: &[(f32, f32)], close: bool| {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let (px, py) = p(x, y);
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
        if close {
            pb.close();
        }
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
        }
    };
    match tool {
        Tool::Select => {
            let mut pb = PathBuilder::new();
            for (i, &(x, y)) in [
                (6.0, 3.0),
                (6.0, 19.0),
                (10.5, 15.0),
                (13.5, 21.0),
                (16.0, 20.0),
                (13.0, 14.0),
                (19.0, 14.0),
            ]
            .iter()
            .enumerate()
            {
                let (px, py) = p(x, y);
                if i == 0 {
                    pb.move_to(px, py);
                } else {
                    pb.line_to(px, py);
                }
            }
            pb.close();
            if let Some(path) = pb.finish() {
                pm.fill_path(
                    &path,
                    &paint(color, 0xff, true),
                    FillRule::Winding,
                    tf,
                    None,
                );
            }
        }
        Tool::Group => {
            if let Some(path) =
                rounded_rect(ox + 4.0 * u, oy + 5.0 * u, 16.0 * u, 14.0 * u, 3.5 * u)
            {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
            line(pm, &[(4.0, 10.0), (20.0, 10.0)], false);
        }
        Tool::Todo => {
            let (px, py) = p(12.0, 12.0);
            if let Some(path) = PathBuilder::from_circle(px, py, 8.0 * u) {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
            line(pm, &[(8.5, 12.3), (11.0, 14.8), (15.8, 9.4)], false);
        }
        Tool::Note => {
            line(
                pm,
                &[
                    (5.0, 4.0),
                    (15.0, 4.0),
                    (19.0, 8.0),
                    (19.0, 20.0),
                    (5.0, 20.0),
                ],
                true,
            );
            line(pm, &[(15.0, 4.0), (15.0, 8.0), (19.0, 8.0)], false);
        }
        Tool::Pen => {
            line(
                pm,
                &[
                    (17.5, 5.5),
                    (19.5, 7.5),
                    (9.0, 18.0),
                    (5.0, 19.0),
                    (6.0, 15.0),
                ],
                true,
            );
            line(pm, &[(15.0, 8.0), (17.0, 10.0)], false);
        }
        Tool::Text => {
            // a capital T
            line(pm, &[(6.0, 6.5), (18.0, 6.5)], false);
            line(pm, &[(12.0, 6.5), (12.0, 19.0)], false);
        }
        Tool::Sticker => {
            // A smiley face.
            let (px, py) = p(12.0, 12.0);
            if let Some(path) = PathBuilder::from_circle(px, py, 8.0 * u) {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
            line(
                pm,
                &[(9.0, 14.0), (10.5, 15.5), (13.5, 15.5), (15.0, 14.0)],
                false,
            );
            line(pm, &[(9.5, 10.0), (9.5, 10.2)], false);
            line(pm, &[(14.5, 10.0), (14.5, 10.2)], false);
        }
        Tool::Image => {
            // A framed picture: a sun and a hill.
            if let Some(path) =
                rounded_rect(ox + 4.0 * u, oy + 5.0 * u, 16.0 * u, 14.0 * u, 2.5 * u)
            {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
            line(
                pm,
                &[
                    (5.0, 17.0),
                    (10.0, 12.0),
                    (13.5, 15.0),
                    (16.0, 13.0),
                    (19.0, 16.5),
                ],
                false,
            );
            line(pm, &[(15.5, 9.0), (15.6, 9.1)], false);
        }
        Tool::Rect => {
            if let Some(path) =
                rounded_rect(ox + 4.5 * u, oy + 6.0 * u, 15.0 * u, 12.0 * u, 2.5 * u)
            {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
        }
        Tool::Ellipse => {
            if let Some(path) = Rect::from_xywh(ox + 4.0 * u, oy + 6.0 * u, 16.0 * u, 12.0 * u)
                .and_then(PathBuilder::from_oval)
            {
                pm.stroke_path(&path, &paint(color, 0xff, true), &stroke, tf, None);
            }
        }
        Tool::Line => line(pm, &[(5.5, 18.5), (18.5, 5.5)], false),
        Tool::Connector => {
            // An elbow arrow with a head.
            line(pm, &[(5.0, 18.0), (5.0, 8.0), (17.0, 8.0)], false);
            line(pm, &[(13.5, 4.5), (18.0, 8.0), (13.5, 11.5)], false);
        }
        Tool::Eraser => {
            line(
                pm,
                &[
                    (5.0, 15.0),
                    (13.0, 6.0),
                    (19.5, 11.5),
                    (12.0, 19.5),
                    (8.5, 19.5),
                ],
                true,
            );
            line(pm, &[(9.0, 10.5), (15.5, 16.0)], false);
        }
    }
}

// ---- wallpaper

/// Decode a PNG and scale it to cover `w` x `h`, darken it by `dim`, and return it in the
/// shm buffer's byte order (B, G, R, 0xFF), ready to be copied into the canvas.
pub fn build_wallpaper(path: &str, w: u32, h: u32, dim: f32) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        let kind = if bytes.starts_with(&[0xff, 0xd8]) {
            "JPEG"
        } else {
            "an unknown format"
        };
        return Err(format!(
            "{path} is {kind}; only PNG wallpapers are supported (convert it to PNG)"
        ));
    }
    let src = Pixmap::decode_png(&bytes).map_err(|e| format!("cannot decode {path}: {e}"))?;
    drop(bytes);
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    // "Cover" fit: fill the screen, crop the overflow, centred.
    let k = (w as f32 / sw).max(h as f32 / sh);
    let (tx, ty) = ((w as f32 - sw * k) / 2.0, (h as f32 - sh * k) / 2.0);
    let mut dst = Pixmap::new(w, h).ok_or("bad output size")?;
    let quality = if k < 0.5 {
        FilterQuality::Bicubic
    } else {
        FilterQuality::Bilinear
    };
    let pp = PixmapPaint {
        quality,
        ..PixmapPaint::default()
    };
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &pp,
        Transform::from_row(k, 0.0, 0.0, k, tx, ty),
        None,
    );
    drop(src);
    let a = (dim.clamp(0.0, 0.9) * 255.0) as u8;
    if let Some(r) = Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
        let mut black = Paint::default();
        black.set_color_rgba8(0, 0, 0, a);
        dst.fill_rect(r, &black, Transform::identity(), None);
    }
    let mut data = dst.take();
    for px in data.as_chunks_mut::<4>().0 {
        *px = [px[2], px[1], px[0], 0xff]; // RGBA -> BGRX
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Todo;
    use crate::editor::{Edit, EditTarget};
    use crate::layout::{overlay, Tool};

    fn fonts() -> Fonts {
        Fonts::load().expect("embedded fonts load").0
    }

    /// Read the pixel at (x, y) as on-screen (r, g, b), undoing the Xrgb byte order.
    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> Rgb {
        let i = ((y * w + x) * 4) as usize;
        (buf[i + 2], buf[i + 1], buf[i])
    }

    struct Rig {
        board: Board,
        editor: Editor,
        theme: Theme,
        bg: Background,
        wp: Option<Vec<u8>>,
        cam: Camera,
        w: u32,
        h: u32,
        scale: u32,
    }

    impl Rig {
        fn new() -> Rig {
            Rig {
                board: Board::default_board(),
                editor: Editor::new(),
                theme: Theme::Dark,
                bg: Background::default(),
                wp: None,
                cam: Camera {
                    x: 0.0,
                    y: 0.0,
                    zoom: 1.0,
                },
                w: 1800,
                h: 900,
                scale: 1,
            }
        }
        fn frame(&self) -> Vec<u8> {
            let mut f = fonts();
            let ov = {
                let ff: &Fonts = &f;
                overlay(
                    &self.board,
                    self.w as f32 / self.scale as f32,
                    self.h as f32 / self.scale as f32,
                    crate::layout::Margins::default(),
                    &|s, t| ff.text_width(s.face(), t, s.size()),
                )
            };
            let scene = Scene {
                board: &self.board,
                editor: &self.editor,
                theme: self.theme,
                palette: None,
                background: &self.bg,
                wallpaper: self.wp.as_deref(),
                overlay: &ov,
                current_area: Some(1),
                show_help: false,
                detail: None,
                name_box: None,
                theme_open: false,
                hover: Hover::None,
                text_bar: None,
            };
            let mut buf = vec![0u8; (self.w * self.h * 4) as usize];
            draw(
                &mut buf, self.w, self.h, &self.cam, self.scale, &scene, &mut f,
            );
            buf
        }
        fn geom(&self, ai: usize) -> AreaGeom {
            let f = fonts();
            area_geom(&self.board.areas[ai], &|s, c| {
                f.advance(s.face(), c, s.size())
            })
        }
    }

    /// A point inside a card's bottom padding (no text, clear of the rounded corners),
    /// in screen px for a camera at (-50, -50) and the given scale.
    fn inside(card: LRect, scale: u32) -> (u32, u32) {
        (
            ((card.x + 50.0 + 160.0) * scale as f32) as u32,
            ((card.bottom() + 50.0 - 3.0) * scale as f32) as u32,
        )
    }

    #[test]
    fn spec_colours_dark_and_light() {
        let d = colors(Theme::Dark);
        assert_eq!(d.canvas, (0x14, 0x14, 0x16));
        assert_eq!(d.card, (0x1c, 0x1c, 0x1e));
        assert_eq!(d.hover, (0x2c, 0x2c, 0x2e));
        assert_eq!(d.sep, (0x38, 0x38, 0x3a));
        assert_eq!(d.secondary, (0x98, 0x98, 0x9f));
        assert_eq!(d.tertiary, (0x63, 0x63, 0x66));
        assert_eq!(d.accent[0], (0x0a, 0x84, 0xff));
        assert_eq!(d.accent[1], (0x30, 0xd1, 0x58));
        assert_eq!(d.accent[2], (0xff, 0x9f, 0x0a));
        assert_eq!(d.accent[3], (0xff, 0x45, 0x3a));
        assert_eq!(d.accent[4], (0xff, 0xd6, 0x0a));
        assert_eq!(d.accent[5], (0x40, 0xc8, 0xe0));
        assert_eq!(d.accent[6], (0xbf, 0x5a, 0xf2));
        assert_eq!(d.accent[7], (0xff, 0x37, 0x5f));
        assert_eq!(d.accent[8], (0x5e, 0x5c, 0xe6));
        assert_eq!(d.accent[9], (0x98, 0x98, 0x9d));
        let l = colors(Theme::Light);
        assert_eq!(l.canvas, (0xf2, 0xf2, 0xf7));
        assert_eq!(l.card, (0xff, 0xff, 0xff));
        assert_eq!(l.sep, (0xe5, 0xe5, 0xea));
        assert_eq!(l.accent[0], (0x00, 0x7a, 0xff));
        assert_eq!(l.accent[1], (0x34, 0xc7, 0x59));
        assert_eq!(l.accent[2], (0xff, 0x95, 0x00));
        assert_eq!(l.accent[3], (0xff, 0x3b, 0x30));
    }

    #[test]
    fn canvas_card_and_hero_border_colours() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let buf = r.frame();
        assert_eq!(
            px(&buf, r.w, 5, 880),
            colors(Theme::Dark).canvas,
            "canvas away from dots/cards"
        );
        let inbox = r.geom(0).card;
        let (cx, cy) = inside(inbox, 1);
        assert_eq!(px(&buf, r.w, cx, cy), colors(Theme::Dark).card, "card fill");
        // hero (Focus) has a 2px accent border; the inbox has a hairline separator border
        let focus = r.geom(1).card;
        let edge_x = (focus.x + 50.0 + 400.0 / 2.0) as u32;
        let edge_y = (focus.y + 50.0) as u32; // the 2px border straddles the card edge
        assert_eq!(
            px(&buf, r.w, edge_x, edge_y),
            colors(Theme::Dark).accent[0],
            "hero accent border"
        );
        let inbox_top = (inbox.y + 50.0 + 0.5) as u32;
        let hair = px(&buf, r.w, ((inbox.x + 50.0) + 160.0) as u32, inbox_top);
        let (canvas, sep) = (colors(Theme::Dark).canvas, colors(Theme::Dark).sep);
        assert!(
            hair.0 > canvas.0 && hair.0 <= sep.0,
            "a 1px hairline straddling a pixel edge is a canvas/separator blend: {hair:?}"
        );
    }

    #[test]
    fn theme_blend_hits_both_ends_and_glides_in_between() {
        let (a, b) = (colors(Theme::Dark), colors(Theme::Brutal));
        let start = lerp_colors(&a, &b, 0.0);
        let end = lerp_colors(&a, &b, 1.0);
        assert_eq!((start.canvas, start.shadow), (a.canvas, a.shadow));
        assert_eq!((end.canvas, end.shadow), (b.canvas, b.shadow));
        let mid = lerp_colors(&a, &b, 0.5);
        assert_ne!(mid.canvas, a.canvas);
        assert_ne!(mid.canvas, b.canvas);
        assert!(mid.radius_scale < a.radius_scale && mid.radius_scale > b.radius_scale);
    }

    #[test]
    fn light_theme_changes_canvas_and_card() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        r.theme = Theme::Light;
        let buf = r.frame();
        assert_eq!(px(&buf, r.w, 5, 880), colors(Theme::Light).canvas);
        let inbox = r.geom(0).card;
        assert_eq!(
            px(&buf, r.w, inside(inbox, 1).0, inside(inbox, 1).1),
            (0xff, 0xff, 0xff)
        );
    }

    #[test]
    fn solid_background_replaces_the_canvas_and_dots_follow_its_brightness() {
        let mut r = Rig::new();
        r.board = Board::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        r.bg.color = Some("#14211A".into());
        let buf = r.frame();
        assert_eq!(px(&buf, r.w, 5, 880), (0x14, 0x21, 0x1a));
        // a dot at world origin (screen 50,50): lighter than the dark background
        let dot = px(&buf, r.w, 50, 50);
        assert!(dot.1 > 0x21, "dark bg gets lighter dots: {dot:?}");
        r.bg.color = Some("#E8E2D4".into());
        let buf = r.frame();
        assert_eq!(px(&buf, r.w, 5, 880), (0xe8, 0xe2, 0xd4));
        let dot = px(&buf, r.w, 50, 50);
        assert!(dot.1 < 0xe2, "light bg gets darker dots: {dot:?}");
    }

    #[test]
    fn wallpaper_is_copied_under_the_cards_with_no_dot_grid() {
        let mut r = Rig::new();
        let mut wp = vec![0u8; (r.w * r.h * 4) as usize];
        for p in wp.as_chunks_mut::<4>().0 {
            *p = [0x40, 0x80, 0xc0, 0xff]; // B,G,R,X => on-screen (0xc0,0x80,0x40)
        }
        r.wp = Some(wp);
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let buf = r.frame();
        assert_eq!(px(&buf, r.w, 5, 880), (0xc0, 0x80, 0x40));
        assert_eq!(
            px(&buf, r.w, 50, 880 - 40),
            (0xc0, 0x80, 0x40),
            "no grid dots on a wallpaper"
        );
        let inbox = r.geom(0).card;
        assert_eq!(
            px(&buf, r.w, inside(inbox, 1).0, inside(inbox, 1).1),
            colors(Theme::Dark).card,
            "cards stay opaque on top"
        );
    }

    #[test]
    fn wrong_sized_wallpaper_is_ignored_safely() {
        let mut r = Rig::new();
        r.wp = Some(vec![0xff; 16]);
        let buf = r.frame();
        assert_eq!(px(&buf, r.w, 5, 880), colors(Theme::Dark).canvas);
    }

    #[test]
    fn check_circle_ring_and_filled_state_use_the_area_colour() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        r.board.areas[2].todos = vec![
            Todo::new("open"),
            Todo {
                text: "done".into(),
                done: true,
                priority: 0,
                body: None,
                due: None,
            },
        ];
        let g = r.geom(2);
        let accent = colors(Theme::Dark).accent[r.board.areas[2].color as usize];
        let buf = r.frame();
        let (c0, c1) = (g.rows[0].circle, g.rows[1].circle);
        // open: ring pixel on the left edge, hollow centre shows the card
        let ring = px(
            &buf,
            r.w,
            (c0.x + 50.0 + 1.0) as u32,
            (c0.y + 50.0 + 11.0) as u32,
        );
        assert!(
            ring.2 > 150 && ring != colors(Theme::Dark).card,
            "ring edge is accent-tinted: {ring:?} vs {accent:?}"
        );
        let centre = px(
            &buf,
            r.w,
            (c0.x + 50.0 + 11.0) as u32,
            (c0.y + 50.0 + 3.0) as u32,
        );
        assert_eq!(centre, colors(Theme::Dark).card, "ring is hollow");
        // done: filled with the area colour
        assert_eq!(
            px(
                &buf,
                r.w,
                (c1.x + 50.0 + 3.0) as u32,
                (c1.y + 50.0 + 11.0) as u32
            ),
            accent
        );
    }

    #[test]
    fn priority_marks_are_orange() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        r.board.areas[2].todos = vec![Todo {
            text: "x".into(),
            done: false,
            priority: 3,
            body: None,
            due: None,
        }];
        let g = r.geom(2);
        let buf = r.frame();
        let orange = colors(Theme::Dark).accent[2];
        let row = &g.rows[0];
        let found = ((row.prio.x + 50.0) as u32..(row.prio.right() + 50.0) as u32)
            .flat_map(|x| {
                ((row.prio.y + 50.0) as u32..(row.prio.bottom() + 50.0) as u32).map(move |y| (x, y))
            })
            .filter(|&(x, y)| {
                let p = px(&buf, r.w, x, y);
                p.0 > 0xe0 && p.1 > 0x70 && p.2 < 0x40
            })
            .count();
        assert!(
            found > 10,
            "expected orange '!!!' pixels, found {found} (orange = {orange:?})"
        );
    }

    #[test]
    fn text_is_drawn_in_cards() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        r.board.areas[2].todos = vec![Todo::new("Hello world")];
        let g = r.geom(2);
        let buf = r.frame();
        let row = &g.rows[0];
        let bright = ((row.text.x + 50.0) as u32..(row.text.x + 50.0 + 90.0) as u32)
            .flat_map(|x| {
                ((row.text.y + 50.0) as u32..(row.text.bottom() + 50.0) as u32).map(move |y| (x, y))
            })
            .filter(|&(x, y)| px(&buf, r.w, x, y).0 > 0xc0)
            .count();
        assert!(bright > 40, "expected white glyph pixels, found {bright}");
    }

    #[test]
    fn notes_strokes_and_editing_do_not_panic_and_draw() {
        let mut r = Rig::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let n = r.board.add_note(400.0, 300.0);
        r.board.note_mut(n).unwrap().title = "Idea".into();
        r.board.note_mut(n).unwrap().body = "one two three four five six seven eight".into();
        let s = r.board.add_stroke(300.0, 500.0, 0);
        r.board.stroke_mut(s).unwrap().pts = vec![(300.0, 500.0), (350.0, 520.0), (400.0, 500.0)];
        r.editor.editing = Some(Edit {
            target: EditTarget::NoteBody(n),
            caret: 5,
        });
        r.editor.tool = Tool::Pen;
        let buf = r.frame();
        // stroke pixel on its path
        let blue = colors(Theme::Dark).accent[0];
        assert_eq!(
            px(&buf, r.w, (350.0 + 50.0) as u32, (520.0 + 50.0) as u32),
            blue
        );
        // note card is tinted (not the plain card colour)
        let note = note_geom(r.board.note(n).unwrap(), &|_, _| 7.0).card;
        let tint = px(
            &buf,
            r.w,
            (note.right() + 50.0 - 6.0) as u32,
            (note.bottom() + 50.0 - 6.0) as u32,
        );
        assert_ne!(tint, colors(Theme::Dark).card);
    }

    #[test]
    fn toolbar_highlights_the_active_tool() {
        let mut r = Rig::new();
        r.editor.tool = Tool::Pen;
        let buf = r.frame();
        let mut f = fonts();
        let ov = overlay(
            &r.board,
            r.w as f32,
            r.h as f32,
            crate::layout::Margins::default(),
            &|s, t| f.text_width(s.face(), t, s.size()),
        );
        let _ = &mut f;
        let pen = ov.tools.iter().find(|(t, _)| *t == Tool::Pen).unwrap().1;
        let sel = ov.tools.iter().find(|(t, _)| *t == Tool::Select).unwrap().1;
        // corner area of the button (inside the rounded rect, away from the icon) is accent-filled only for the active tool
        let at = |rc: LRect| px(&buf, r.w, (rc.x + 5.0) as u32, (rc.y + rc.h / 2.0) as u32);
        assert_eq!(at(pen), colors(Theme::Dark).accent[0]);
        assert_ne!(at(sel), colors(Theme::Dark).accent[0]);
    }

    #[test]
    fn view_menu_draws_zoom_icons_and_a_background_swatch() {
        let r = Rig::new();
        let buf = r.frame();
        let mut f = fonts();
        let ov = overlay(
            &r.board,
            r.w as f32,
            r.h as f32,
            crate::layout::Margins::default(),
            &|s, t| f.text_width(s.face(), t, s.size()),
        );
        let _ = &mut f;
        let sec = colors(Theme::Dark).secondary;
        let icon_px = |rc: LRect| {
            (rc.x as u32..rc.right() as u32)
                .flat_map(|x| (rc.y as u32..rc.bottom() as u32).map(move |y| (x, y)))
                .filter(|&(x, y)| {
                    let p = px(&buf, r.w, x, y);
                    p.0 > sec.0 - 25 && p.0 < sec.0 + 25 && p.2 > sec.2 - 25 && p.2 < sec.2 + 25
                })
                .count()
        };
        let (zin, zout) = (ov.view_btns[0].1, ov.view_btns[1].1);
        let (a, b) = (icon_px(zin), icon_px(zout));
        assert!(
            a > 15 && b > 15,
            "both buttons have icon pixels: in={a} out={b}"
        );
        assert!(
            a > b,
            "the plus has more ink than the minus: in={a} out={b}"
        );
        // the pill itself is drawn (not plain canvas)
        let pill = ov.view_pill;
        assert_ne!(
            px(
                &buf,
                r.w,
                (pill.x + 3.0) as u32,
                (pill.y + pill.h / 2.0) as u32
            ),
            colors(Theme::Dark).canvas
        );
    }

    #[test]
    fn background_button_shows_a_swatch_of_the_current_background() {
        let mut r = Rig::new();
        let mut f = fonts();
        let ov = overlay(
            &r.board,
            r.w as f32,
            r.h as f32,
            crate::layout::Margins::default(),
            &|s, t| f.text_width(s.face(), t, s.size()),
        );
        let _ = &mut f;
        let (_, rc) = ov.view_btns[2];
        let centre = |buf: &[u8]| {
            px(
                buf,
                r.w,
                (rc.x + rc.w / 2.0) as u32,
                (rc.y + rc.h / 2.0) as u32,
            )
        };
        assert_eq!(
            centre(&r.frame()),
            colors(Theme::Dark).canvas,
            "default: the theme canvas colour"
        );
        r.bg.color = Some("#14211A".into());
        assert_eq!(
            centre(&r.frame()),
            (0x14, 0x21, 0x1a),
            "a solid colour shows as that colour"
        );
        r.bg.color = None;
        r.bg.image_on = true;
        assert_eq!(
            centre(&r.frame()),
            colors(Theme::Dark).accent[0],
            "a wallpaper shows as the accent swatch"
        );
    }

    #[test]
    fn shapes_free_text_and_handles_are_drawn() {
        let mut r = Rig::new();
        r.board = Board::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let blue = colors(Theme::Dark).accent[0];
        let rect = r
            .board
            .add_shape(crate::board::ShapeKind::Rect, 200.0, 200.0);
        r.board.shape_mut(rect).unwrap().x2 = 400.0;
        r.board.shape_mut(rect).unwrap().y2 = 320.0;
        let oval = r
            .board
            .add_shape(crate::board::ShapeKind::Ellipse, 500.0, 200.0);
        r.board.shape_mut(oval).unwrap().x2 = 700.0;
        r.board.shape_mut(oval).unwrap().y2 = 320.0;
        let line = r
            .board
            .add_shape(crate::board::ShapeKind::Line, 200.0, 500.0);
        r.board.shape_mut(line).unwrap().x2 = 400.0;
        r.board.shape_mut(line).unwrap().y2 = 500.0;
        let t = r.board.add_text(500.0, 450.0);
        r.board.text_mut(t).unwrap().text = "Hello free text".into();
        let buf = r.frame();
        // the rectangle's top edge (centre of the stroke at y = 200 -> screen 250)
        let top = px(&buf, r.w, 300 + 50, 250);
        assert!(
            top.2 > 150 && top.0 < 100,
            "rect outline is blue: {top:?} vs {blue:?}"
        );
        // inside the rectangle: a faint blue tint over the canvas, not the plain canvas colour
        let inside = px(&buf, r.w, 300 + 50, 260 + 50);
        assert_ne!(inside, colors(Theme::Dark).canvas, "translucent fill");
        // the ellipse outline at its left-most point
        let oval_edge = px(&buf, r.w, 500 + 50, 260 + 50);
        assert!(oval_edge.2 > 120, "ellipse outline: {oval_edge:?}");
        // the ellipse does NOT cover its bounding-box corner
        assert_eq!(
            px(&buf, r.w, 503 + 50, 203 + 50),
            colors(Theme::Dark).canvas
        );
        // the line
        assert!(px(&buf, r.w, 300 + 50, 500 + 50).2 > 150, "line stroke");
        // free text pixels (white on dark)
        let bright = (500 + 50..620 + 50)
            .flat_map(|x| (450 + 50..478 + 50).map(move |y| (x, y)))
            .filter(|&(x, y)| px(&buf, r.w, x, y).0 > 0xc0)
            .count();
        assert!(bright > 40, "free text glyphs: {bright}");
        // selected shape shows white handle squares with an accent edge
        r.editor.selected = Some(Sel::Shape(rect));
        let buf = r.frame();
        assert_eq!(
            px(&buf, r.w, 200 + 50, 200 + 50 - 1),
            (0xff, 0xff, 0xff),
            "handle centre is white"
        );
    }

    #[test]
    fn resized_card_is_drawn_at_its_new_width() {
        let mut r = Rig::new();
        r.board = Board::new();
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let id = r.board.add_area(0.0, 0.0);
        r.board.area_mut(id).unwrap().w = 600.0;
        let buf = r.frame();
        let card = colors(Theme::Dark).card;
        assert_eq!(
            px(&buf, r.w, 50 + 560, 50 + 100),
            card,
            "inside the widened card"
        );
        assert_eq!(
            px(&buf, r.w, 50 + 640, 50 + 100),
            colors(Theme::Dark).canvas,
            "and nothing beyond it"
        );
    }

    #[test]
    fn current_chip_is_filled_with_its_space_colour() {
        let mut r = Rig::new();
        for n in ["Home", "Docs", "Plans"] {
            r.board.add_space(n, 0.0, 0.0, 1.0);
        }
        let buf = r.frame();
        let mut f = fonts();
        let ov = overlay(
            &r.board,
            r.w as f32,
            r.h as f32,
            crate::layout::Margins::default(),
            &|s, t| f.text_width(s.face(), t, s.size()),
        );
        let _ = &mut f;
        let (ai, rect) = ov.chips[1]; // the rig marks index 1 as current
        assert_eq!(ai, 1);
        let col = px(
            &buf,
            r.w,
            (rect.x + 3.0) as u32,
            (rect.y + rect.h / 2.0) as u32,
        );
        assert_eq!(col, colors(Theme::Dark).accent[1]);
    }

    #[test]
    fn hidpi_scale_two_renders_the_same_layout_at_double_size() {
        let mut r = Rig::new();
        r.scale = 2;
        r.w = 3600;
        r.h = 1800;
        r.cam = Camera {
            x: -50.0,
            y: -50.0,
            zoom: 1.0,
        };
        let buf = r.frame();
        let inbox = r.geom(0).card;
        assert_eq!(
            px(&buf, r.w, inside(inbox, 2).0, inside(inbox, 2).1),
            colors(Theme::Dark).card
        );
    }

    #[test]
    fn extreme_camera_tiny_zoom_and_huge_zoom_do_not_panic() {
        let mut r = Rig::new();
        r.board.areas[2].todos = vec![Todo::new(
            "a long long long long long long long todo title that must be clipped",
        )];
        for cam in [
            Camera {
                x: 1.0e7,
                y: -1.0e7,
                zoom: 0.1,
            },
            Camera {
                x: -3.0,
                y: -3.0,
                zoom: 8.0,
            },
            Camera {
                x: 0.0,
                y: 0.0,
                zoom: 0.1,
            },
        ] {
            r.cam = cam;
            r.frame();
        }
    }

    #[test]
    fn build_wallpaper_rejects_non_png_with_a_helpful_message() {
        let dir = std::env::temp_dir().join(format!("wallboard-wp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("a.jpg");
        std::fs::write(&jpg, [0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0, 0, 0]).unwrap();
        let e = build_wallpaper(jpg.to_str().unwrap(), 10, 10, 0.3).unwrap_err();
        assert!(e.contains("JPEG") && e.contains("PNG"), "{e}");
        assert!(build_wallpaper("/definitely/not/here.png", 10, 10, 0.3)
            .unwrap_err()
            .contains("cannot read"));
    }

    #[test]
    fn build_wallpaper_covers_dims_and_swaps_channels() {
        let dir = std::env::temp_dir().join(format!("wallboard-wp2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 4x2 PNG: left half pure red, right half pure blue.
        let mut src = Pixmap::new(4, 2).unwrap();
        for y in 0..2 {
            for x in 0..4 {
                let i = ((y * 4 + x) * 4) as usize;
                let c = if x < 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                src.data_mut()[i..i + 4].copy_from_slice(&c);
            }
        }
        let path = dir.join("t.png");
        src.save_png(&path).unwrap();
        let (w, h) = (16u32, 8u32);
        let out = build_wallpaper(path.to_str().unwrap(), w, h, 0.0).unwrap();
        assert_eq!(out.len(), (w * h * 4) as usize);
        let at = |x: u32, y: u32| px(&out, w, x, y);
        assert_eq!(
            at(1, 4),
            (255, 0, 0),
            "left is red on screen (channels swapped into BGRX)"
        );
        assert_eq!(at(14, 4), (0, 0, 255));
        assert!(
            out.as_chunks::<4>().0.iter().all(|p| p[3] == 0xff),
            "opaque"
        );
        // dimming darkens
        let dim = build_wallpaper(path.to_str().unwrap(), w, h, 0.5).unwrap();
        let (p, q) = (px(&dim, w, 1, 4), px(&out, w, 1, 4));
        assert!(p.0 < q.0 && p.0 > 100, "{p:?} vs {q:?}");
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use crate::layout::overlay;
    #[test]
    fn dump_frame() {
        let Some(dir) = std::env::var_os("WB_DUMP") else {
            return;
        };
        let mut f = Fonts::load().unwrap().0;
        let mut board = Board::default_board();
        board.areas[2].todos = vec![
            crate::board::Todo::new("Buy milk"),
            crate::board::Todo {
                text: "Ship it".into(),
                done: true,
                priority: 2,
                body: Some("details here\nsecond line".into()),
                due: None,
            },
        ];
        board.areas[1].todos = vec![
            crate::board::Todo::new("Focus one"),
            crate::board::Todo::new("Focus two"),
        ];
        let n = board.add_note(1300.0, 40.0);
        board.note_mut(n).unwrap().title = "Idea".into();
        board.note_mut(n).unwrap().body = "A free note with some body text that wraps".into();
        let s = board.add_stroke(1300.0, 400.0, 2);
        board.stroke_mut(s).unwrap().pts = vec![
            (1300.0, 400.0),
            (1360.0, 440.0),
            (1420.0, 400.0),
            (1480.0, 450.0),
        ];
        let editor = Editor::new();
        let (w, h) = (1920u32, 1080u32);
        for theme in Theme::ALL {
            let name = theme.label().to_lowercase();
            let ov = {
                let ff: &Fonts = &f;
                overlay(
                    &board,
                    w as f32,
                    h as f32,
                    crate::layout::Margins::default(),
                    &|s, t| ff.text_width(s.face(), t, s.size()),
                )
            };
            let bg = Background::default();
            let scene = Scene {
                board: &board,
                editor: &editor,
                theme,
                palette: None,
                background: &bg,
                wallpaper: None,
                overlay: &ov,
                current_area: Some(1),
                show_help: false,
                detail: None,
                name_box: None,
                theme_open: theme == Theme::Aurora || theme == Theme::Brutal,
                hover: Hover::Theme(Theme::Neon),
                text_bar: None,
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            draw(
                &mut buf,
                w,
                h,
                &Camera {
                    x: -40.0,
                    y: -120.0,
                    zoom: 0.62,
                },
                1,
                &scene,
                &mut f,
            );
            for p in buf.as_chunks_mut::<4>().0 {
                *p = [p[2], p[1], p[0], 255];
            }
            let pm = Pixmap::from_vec(buf, tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap();
            pm.save_png(std::path::Path::new(&dir).join(format!("ui_{name}.png")))
                .unwrap();
        }
    }
}

#[cfg(test)]
mod dump_text {
    use super::*;
    use crate::board::TextSize;
    use crate::layout::{overlay, text_bar, Margins};
    /// `WB_DUMP=/dir cargo test dump_text_styles`: styled and Markdown text, with the format bar.
    #[test]
    fn dump_text_styles() {
        let Some(dir) = std::env::var_os("WB_DUMP") else {
            return;
        };
        let mut f = Fonts::load().unwrap().0;
        let mut board = Board::new();
        let md = board.add_text(110.0, 60.0);
        board.text_mut(md).unwrap().text = "# Weekly plan\n## Goals\nShip **wallboard** with *rich* text, ~~no bugs~~ and `code`\n- first bullet\n  - nested bullet\n1. numbered\n2. second\n- [ ] open task\n- [x] done task\n> a quote worth keeping\n---\nplain line after the rule".into();
        let st = board.add_text(820.0, 90.0);
        {
            let t = board.text_mut(st).unwrap();
            t.text = "Bold italic heading\nunderlined in red".into();
            (t.size, t.bold, t.italic, t.underline, t.color) = (TextSize::Heading, true, true, true, 3);
        }
        let sm = board.add_text(820.0, 300.0);
        {
            let t = board.text_mut(sm).unwrap();
            t.text = "small struck text".into();
            (t.size, t.strike) = (TextSize::Small, true);
        }
        let note = board.add_note(110.0, 560.0);
        {
            let n = board.note_mut(note).unwrap();
            n.title = "Sticky *styled*".into();
            n.body = "## Today\n- [x] **ship** it\n- [ ] write `tests`\n> stay ~~busy~~ calm".into();
        }
        let area = board.add_area(520.0, 420.0);
        board.area_mut(area).unwrap().todos = vec![
            crate::board::Todo::new("Call **mom** about *dinner*"),
            crate::board::Todo {
                body: Some("- bring **cake**\n- `7pm`".into()),
                ..crate::board::Todo::new("Plain todo")
            },
        ];
        let mut editor = Editor::new();
        editor.selected = Some(Sel::Text(st));
        let (w, h) = (1400u32, 1000u32);
        let cam = Camera { x: 0.0, y: -40.0, zoom: 1.0 };
        for theme in [Theme::Dark, Theme::Light] {
            let ff: &Fonts = &f;
            let adv = |s: Style, ch: char| ff.advance(s.face(), ch, s.size());
            let ov = overlay(&board, w as f32, h as f32, Margins::default(), &|s, t| {
                ff.text_width(s.face(), t, s.size())
            });
            let bar = text_bar(&board, st, &cam, (w as f32, h as f32), &adv);
            let bg = Background::default();
            let scene = Scene {
                board: &board,
                editor: &editor,
                theme,
                palette: None,
                background: &bg,
                wallpaper: None,
                overlay: &ov,
                current_area: None,
                show_help: false,
                detail: None,
                name_box: None,
                theme_open: false,
                hover: Hover::Fmt(FmtBtn::Size(TextSize::Title)),
                text_bar: bar.as_ref(),
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            draw(&mut buf, w, h, &cam, 1, &scene, &mut f);
            for p in buf.as_chunks_mut::<4>().0 {
                *p = [p[2], p[1], p[0], 255];
            }
            let pm = Pixmap::from_vec(buf, tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap();
            let name = format!("text_{}.png", theme.label().to_lowercase());
            pm.save_png(std::path::Path::new(&dir).join(name)).unwrap();
        }
    }
}

/// Renders the marketing reel (portrait 1080x1920, 30 fps) with the real renderer, as PNG frames.
/// Run: `WB_DEMO=/some/dir cargo test --release demo_video -- --nocapture`, then encode with
/// ffmpeg. Test-only: it is never part of the shipped binary.
#[cfg(test)]
mod demo {
    use super::*;
    use crate::board::Todo;
    use crate::layout::{overlay, Margins};

    const FPS: f32 = 30.0;
    const DUR: f32 = 21.0;
    const LW: f32 = 540.0;
    const LH: f32 = 960.0;
    const BLEND: f32 = 0.3;

    fn ss(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Piecewise smoothstep through keyframes (time, values...).
    fn keys<const N: usize>(k: &[(f32, [f32; N])], t: f32) -> [f32; N] {
        if t <= k[0].0 {
            return k[0].1;
        }
        for w in k.windows(2) {
            if t <= w[1].0 {
                let u = ss((t - w[0].0) / (w[1].0 - w[0].0));
                let mut out = [0.0; N];
                for i in 0..N {
                    out[i] = w[0].1[i] + (w[1].1[i] - w[0].1[i]) * u;
                }
                return out;
            }
        }
        k[k.len() - 1].1
    }

    fn todo(text: &str, done: bool, priority: u8) -> Todo {
        Todo {
            text: text.into(),
            done,
            priority,
            body: None,
            due: None,
        }
    }

    /// The board at time `t`: positions are fixed (laid out for the final content), content grows.
    fn board_at(t: f32, f: &Fonts, land: bool) -> Board {
        let mut b = Board::default_board();
        let typed = "Post the reel on Instagram";
        let n = (((t - 3.9) / 0.06).max(0.0) as usize).min(typed.chars().count());
        let ship_done = t > 6.0;
        let content: [(usize, Vec<Todo>); 7] = [
            (
                0,
                vec![
                    todo("Call the dentist", false, 0),
                    todo("Pay electricity bill", false, 1),
                ],
            ),
            (1, {
                let mut v = vec![
                    todo("Ship wallboard v1", ship_done, 3),
                    todo("Record launch reel", true, 2),
                ];
                if n > 0 {
                    v.push(todo(&typed.chars().take(n).collect::<String>(), false, 0));
                }
                v
            }),
            (
                2,
                vec![
                    todo("Review pull requests", false, 0),
                    todo("Write release notes", true, 0),
                    todo("Fix flaky test", false, 1),
                ],
            ),
            (
                3,
                vec![
                    todo("Talk to 5 users", false, 2),
                    todo("Draft pitch deck", false, 0),
                ],
            ),
            (
                4,
                vec![
                    todo("Morning run", true, 0),
                    todo("Stretch 10 min", false, 0),
                    todo("Drink more water", false, 0),
                ],
            ),
            (
                5,
                vec![
                    todo("Rust lifetimes chapter", false, 0),
                    todo("Wayland protocol notes", false, 0),
                ],
            ),
            (
                6,
                vec![todo("Book flights", false, 1), todo("Groceries", false, 0)],
            ),
        ];
        // Final-content copy used only to compute stable positions.
        let adv = |s: Style, ch: char| f.advance(s.face(), ch, s.size());
        let mut full = Board::default_board();
        for (i, v) in &content {
            full.areas[*i].todos = v.clone();
        }
        if full.areas[1].todos.len() < 3 {
            full.areas[1].todos.push(todo(typed, false, 0));
        }
        let h = |i: usize| area_geom(&full.areas[i], &adv).card.h;
        let (lx, rx, gap) = (0.0_f32, 430.0_f32, 28.0_f32);
        let (n1_pos, n2_pos, stroke_pos);
        if land {
            // Three columns, three rows: a landscape desktop.
            let cols = [0.0_f32, 348.0, 776.0];
            let rows: [[usize; 3]; 3] = [[0, 1, 2], [3, 4, 5], [6, 6, 6]];
            let mut y = 0.0;
            for (ri, row) in rows.iter().enumerate() {
                let mut hmax: f32 = 0.0;
                for (ci, &i) in row.iter().enumerate() {
                    if ri == 2 && ci > 0 {
                        continue;
                    }
                    b.areas[i].x = cols[ci];
                    b.areas[i].y = y;
                    hmax = hmax.max(h(i));
                }
                y += hmax + gap;
            }
            let ry = b.areas[6].y;
            n1_pos = (cols[1], ry);
            n2_pos = (cols[2], ry + 10.0);
            stroke_pos = (cols[1] + 10.0, ry + 160.0);
        } else {
            // Two columns: left Focus, Work, Health, Life; right Inbox, Startups, Learning.
            let mut y = 0.0;
            for i in [1usize, 2, 4, 6] {
                b.areas[i].x = lx;
                b.areas[i].y = y;
                y += h(i) + gap;
            }
            let mut y2 = 0.0;
            for i in [0usize, 3, 5] {
                b.areas[i].x = rx;
                b.areas[i].y = y2;
                y2 += h(i) + gap;
            }
            n1_pos = (rx, y2);
            n2_pos = (rx + 40.0, y2 + 190.0);
            stroke_pos = (rx + 10.0, y2 + 380.0);
        }
        for (i, v) in content {
            b.areas[i].todos = v;
        }
        let n1 = b.add_note(n1_pos.0, n1_pos.1);
        b.note_mut(n1).unwrap().title = "Idea".into();
        b.note_mut(n1).unwrap().body = "Everything in one calm place, right on the desktop.".into();
        b.note_mut(n1).unwrap().color = 4;
        let n2 = b.add_note(n2_pos.0, n2_pos.1);
        b.note_mut(n2).unwrap().title = "Reading".into();
        b.note_mut(n2).unwrap().body = "The Rust book, ch. 10".into();
        b.note_mut(n2).unwrap().color = 1;
        let s = b.add_stroke(stroke_pos.0, stroke_pos.1, 2);
        b.stroke_mut(s).unwrap().pts = (0..24)
            .map(|i| {
                let x = i as f32 * 12.0;
                (stroke_pos.0 + x, stroke_pos.1 + (x * 0.09).sin() * 14.0)
            })
            .collect();
        b
    }

    struct Blend {
        from: Colors,
        to: Theme,
        t0: f32,
    }
    impl Blend {
        fn at(&mut self, target: Theme, now: f32) -> Colors {
            if target != self.to {
                let p = ss((now - self.t0) / BLEND);
                self.from = lerp_colors(&self.from, &colors(self.to), p);
                self.to = target;
                self.t0 = now;
            }
            lerp_colors(&self.from, &colors(self.to), ss((now - self.t0) / BLEND))
        }
    }

    fn cursor(pm: &mut PixmapMut, s: f32, x: f32, y: f32) {
        let pts = [
            (0.0, 0.0),
            (0.0, 17.0),
            (4.5, 13.0),
            (8.0, 20.0),
            (11.0, 18.5),
            (7.5, 12.0),
            (13.0, 12.0),
        ];
        let mut pb = PathBuilder::new();
        for (i, (px, py)) in pts.iter().enumerate() {
            let (px, py) = (x * s + px * s * 1.3, y * s + py * s * 1.3);
            if i == 0 {
                pb.move_to(px, py);
            } else {
                pb.line_to(px, py);
            }
        }
        pb.close();
        if let Some(p) = pb.finish() {
            let tf = Transform::identity();
            pm.fill_path(&p, &paint(WHITE, 0xff, true), FillRule::Winding, tf, None);
            let st = SkStroke {
                width: 1.6 * s,
                line_join: LineJoin::Round,
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint((0, 0, 0), 0xff, true), &st, tf, None);
        }
    }

    fn caption(
        pm: &mut PixmapMut,
        v: &View,
        fonts: &mut Fonts,
        text: &str,
        a: f32,
        cy: f32,
        size: f32,
        bold: bool,
    ) {
        if a <= 0.01 {
            return;
        }
        let s = v.scale;
        let face = if bold { Face::Bold } else { Face::Regular };
        let tw = fonts.text_width(face, text, size * s);
        let (pw, ph) = (tw + 44.0 * s, size * s * 1.9);
        let (x, y) = ((v.w as f32 - pw) / 2.0, cy * s - ph / 2.0);
        if let Some(p) = rounded_rect(x, y, pw, ph, ph / 2.0) {
            pm.fill_path(
                &p,
                &paint((10, 10, 14), (170.0 * a) as u8, true),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let col = mix((20, 20, 24), WHITE, a);
        text_px(
            pm,
            v,
            fonts,
            face,
            size * s,
            x + 22.0 * s,
            cy * s,
            text,
            col,
            (0.0, v.w as f32),
            false,
        );
    }

    #[test]
    fn demo_video() {
        let Some(dir) = std::env::var_os("WB_DEMO") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Fonts::load().unwrap().0;
        let scale = 2u32;
        let (w, h) = ((LW as u32) * scale, (LH as u32) * scale);
        let editor = Editor::new();
        let bg = Background::default();
        let margins = Margins { top: 150.0 };

        // Stable geometry from the final board.
        let b_final = board_at(DUR, &f, false);
        let adv = |s: Style, ch: char| f.advance(s.face(), ch, s.size());
        let focus = area_geom(&b_final.areas[1], &adv).card;
        let bottom = b_final
            .areas
            .iter()
            .map(|a| area_geom(a, &adv).card.bottom())
            .fold(0.0, f32::max);
        let wide_cy = bottom / 2.0 - 10.0;
        let notes_cy = b_final.notes[0].y + 120.0;

        // Theme-tour cursor targets (logical px), from the real overlay layout.
        let ov0 = {
            let ff: &Fonts = &f;
            overlay(&b_final, LW, LH, margins, &|s, t| {
                ff.text_width(s.face(), t, s.size())
            })
        };
        let centre = |r: LRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let theme_btn = centre(
            ov0.view_btns
                .iter()
                .find(|(b, _)| *b == ViewBtn::Theme)
                .unwrap()
                .1,
        );
        let row = |t: Theme| centre(ov0.theme_items.iter().find(|(x, _)| *x == t).unwrap().1);

        let tour = [
            Theme::Light,
            Theme::Paper,
            Theme::Brutal,
            Theme::Blueprint,
            Theme::Neon,
            Theme::Aurora,
        ];
        // Cursor keyframes: (time, x, y).
        let mut ck: Vec<(f32, [f32; 2])> = vec![
            (0.0, [300.0, 800.0]),
            (9.6, [300.0, 800.0]),
            (10.6, [theme_btn.0, theme_btn.1]),
        ];
        let mut t = 11.4;
        for th in tour {
            let r = row(th);
            ck.push((t, [r.0, r.1]));
            ck.push((t + 0.9, [r.0 + 2.0, r.1 + 2.0]));
            t += 1.25;
        }
        let tour_end = t; // ~ 18.9
        ck.push((tour_end + 0.7, [330.0, 760.0]));
        let click_open = 10.8_f32;
        let panel_close = tour_end - 0.3;
        let choose_at = |i: usize| 11.4 + 1.25 * i as f32 + 0.5;

        let mut blend = Blend {
            from: colors(Theme::Dark),
            to: Theme::Dark,
            t0: -10.0,
        };
        let caps: [(f32, f32, &str); 5] = [
            (0.5, 3.2, "Your whole life, on your desktop"),
            (3.5, 8.8, "Todos, notes & sketches on an infinite canvas"),
            (9.0, 10.5, "Now make it yours"),
            (11.0, 18.3, "7 themes. Hover to preview live."),
            (
                18.9,
                21.0,
                "wallboard · a life-OS for niri · Rust · ~15 MB RAM",
            ),
        ];

        let frames = (DUR * FPS) as usize;
        for fr in 0..frames {
            let t = fr as f32 / FPS;
            let board = board_at(t, &f, false);
            // Camera.
            let z_wide = 0.54;
            let k = keys(
                &[
                    (0.0, [375.0, wide_cy, 0.50]),
                    (3.0, [375.0, wide_cy, 0.54]),
                    (
                        3.8,
                        [
                            focus.x + focus.w / 2.0,
                            focus.y + focus.h / 2.0 + 10.0,
                            0.93,
                        ],
                    ),
                    (
                        7.0,
                        [
                            focus.x + focus.w / 2.0,
                            focus.y + focus.h / 2.0 + 10.0,
                            0.97,
                        ],
                    ),
                    (8.2, [590.0, notes_cy, 0.95]),
                    (9.4, [590.0, notes_cy, 0.98]),
                    (10.4, [375.0, wide_cy, z_wide]),
                    (19.0, [375.0, wide_cy, z_wide + 0.015]),
                    (21.0, [375.0, wide_cy, 0.50]),
                ],
                t,
            );
            let (cx, cy, z) = (k[0], k[1], k[2]);
            let cam = Camera {
                x: cx - LW / 2.0 / z,
                y: cy - LH / 2.0 / z,
                zoom: z,
            };
            // Cursor + hover + panel state.
            let cpos = {
                let pts: Vec<(f32, [f32; 2])> = ck.clone();
                keys(&pts, t)
            };
            let mut cur = Theme::Dark;
            let mut open = false;
            if t >= click_open && t < panel_close {
                open = true;
            }
            for (i, th) in tour.iter().enumerate() {
                if t >= choose_at(i) && *th == Theme::Aurora {
                    cur = Theme::Aurora;
                }
            }
            let ov = {
                let ff: &Fonts = &f;
                overlay(&board, LW, LH, margins, &|s, t| {
                    ff.text_width(s.face(), t, s.size())
                })
            };
            let hover = if t > 10.0 {
                ov.hover_at(cpos[0], cpos[1], open)
            } else {
                Hover::None
            };
            let shown = match hover {
                Hover::Theme(th) if open => th,
                _ => cur,
            };
            let palette = blend.at(shown, t);
            let current = crate::layout::current_area(&board, &cam, LW, LH, &|s, c| {
                f.advance(s.face(), c, s.size())
            });
            let scene = Scene {
                board: &board,
                editor: &editor,
                theme: cur,
                palette: Some(palette),
                background: &bg,
                wallpaper: None,
                overlay: &ov,
                current_area: current,
                show_help: false,
                detail: None,
                name_box: None,
                theme_open: open,
                hover,
                text_bar: None,
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            draw(&mut buf, w, h, &cam, scale, &scene, &mut f);

            // Captions, title and cursor on top.
            {
                let v = View {
                    cam,
                    z: z * scale as f32,
                    scale: scale as f32,
                    w,
                    h,
                };
                let mut pm = PixmapMut::from_bytes(&mut buf, w, h).unwrap();
                for (a, b, text) in caps {
                    let al = ss((t - a) / 0.35).min(ss((b - t) / 0.35));
                    caption(&mut pm, &v, &mut f, text, al, 775.0, 15.0, true);
                }
                let title_a = ss(t / 0.5).min(ss((3.2 - t) / 0.5));
                caption(&mut pm, &v, &mut f, "wallboard", title_a, 85.0, 34.0, true);
                if t > 9.0 && t < tour_end + 0.8 {
                    let fade = ss((t - 9.0) / 0.4).min(ss((tour_end + 0.8 - t) / 0.4));
                    if fade > 0.5 {
                        cursor(&mut pm, scale as f32, cpos[0], cpos[1]);
                    }
                }
            }
            for p in buf.as_chunks_mut::<4>().0 {
                *p = [p[2], p[1], p[0], 255];
            }
            let pm = Pixmap::from_vec(buf, tiny_skia::IntSize::from_wh(w, h).unwrap()).unwrap();
            pm.save_png(dir.join(format!("f_{fr:04}.png"))).unwrap();
        }
        println!("wrote {frames} frames to {}", dir.display());
    }

    // ---------------------------------------------------------------------------------------
    // "System" reel: wallboard as the desktop wallpaper, apps tiling over it. Windows are
    // drawn mock-ups (no real screen contents are captured).
    // ---------------------------------------------------------------------------------------

    const DW: f32 = 1280.0;
    const DH: f32 = 720.0;
    const SDUR: f32 = 26.0;

    fn rrect(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgb, a: u8) {
        if let Some(p) = rounded_rect(x, y, w, h, r) {
            pm.fill_path(
                &p,
                &paint(c, a, true),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    fn ring(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgb, width: f32) {
        if let Some(p) = rounded_rect(x, y, w, h, r) {
            let st = SkStroke {
                width,
                ..SkStroke::default()
            };
            pm.stroke_path(&p, &paint(c, 0xff, true), &st, Transform::identity(), None);
        }
    }

    fn dot(pm: &mut PixmapMut, x: f32, y: f32, r: f32, c: Rgb) {
        if let Some(p) = PathBuilder::from_circle(x, y, r) {
            pm.fill_path(
                &p,
                &paint(c, 0xff, true),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// Window rect (logical px) for a column, sliding in/out with progress `p` (0 = away, 1 = in).
    fn win_rect(right: bool, p: f32) -> (f32, f32, f32, f32) {
        let (w, y, h) = ((DW - 24.0) / 2.0, 46.0, DH - 46.0 - 12.0);
        let x = if right {
            let to = 12.0 + w + 8.0;
            DW + 20.0 + (to - DW - 20.0) * ss(p)
        } else {
            let to = 12.0;
            -w - 20.0 + (to + w + 20.0) * ss(p)
        };
        (x, y, w - 4.0, h)
    }

    fn window_shadow(pm: &mut PixmapMut, s: f32, r: (f32, f32, f32, f32)) {
        rrect(
            pm,
            (r.0 - 4.0) * s,
            (r.1 + 6.0) * s,
            (r.2 + 8.0) * s,
            (r.3 + 4.0) * s,
            16.0 * s,
            (0, 0, 0),
            90,
        );
        rrect(
            pm,
            (r.0 - 10.0) * s,
            (r.1 + 2.0) * s,
            (r.2 + 20.0) * s,
            (r.3 + 18.0) * s,
            22.0 * s,
            (0, 0, 0),
            36,
        );
    }

    fn draw_browser(
        pm: &mut PixmapMut,
        v: &View,
        fonts: &mut Fonts,
        r: (f32, f32, f32, f32),
        t_open: f32,
        focused: bool,
    ) {
        let s = v.scale;
        window_shadow(pm, s, r);
        let (x, y, w, h) = (r.0 * s, r.1 * s, r.2 * s, r.3 * s);
        rrect(pm, x, y, w, h, 12.0 * s, (247, 247, 250), 0xff);
        rrect(pm, x, y, w, 44.0 * s, 12.0 * s, (231, 231, 237), 0xff);
        rrect(pm, x, y + 30.0 * s, w, 14.0 * s, 0.0, (231, 231, 237), 0xff);
        for (i, c) in [(255, 95, 87), (254, 188, 46), (40, 200, 64)]
            .iter()
            .enumerate()
        {
            dot(
                pm,
                x + (22.0 + i as f32 * 20.0) * s,
                y + 22.0 * s,
                5.5 * s,
                *c,
            );
        }
        rrect(
            pm,
            x + 100.0 * s,
            y + 9.0 * s,
            w - 130.0 * s,
            26.0 * s,
            13.0 * s,
            (255, 255, 255),
            0xff,
        );
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            12.5 * s,
            x + 118.0 * s,
            y + 22.0 * s,
            "localhost:3000/dashboard",
            (110, 110, 120),
            (0.0, v.w as f32),
            false,
        );
        // Page: heading, copy, three stat cards, a growing bar chart.
        rrect(
            pm,
            x + 28.0 * s,
            y + 72.0 * s,
            210.0 * s,
            24.0 * s,
            8.0 * s,
            (30, 30, 40),
            0xff,
        );
        for (i, wd) in [0.8_f32, 0.62, 0.7].iter().enumerate() {
            rrect(
                pm,
                x + 28.0 * s,
                (y + 114.0 + i as f32 * 20.0 * 1.0) * s,
                (w / s - 56.0) * wd * s,
                9.0 * s,
                4.5 * s,
                (206, 206, 214),
                0xff,
            );
        }
        let cw = (w / s - 56.0 - 24.0) / 3.0;
        for (i, c) in [(10u8, 132u8, 255u8), (48, 209, 88), (255, 159, 10)]
            .iter()
            .enumerate()
        {
            let cx = x + (28.0 + i as f32 * (cw + 12.0)) * s;
            rrect(
                pm,
                cx,
                y + 190.0 * s,
                cw * s,
                92.0 * s,
                12.0 * s,
                (255, 255, 255),
                0xff,
            );
            rrect(
                pm,
                cx + 14.0 * s,
                y + 204.0 * s,
                44.0 * s,
                10.0 * s,
                5.0 * s,
                *c,
                0xff,
            );
            rrect(
                pm,
                cx + 14.0 * s,
                y + 232.0 * s,
                (cw - 40.0) * s,
                20.0 * s,
                6.0 * s,
                (30, 30, 40),
                0xff,
            );
        }
        let g = ss(t_open);
        let bars = [0.45_f32, 0.7, 0.55, 0.9, 0.65, 0.8, 0.5, 0.95, 0.72];
        let bw = (w / s - 56.0) / bars.len() as f32;
        for (i, b) in bars.iter().enumerate() {
            let bh = 150.0 * b * g;
            rrect(
                pm,
                x + (28.0 + i as f32 * bw) * s,
                y + (h / s - 28.0 - bh) * s,
                (bw - 8.0) * s,
                bh * s,
                6.0 * s,
                (10, 132, 255),
                0xff,
            );
        }
        if focused {
            ring(pm, x, y, w, h, 12.0 * s, (10, 132, 255), 2.2 * s);
        }
    }

    fn draw_terminal(
        pm: &mut PixmapMut,
        v: &View,
        fonts: &mut Fonts,
        r: (f32, f32, f32, f32),
        typed: usize,
        focused: bool,
        blink: bool,
    ) {
        let s = v.scale;
        window_shadow(pm, s, r);
        let (x, y, w, h) = (r.0 * s, r.1 * s, r.2 * s, r.3 * s);
        rrect(pm, x, y, w, h, 12.0 * s, (22, 22, 30), 0xff);
        let lines: [(&str, Rgb); 8] = [
            ("$ cargo build --release", (235, 235, 245)),
            ("   Compiling wallboard v0.1.0", (150, 150, 170)),
            ("    Finished `release` profile [optimized]", (48, 209, 88)),
            ("$ ./wallboard", (235, 235, 245)),
            ("  backed up to a private repo", (150, 150, 170)),
            ("$ git push origin main", (235, 235, 245)),
            ("  main -> main", (150, 150, 170)),
            ("$ ", (235, 235, 245)),
        ];
        let mut left = typed;
        let mut last = (0.0, 0.0);
        for (i, (txt, col)) in lines.iter().enumerate() {
            let n = txt.chars().count();
            let show = left.min(n);
            let part: String = txt.chars().take(show).collect();
            let cy = y + (30.0 + i as f32 * 26.0) * s;
            let mut tx = x + 22.0 * s;
            if part.starts_with("$ ") {
                tx += text_px(
                    pm,
                    v,
                    fonts,
                    Face::Bold,
                    14.5 * s,
                    tx,
                    cy,
                    "$ ",
                    (48, 209, 88),
                    (0.0, v.w as f32),
                    false,
                );
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    14.5 * s,
                    tx,
                    cy,
                    &part[2..],
                    *col,
                    (0.0, v.w as f32),
                    false,
                );
            } else {
                text_px(
                    pm,
                    v,
                    fonts,
                    Face::Regular,
                    14.5 * s,
                    tx,
                    cy,
                    &part,
                    *col,
                    (0.0, v.w as f32),
                    false,
                );
            }
            last = (tx + fonts.text_width(Face::Regular, &part, 14.5 * s), cy);
            if left <= n {
                break;
            }
            left -= n;
        }
        if blink {
            rrect(
                pm,
                last.0 + 4.0 * s,
                last.1 - 9.0 * s,
                8.0 * s,
                18.0 * s,
                1.0,
                (235, 235, 245),
                0xff,
            );
        }
        if focused {
            ring(pm, x, y, w, h, 12.0 * s, (10, 132, 255), 2.2 * s);
        }
    }

    /// A shell bar on niri's top layer: workspaces left, clock centre, status right.
    fn draw_bar(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, ws_active: usize) {
        let s = v.scale;
        rrect(pm, 0.0, 0.0, v.w as f32, 32.0 * s, 0.0, (14, 14, 20), 238);
        for i in 0..3 {
            let cx = (18.0 + i as f32 * 22.0) * s;
            if i == ws_active {
                rrect(
                    pm,
                    cx - 9.0 * s,
                    12.0 * s,
                    24.0 * s,
                    8.0 * s,
                    4.0 * s,
                    (240, 240, 250),
                    0xff,
                );
            } else {
                dot(pm, cx + 3.0 * s, 16.0 * s, 4.0 * s, (95, 95, 112));
            }
        }
        let clock = "Thu 9:41";
        let tw = fonts.text_width(Face::Bold, clock, 14.0 * s);
        text_px(
            pm,
            v,
            fonts,
            Face::Bold,
            14.0 * s,
            (v.w as f32 - tw) / 2.0,
            16.0 * s,
            clock,
            (240, 240, 250),
            (0.0, v.w as f32),
            false,
        );
        let tx = v.w as f32 - 150.0 * s;
        for i in 0..3 {
            dot(
                pm,
                tx + i as f32 * 16.0 * s,
                16.0 * s,
                4.5 * s,
                (150, 150, 170),
            );
        }
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            13.0 * s,
            tx + 58.0 * s,
            16.0 * s,
            "87%",
            (200, 200, 214),
            (0.0, v.w as f32),
            false,
        );
        rrect(
            pm,
            tx + 92.0 * s,
            10.0 * s,
            28.0 * s,
            12.0 * s,
            3.0 * s,
            (48, 209, 88),
            0xff,
        );
    }

    fn headline(pm: &mut PixmapMut, v: &View, fonts: &mut Fonts, l1: &str, l2: &str, a: f32) {
        if a <= 0.01 {
            return;
        }
        let bg = (12, 12, 22);
        for (i, (txt, col)) in [(l1, (255, 255, 255)), (l2, (150, 178, 255))]
            .iter()
            .enumerate()
        {
            let px = 66.0;
            let tw = fonts.text_width(Face::Bold, txt, px);
            let x = (v.w as f32 - tw) / 2.0;
            text_px(
                pm,
                v,
                fonts,
                Face::Bold,
                px,
                x,
                440.0 + i as f32 * 86.0 + (1.0 - a) * 14.0,
                txt,
                mix(bg, *col, a),
                (0.0, v.w as f32),
                false,
            );
        }
    }

    #[test]
    fn demo_system_video() {
        let Some(dir) = std::env::var_os("WB_DEMO2") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = Fonts::load().unwrap().0;
        let ds = 2u32;
        let (dw, dh) = ((DW as u32) * ds, (DH as u32) * ds);
        let (fw, fh) = (1080u32, 1920u32);
        let editor = Editor::new();
        let bg = Background::default();
        let margins = Margins { top: 56.0 };

        let b_final = board_at(SDUR + 5.0, &f, true);
        let adv = |s: Style, ch: char| f.advance(s.face(), ch, s.size());
        let bottom = b_final
            .areas
            .iter()
            .map(|a| area_geom(a, &adv).card.bottom())
            .fold(0.0, f32::max);
        let board_w = 1096.0_f32;
        let z_board = (590.0 / bottom).min(1.0);
        let cam = Camera {
            x: board_w / 2.0 - DW / 2.0 / z_board,
            y: (bottom / 2.0 + 24.0 / z_board) - DH / 2.0 / z_board,
            zoom: z_board,
        };
        let focus_card = area_geom(&b_final.areas[1], &adv);
        let to_screen = |wx: f32, wy: f32| ((wx - cam.x) * z_board, (wy - cam.y) * z_board);
        let focus_c = to_screen(
            focus_card.card.x + focus_card.card.w / 2.0,
            focus_card.card.y + focus_card.card.h / 2.0,
        );
        let ship_circle = {
            let c = focus_card.rows[0].circle;
            to_screen(c.x + c.w / 2.0, c.y + c.h / 2.0)
        };

        let ov0 = {
            let ff: &Fonts = &f;
            overlay(&b_final, DW, DH, margins, &|s, t| {
                ff.text_width(s.face(), t, s.size())
            })
        };
        let centre = |r: LRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let theme_btn = centre(
            ov0.view_btns
                .iter()
                .find(|(b, _)| *b == ViewBtn::Theme)
                .unwrap()
                .1,
        );
        let row = |t: Theme| centre(ov0.theme_items.iter().find(|(x, _)| *x == t).unwrap().1);
        let (r_neon, r_paper, r_aurora) = (row(Theme::Neon), row(Theme::Paper), row(Theme::Aurora));

        let ck: Vec<(f32, [f32; 2])> = vec![
            (0.0, [1000.0, 640.0]),
            (2.0, [1000.0, 640.0]),
            (4.7, [ship_circle.0, ship_circle.1]),
            (5.6, [ship_circle.0, ship_circle.1]),
            (6.8, [760.0, 600.0]),
            (17.2, [760.0, 600.0]),
            (18.4, [theme_btn.0, theme_btn.1]),
            (19.5, [r_neon.0, r_neon.1]),
            (20.4, [r_neon.0 + 2.0, r_neon.1 + 2.0]),
            (20.8, [r_paper.0, r_paper.1]),
            (21.6, [r_paper.0 + 2.0, r_paper.1 + 2.0]),
            (22.0, [r_aurora.0, r_aurora.1]),
            (22.9, [r_aurora.0 + 2.0, r_aurora.1 + 2.0]),
            (23.7, [760.0, 640.0]),
        ];
        let (open_at, close_at) = (18.6_f32, 23.2_f32);

        let mut blend = Blend {
            from: colors(Theme::Dark),
            to: Theme::Dark,
            t0: -10.0,
        };
        let heads: [(f32, f32, &str, &str); 5] = [
            (0.2, 7.6, "Your wallpaper", "is your productivity app"),
            (8.0, 14.8, "Open an app.", "It tiles right over it."),
            (15.1, 17.9, "Close it.", "Your board is back."),
            (18.1, 23.7, "Make it yours.", "7 live themes."),
            (24.0, SDUR, "wallboard", "your desktop, finally useful"),
        ];
        let subs: [(f32, f32, &str); 5] = [
            (0.4, 7.6, "A life-OS board on niri's background layer"),
            (
                8.2,
                14.8,
                "Browser, terminal, anything: windows tile on top",
            ),
            (15.3, 17.9, "Every empty workspace is your home screen"),
            (18.3, 23.7, "Hover a theme to preview it live"),
            (24.2, SDUR, "Rust · ~15 MB RAM · CPU-rendered"),
        ];
        // Composite camera: (zoom, centre x, centre y) in desktop-buffer px.
        let (fcx, fcy) = (focus_c.0 * 2.0, focus_c.1 * 2.0);
        let zk: Vec<(f32, [f32; 3])> = vec![
            (0.0, [1.0, 1280.0, 720.0]),
            (1.6, [1.0, 1280.0, 720.0]),
            (2.6, [1.7, fcx, fcy]),
            (7.0, [1.7, fcx, fcy]),
            (8.0, [1.0, 1280.0, 720.0]),
            (17.6, [1.0, 1280.0, 720.0]),
            (18.6, [1.3, 1700.0, 800.0]),
            (23.2, [1.3, 1700.0, 800.0]),
            (24.2, [1.0, 1280.0, 720.0]),
        ];

        let frames = (SDUR * FPS) as usize;
        for fr in 0..frames {
            let t = fr as f32 / FPS;
            let board = board_at(t + 0.9, &f, true);
            let cpos = keys(&ck, t);
            let cursor_on = t < 7.6 || (t > 17.0 && t < 24.2);
            let open = t >= open_at && t < close_at;
            let cur = if t >= 22.4 {
                Theme::Aurora
            } else {
                Theme::Dark
            };
            let ov = {
                let ff: &Fonts = &f;
                overlay(&board, DW, DH, margins, &|s, t| {
                    ff.text_width(s.face(), t, s.size())
                })
            };
            let hover = if cursor_on && t > 17.0 {
                ov.hover_at(cpos[0], cpos[1], open)
            } else {
                Hover::None
            };
            let shown = match hover {
                Hover::Theme(th) if open => th,
                _ => cur,
            };
            let palette = blend.at(shown, t);
            let current = crate::layout::current_area(&board, &cam, DW, DH, &|s, c| {
                f.advance(s.face(), c, s.size())
            });
            let scene = Scene {
                board: &board,
                editor: &editor,
                theme: cur,
                palette: Some(palette),
                background: &bg,
                wallpaper: None,
                overlay: &ov,
                current_area: current,
                show_help: false,
                detail: None,
                name_box: None,
                theme_open: open,
                hover,
                text_bar: None,
            };
            let mut dbuf = vec![0u8; (dw * dh * 4) as usize];
            draw(&mut dbuf, dw, dh, &cam, ds, &scene, &mut f);

            // Windows (above the background-layer board), then the top-layer bar, then cursor.
            {
                let v = View {
                    cam,
                    z: z_board * ds as f32,
                    scale: ds as f32,
                    w: dw,
                    h: dh,
                };
                let mut pm = PixmapMut::from_bytes(&mut dbuf, dw, dh).unwrap();
                let p_term = ss((t - 8.0) / 1.3).min(1.0 - ss((t - 15.4) / 1.2));
                let p_brow = ss((t - 9.6) / 1.3).min(1.0 - ss((t - 14.2) / 1.2));
                if p_term > 0.001 {
                    let typed = (((t - 9.3).max(0.0)) * 38.0) as usize;
                    draw_terminal(
                        &mut pm,
                        &v,
                        &mut f,
                        win_rect(true, p_term),
                        typed,
                        p_brow < 0.5,
                        ((t * 2.0) as i32) % 2 == 0,
                    );
                }
                if p_brow > 0.001 {
                    draw_browser(
                        &mut pm,
                        &v,
                        &mut f,
                        win_rect(false, p_brow),
                        (t - 10.9) / 1.4,
                        true,
                    );
                }
                draw_bar(&mut pm, &v, &mut f, 0);
                if cursor_on {
                    cursor(&mut pm, ds as f32, cpos[0], cpos[1]);
                }
            }

            // Final 1080x1920 composite.
            let mut fb = vec![0u8; (fw * fh * 4) as usize];
            {
                let vf = View {
                    cam: Camera {
                        x: 0.0,
                        y: 0.0,
                        zoom: 1.0,
                    },
                    z: 1.0,
                    scale: 1.0,
                    w: fw,
                    h: fh,
                };
                let mut pm = PixmapMut::from_bytes(&mut fb, fw, fh).unwrap();
                let (r, g, b) = bgr((10, 10, 20));
                pm.fill(Color::from_rgba8(r, g, b, 0xff));
                draw_aurora(
                    &mut pm,
                    &vf,
                    [(0x2d, 0x5b, 0xff), (0x9b, 0x3d, 0xff), (0x00, 0xd4, 0xb4)],
                );

                let (sx, sy, sw, sh) = (20.0_f32, 770.0_f32, 1040.0_f32, 585.0_f32);
                rrect(
                    &mut pm,
                    sx - 10.0,
                    sy + 6.0,
                    sw + 20.0,
                    sh + 22.0,
                    30.0,
                    (0, 0, 0),
                    120,
                );
                rrect(
                    &mut pm,
                    sx - 8.0,
                    sy - 8.0,
                    sw + 16.0,
                    sh + 16.0,
                    28.0,
                    (30, 30, 38),
                    0xff,
                );
                ring(
                    &mut pm,
                    sx - 8.0,
                    sy - 8.0,
                    sw + 16.0,
                    sh + 16.0,
                    28.0,
                    (70, 70, 86),
                    1.5,
                );
                let z = keys(&zk, t);
                let (kz, mut cx, mut cy) = (z[0], z[1], z[2]);
                let half_w = dw as f32 / 2.0 / kz;
                let half_h = dh as f32 / 2.0 / kz;
                cx = cx.clamp(half_w, dw as f32 - half_w);
                cy = cy.clamp(half_h, dh as f32 - half_h);
                let sc = sw / dw as f32 * kz;
                let tf = Transform::from_row(
                    sc,
                    0.0,
                    0.0,
                    sc,
                    sx + sw / 2.0 - cx * sc,
                    sy + sh / 2.0 - cy * sc,
                );
                let dpm =
                    Pixmap::from_vec(dbuf, tiny_skia::IntSize::from_wh(dw, dh).unwrap()).unwrap();
                let mut paint_p = Paint::default();
                paint_p.shader = tiny_skia::Pattern::new(
                    dpm.as_ref(),
                    tiny_skia::SpreadMode::Pad,
                    FilterQuality::Bicubic,
                    1.0,
                    tf,
                );
                paint_p.anti_alias = true;
                if let Some(p) = rounded_rect(sx, sy, sw, sh, 20.0) {
                    pm.fill_path(&p, &paint_p, FillRule::Winding, Transform::identity(), None);
                }
                for (a, b, l1, l2) in heads {
                    let al = ss((t - a) / 0.4).min(ss((b - t) / 0.4));
                    headline(&mut pm, &vf, &mut f, l1, l2, al);
                }
                for (a, b, text) in subs {
                    let al = ss((t - a) / 0.4).min(ss((b - t) / 0.4));
                    caption(&mut pm, &vf, &mut f, text, al, 1470.0, 25.0, false);
                }
            }
            for p in fb.as_chunks_mut::<4>().0 {
                *p = [p[2], p[1], p[0], 255];
            }
            let pm = Pixmap::from_vec(fb, tiny_skia::IntSize::from_wh(fw, fh).unwrap()).unwrap();
            pm.save_png(dir.join(format!("s_{fr:04}.png"))).unwrap();
        }
        println!("wrote {frames} frames to {}", dir.display());
    }
}

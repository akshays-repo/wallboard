//! CPU rendering into a shm buffer with tiny-skia, in an Apple (Reminders / Freeform) style.
//!
//! The shm format is Xrgb8888 (little-endian bytes B, G, R, X) while tiny-skia writes bytes as
//! R, G, B, A. Colours are therefore given to tiny-skia with red and blue swapped (see `bgr`),
//! so the screen shows the intended colour.

use crate::board::{Area, Background, Board, Note, Stroke, Theme, MAX_HERO_OPEN};
use crate::camera::Camera;
use crate::editor::{lines_of, scroll_x, style_of, EditTarget, Editor};
use crate::layout::{
    area_geom, note_geom, radius, AreaGeom, Overlay, Rect as LRect, Tool, BODY_LINE_H, CIRCLE,
    NOTE_LINE_H, NOTE_RADIUS, NOTE_TITLE_H, PAD, ROW_H, TEXT_GAP,
};
use crate::text::{blit, line_of, quantize_px, Face, Fonts, Style, MIN_GLYPH_PX};
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut,
    PixmapPaint, Rect, Stroke as SkStroke, Transform,
};

type Rgb = (u8, u8, u8);

/// A theme's colours. Dark and light follow the Apple system palettes.
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
        },
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
    pub background: &'a Background,
    /// Pre-scaled, pre-dimmed wallpaper in buffer byte order (see `build_wallpaper`).
    pub wallpaper: Option<&'a [u8]>,
    pub overlay: &'a Overlay,
    pub current_area: Option<usize>,
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
    let c = colors(scene.theme);
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
            draw_grid(&mut pm, &v, bg);
        }
    }
    let Some(mut pm) = PixmapMut::from_bytes(canvas, w, h) else {
        return;
    };

    for (ai, a) in scene.board.areas.iter().enumerate() {
        draw_area(&mut pm, &v, fonts, a, ai, scene, &c);
    }
    for n in &scene.board.notes {
        draw_note(&mut pm, &v, fonts, n, scene, &c);
    }
    for s in &scene.board.strokes {
        draw_stroke(&mut pm, &v, s, &c);
    }
    draw_overlay(&mut pm, &v, fonts, scene, &c);
}

fn draw_grid(pm: &mut PixmapMut, v: &View, bg: Rgb) {
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
    // Two hard-edged layers (no blur): a tight contact shadow and a wider, fainter one.
    let (a1, a2) = if c.dark { (60, 34) } else { (26, 14) };
    if let Some(p) = rounded_rect(x + 3.0 * v.z, y + 10.0 * v.z, w - 6.0 * v.z, h, r) {
        pm.fill_path(&p, &paint((0, 0, 0), a2, true), FillRule::Winding, tf, None);
    }
    if let Some(p) = rounded_rect(x, y + 1.5 * v.z, w, h, r) {
        pm.fill_path(&p, &paint((0, 0, 0), a1, true), FillRule::Winding, tf, None);
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
        } else {
            let shown = fonts.fit_ellipsis(Face::Regular, &t.text, Style::Row.size(), row.text.w);
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
        if let Some(body) = &row.body {
            let chars: Vec<char> = t.body.as_deref().unwrap_or("").chars().collect();
            let body_px = Style::Chip.size() * v.z;
            for (li, line) in body.lines.iter().enumerate() {
                let s: String = chars[line.start..line.end].iter().collect();
                let ly = v.y(body.rect.y + (li as f32 + 0.5) * BODY_LINE_H);
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
            let editing_body = editing.is_some_and(|e| e.target == EditTarget::TodoBody(a.id, ri));
            if editing_body {
                let e = editing.unwrap();
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
    for (li, line) in g.body.lines.iter().enumerate() {
        let s: String = chars[line.start..line.end].iter().collect();
        let cy = v.y(g.body.rect.y + (li as f32 + 0.5) * NOTE_LINE_H);
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
    let pts: Vec<(f32, f32)> = s.pts.iter().map(|&(x, y)| (v.x(x), v.y(y))).collect();
    let Some(&first) = pts.first() else {
        return;
    };
    let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
    for p in &pts {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let pad = s.width * v.z + 2.0;
    if !v.visible(x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad) {
        return;
    }
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
        if active {
            if let Some(p) = rounded_rect(x, y, w, h, 10.0 * s) {
                pm.fill_path(
                    &p,
                    &paint(c.accent[0], 0xff, true),
                    FillRule::Winding,
                    tf,
                    None,
                );
            }
        }
        let col = if active { WHITE } else { c.secondary };
        draw_tool_icon(pm, v, *tool, x + w / 2.0, y + h / 2.0, col);
    }

    // Area chips.
    if let Some(p) = o.chips_pill {
        pill(pm, v, p, c);
    }
    for (ai, r) in &o.chips {
        let Some(a) = scene.board.areas.get(*ai) else {
            continue;
        };
        let current = scene.current_area == Some(*ai);
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        let px = Style::ChipBold.size() * s;
        let tw = fonts.text_width(Face::Bold, &a.name, px);
        if current {
            if let Some(p) = rounded_rect(x, y, w, h, h / 2.0) {
                pm.fill_path(
                    &p,
                    &paint(c.accent[a.color as usize % 10], 0xff, true),
                    FillRule::Winding,
                    tf,
                    None,
                );
            }
        }
        let col = if current { WHITE } else { c.secondary };
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

    // Launcher dock.
    if let Some(p) = o.dock_pill {
        pill(pm, v, p, c);
    }
    for (li, r) in &o.dock {
        let Some(l) = scene.board.launchers.get(*li) else {
            continue;
        };
        let (x, y, w, h) = (r.x * s, r.y * s, r.w * s, r.h * s);
        if let Some(p) = rounded_rect(x, y, w, h, h / 2.0) {
            pm.fill_path(&p, &paint(c.hover, 0xff, true), FillRule::Winding, tf, None);
        }
        let px = Style::Chip.size() * s;
        let tw = fonts.text_width(Face::Regular, &l.label, px);
        text_px(
            pm,
            v,
            fonts,
            Face::Regular,
            px,
            x + (w - tw) / 2.0,
            y + h / 2.0,
            &l.label,
            c.text,
            full,
            false,
        );
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
                background: &self.bg,
                wallpaper: self.wp.as_deref(),
                overlay: &ov,
                current_area: Some(1),
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
    fn current_chip_is_filled_with_its_area_colour() {
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
        let (ai, rect) = ov.chips[1]; // Focus is the "current" area in the rig
        assert_eq!(ai, 1);
        let col = px(
            &buf,
            r.w,
            (rect.x + 3.0) as u32,
            (rect.y + rect.h / 2.0) as u32,
        );
        assert_eq!(
            col,
            colors(Theme::Dark).accent[r.board.areas[1].color as usize]
        );
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
        for (name, theme) in [("dark", Theme::Dark), ("light", Theme::Light)] {
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
                background: &bg,
                wallpaper: None,
                overlay: &ov,
                current_area: Some(1),
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

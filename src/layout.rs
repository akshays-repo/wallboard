//! Geometry: where everything is. World units for cards and notes, logical px for the
//! screen-space toolbar, chips and dock. Used by rendering, hit-testing and the editor so
//! they always agree. Pure logic: text measuring comes in as a closure.

use crate::board::{Area, Board, Note, Todo, NOTE_W};
use crate::camera::Camera;
use crate::text::{wrap, Line, Style};
use serde::{Deserialize, Serialize};

pub const PAD: f32 = 16.0;
pub const HEADER_H: f32 = 56.0;
pub const HERO_HEADER_H: f32 = 64.0;
pub const ROW_H: f32 = 38.0;
pub const FOOTER_H: f32 = 44.0;
const BOTTOM_PAD: f32 = 8.0;
pub const CIRCLE: f32 = 22.0;
pub const TEXT_GAP: f32 = 12.0;
/// Clicking this far from the right edge of a row's title line cycles its priority.
pub const PRIO_W: f32 = 52.0;
pub const DOT: f32 = 14.0;
pub const RADIUS: f32 = 20.0;
pub const HERO_RADIUS: f32 = 24.0;
/// Todo body (details) text: line height and the gap under it.
pub const BODY_LINE_H: f32 = 18.0;
const BODY_PAD: f32 = 8.0;
/// Notes.
pub const NOTE_TITLE_H: f32 = 30.0;
pub const NOTE_LINE_H: f32 = 22.0;
const NOTE_MIN_BODY_LINES: usize = 2;
pub const NOTE_RADIUS: f32 = 14.0;

/// Measures one char at a text style. Units are whatever the caller scales by (world units here).
pub type Adv<'a> = &'a dyn Fn(Style, char) -> f32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Group,
    Todo,
    Note,
    Pen,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Select,
        Tool::Group,
        Tool::Todo,
        Tool::Note,
        Tool::Pen,
        Tool::Eraser,
    ];
}

/// A wrapped multi-line text block.
#[derive(Debug, Clone)]
pub struct TextBlock {
    pub rect: Rect,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone)]
pub struct RowGeom {
    /// The whole row, including its body when shown.
    pub rect: Rect,
    pub circle: Rect,
    /// The title line's text box (clipped here).
    pub text: Rect,
    /// The right-edge zone of the title line that cycles priority.
    pub prio: Rect,
    /// The details block under the title, if the todo has one.
    pub body: Option<TextBlock>,
}

#[derive(Debug, Clone)]
pub struct AreaGeom {
    pub card: Rect,
    pub header: Rect,
    pub title: Rect,
    pub dot: Rect,
    /// y of the hairline under the header.
    pub sep_y: f32,
    pub rows: Vec<RowGeom>,
    pub footer: Rect,
}

pub fn header_h(a: &Area) -> f32 {
    if a.hero {
        HERO_HEADER_H
    } else {
        HEADER_H
    }
}

pub fn radius(a: &Area) -> f32 {
    if a.hero {
        HERO_RADIUS
    } else {
        RADIUS
    }
}

fn wrap_block(
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    style: Style,
    line_h: f32,
    min_lines: usize,
    adv: Adv,
) -> TextBlock {
    let chars: Vec<char> = text.chars().collect();
    let lines = wrap(&chars, w, &|c| adv(style, c));
    let n = lines.len().max(min_lines);
    TextBlock {
        rect: Rect::new(x, y, w, n as f32 * line_h),
        lines,
    }
}

fn row_geom(a: &Area, t: &Todo, y: f32, adv: Adv) -> RowGeom {
    let w = a.width();
    let text_x = a.x + PAD + CIRCLE + TEXT_GAP;
    let text_w = a.x + w - PRIO_W - text_x;
    let body = t.body.as_ref().map(|b| {
        wrap_block(
            b,
            text_x,
            y + ROW_H - 6.0,
            a.x + w - PAD - text_x,
            Style::Chip,
            BODY_LINE_H,
            1,
            adv,
        )
    });
    let extra = body.as_ref().map_or(0.0, |b| b.rect.h + BODY_PAD - 6.0);
    RowGeom {
        rect: Rect::new(a.x, y, w, ROW_H + extra),
        circle: Rect::new(a.x + PAD, y + (ROW_H - CIRCLE) / 2.0, CIRCLE, CIRCLE),
        text: Rect::new(text_x, y, text_w, ROW_H),
        prio: Rect::new(a.x + w - PRIO_W, y, PRIO_W, ROW_H),
        body,
    }
}

pub fn area_geom(a: &Area, adv: Adv) -> AreaGeom {
    let w = a.width();
    let hh = header_h(a);
    let header = Rect::new(a.x, a.y, w, hh);
    let dot = Rect::new(a.x + w - PAD - DOT, a.y + (hh - DOT) / 2.0, DOT, DOT);
    // Leave room for the open-count label between the title and the dot.
    let title = Rect::new(a.x + PAD, a.y, w - 2.0 * PAD - DOT - 44.0, hh);
    let sep_y = a.y + hh;
    let mut y = sep_y;
    let mut rows = Vec::with_capacity(a.todos.len());
    for t in &a.todos {
        let r = row_geom(a, t, y, adv);
        y += r.rect.h;
        rows.push(r);
    }
    let footer = Rect::new(a.x, y, w, FOOTER_H);
    let card = Rect::new(a.x, a.y, w, (y - a.y) + FOOTER_H + BOTTOM_PAD);
    AreaGeom {
        card,
        header,
        title,
        dot,
        sep_y,
        rows,
        footer,
    }
}

#[derive(Debug, Clone)]
pub struct NoteGeom {
    pub card: Rect,
    pub title: Rect,
    pub dot: Rect,
    pub body: TextBlock,
}

pub fn note_geom(n: &Note, adv: Adv) -> NoteGeom {
    let inner = NOTE_W - 2.0 * PAD;
    let body = wrap_block(
        &n.body,
        n.x + PAD,
        n.y + PAD + NOTE_TITLE_H,
        inner,
        Style::Row,
        NOTE_LINE_H,
        NOTE_MIN_BODY_LINES,
        adv,
    );
    let card = Rect::new(n.x, n.y, NOTE_W, (body.rect.bottom() + PAD) - n.y);
    NoteGeom {
        card,
        title: Rect::new(n.x + PAD, n.y + PAD, inner - DOT - 8.0, NOTE_TITLE_H),
        dot: Rect::new(
            n.x + NOTE_W - PAD - DOT,
            n.y + PAD + (NOTE_TITLE_H - DOT) / 2.0,
            DOT,
            DOT,
        ),
        body,
    }
}

/// What a world point landed on. Indices are positions in `board.areas` / `area.todos` / `board.notes`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Title(usize),
    Dot(usize),
    Circle(usize, usize),
    Text(usize, usize),
    Prio(usize, usize),
    TodoBody(usize, usize),
    NewTodo(usize),
    /// Somewhere on a card with no control (padding).
    Body(usize),
    NoteTitle(usize),
    NoteBody(usize),
    NoteDot(usize),
}

pub fn hit_test(board: &Board, wx: f32, wy: f32, adv: Adv) -> Option<Target> {
    // Notes sit above areas.
    for (ni, n) in board.notes.iter().enumerate().rev() {
        let g = note_geom(n, adv);
        if !g.card.contains(wx, wy) {
            continue;
        }
        let dot_zone = Rect::new(g.dot.x - 6.0, g.dot.y - 6.0, g.dot.w + 12.0, g.dot.h + 12.0);
        return Some(if dot_zone.contains(wx, wy) {
            Target::NoteDot(ni)
        } else if wy < g.body.rect.y {
            Target::NoteTitle(ni)
        } else {
            Target::NoteBody(ni)
        });
    }
    for (ai, a) in board.areas.iter().enumerate().rev() {
        let g = area_geom(a, adv);
        if !g.card.contains(wx, wy) {
            continue;
        }
        if g.header.contains(wx, wy) {
            let dot_zone = Rect::new(g.dot.x - 6.0, g.dot.y - 6.0, g.dot.w + 12.0, g.dot.h + 12.0);
            return Some(if dot_zone.contains(wx, wy) {
                Target::Dot(ai)
            } else {
                Target::Title(ai)
            });
        }
        for (ri, r) in g.rows.iter().enumerate() {
            if !r.rect.contains(wx, wy) {
                continue;
            }
            return Some(if wy >= r.rect.y + ROW_H {
                Target::TodoBody(ai, ri)
            } else if r.prio.contains(wx, wy) {
                Target::Prio(ai, ri)
            } else if wx < r.text.x {
                Target::Circle(ai, ri)
            } else {
                Target::Text(ai, ri)
            });
        }
        if g.footer.contains(wx, wy) {
            return Some(Target::NewTodo(ai));
        }
        return Some(Target::Body(ai));
    }
    None
}

/// Bounding box of all cards and notes.
pub fn bounds(board: &Board, adv: Adv) -> Option<Rect> {
    let rects = board
        .areas
        .iter()
        .map(|a| area_geom(a, adv).card)
        .chain(board.notes.iter().map(|n| note_geom(n, adv).card));
    let mut it = rects;
    let first = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.right());
        y1 = y1.max(r.bottom());
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// The area nearest the centre of the view (0 distance if the centre is inside a card).
pub fn current_area(board: &Board, cam: &Camera, vw: f32, vh: f32, adv: Adv) -> Option<usize> {
    let (cx, cy) = cam.screen_to_world(vw / 2.0, vh / 2.0);
    board
        .areas
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let r = area_geom(a, adv).card;
            let dx = (r.x - cx).max(0.0).max(cx - r.right());
            let dy = (r.y - cy).max(0.0).max(cy - r.bottom());
            (i, dx.hypot(dy))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Screen-space (logical px) overlay: toolbar on the left, area chips on top, launcher dock below.
#[derive(Debug, Clone)]
pub struct Overlay {
    pub tools_pill: Rect,
    pub tools: Vec<(Tool, Rect)>,
    pub chips_pill: Option<Rect>,
    /// (area index, rect)
    pub chips: Vec<(usize, Rect)>,
    pub dock_pill: Option<Rect>,
    /// (launcher index, rect)
    pub dock: Vec<(usize, Rect)>,
}

const CHIP_H: f32 = 28.0;
const CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;
const PILL_PAD: f32 = 6.0;
const BTN_H: f32 = 32.0;
const BTN_PAD: f32 = 16.0;
const BTN_GAP: f32 = 8.0;
pub const TOOL_SIZE: f32 = 40.0;
const TOOL_GAP: f32 = 4.0;
const TOOLS_LEFT: f32 = 14.0;
/// How far the chips and the dock stay clear of the screen edges. A shell bar or dock on the
/// compositor's top layer sits above wallboard and swallows clicks there, so these default to
/// clearing a typical bar and dock. Tune them with `"ui": {"top": .., "bottom": ..}` in board.json.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    /// Gap above the area chips (logical px).
    #[serde(default = "default_top")]
    pub top: f32,
    /// Gap below the launcher dock (logical px).
    #[serde(default = "default_bottom")]
    pub bottom: f32,
}

fn default_top() -> f32 {
    64.0
}

fn default_bottom() -> f32 {
    52.0
}

impl Default for Margins {
    fn default() -> Self {
        Margins {
            top: default_top(),
            bottom: default_bottom(),
        }
    }
}

impl Margins {
    /// Clamp to sane values so a bad number in the file cannot push the UI off screen.
    pub fn sane(self) -> Margins {
        let f = |v: f32, d: f32| {
            if v.is_finite() {
                v.clamp(0.0, 400.0)
            } else {
                d
            }
        };
        Margins {
            top: f(self.top, default_top()),
            bottom: f(self.bottom, default_bottom()),
        }
    }
}

/// Lay a row of items out centred in `[0, vw]`. Returns (pill, item rects) or None if empty.
fn row_of(widths: &[f32], vw: f32, y: f32, h: f32, gap: f32) -> Option<(Rect, Vec<Rect>)> {
    if widths.is_empty() {
        return None;
    }
    let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0);
    let pill_w = total + 2.0 * PILL_PAD;
    let px = (vw - pill_w) / 2.0;
    let mut x = px + PILL_PAD;
    let mut rects = Vec::with_capacity(widths.len());
    for w in widths {
        rects.push(Rect::new(x, y + PILL_PAD, *w, h));
        x += w + gap;
    }
    Some((Rect::new(px, y, pill_w, h + 2.0 * PILL_PAD), rects))
}

pub fn overlay(
    board: &Board,
    vw: f32,
    vh: f32,
    m: Margins,
    tw: &dyn Fn(Style, &str) -> f32,
) -> Overlay {
    let m = m.sane();
    // Toolbar: a vertical pill at the left, centred vertically.
    let n = Tool::ALL.len() as f32;
    let th = n * TOOL_SIZE + (n - 1.0) * TOOL_GAP + 2.0 * PILL_PAD;
    let ty = ((vh - th) / 2.0).max(m.top + 50.0);
    let tools_pill = Rect::new(TOOLS_LEFT, ty, TOOL_SIZE + 2.0 * PILL_PAD, th);
    let tools = Tool::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let y = ty + PILL_PAD + i as f32 * (TOOL_SIZE + TOOL_GAP);
            (
                *t,
                Rect::new(TOOLS_LEFT + PILL_PAD, y, TOOL_SIZE, TOOL_SIZE),
            )
        })
        .collect();

    // Chips: as many areas as fit.
    let max_w = (vw - 40.0).max(0.0);
    let mut widths = Vec::new();
    let mut used = 2.0 * PILL_PAD;
    for a in &board.areas {
        let w = tw(Style::ChipBold, &a.name) + 2.0 * CHIP_PAD;
        let next = used + w + if widths.is_empty() { 0.0 } else { CHIP_GAP };
        if next > max_w {
            break;
        }
        used = next;
        widths.push(w);
    }
    let (chips_pill, chips) = match row_of(&widths, vw, m.top, CHIP_H, CHIP_GAP) {
        Some((p, r)) => (Some(p), r.into_iter().enumerate().collect()),
        None => (None, Vec::new()),
    };

    let dwidths: Vec<f32> = board
        .launchers
        .iter()
        .map(|l| tw(Style::Chip, &l.label) + 2.0 * BTN_PAD)
        .collect();
    let dy = vh - m.bottom - BTN_H - 2.0 * PILL_PAD;
    let (dock_pill, dock) = match row_of(&dwidths, vw, dy, BTN_H, BTN_GAP) {
        Some((p, r)) => (Some(p), r.into_iter().enumerate().collect()),
        None => (None, Vec::new()),
    };
    Overlay {
        tools_pill,
        tools,
        chips_pill,
        chips,
        dock_pill,
        dock,
    }
}

impl Overlay {
    pub fn tool_at(&self, x: f32, y: f32) -> Option<Tool> {
        self.tools
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(t, _)| *t)
    }
    pub fn chip_at(&self, x: f32, y: f32) -> Option<usize> {
        self.chips
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(i, _)| *i)
    }
    pub fn dock_at(&self, x: f32, y: f32) -> Option<usize> {
        self.dock
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(i, _)| *i)
    }
    /// True if the point is over any pill (clicks there must not reach the canvas).
    pub fn covers(&self, x: f32, y: f32) -> bool {
        self.tools_pill.contains(x, y)
            || self.chips_pill.is_some_and(|r| r.contains(x, y))
            || self.dock_pill.is_some_and(|r| r.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Todo;

    fn adv(_: Style, _: char) -> f32 {
        7.0
    }
    const A: Adv = &adv;
    fn tw(_: Style, s: &str) -> f32 {
        s.chars().count() as f32 * 8.0
    }

    fn board_with_todos(n: usize) -> Board {
        let mut b = Board::default_board();
        b.areas[2].todos = (0..n).map(|i| Todo::new(&format!("t{i}"))).collect();
        b
    }

    #[test]
    fn card_height_grows_with_rows() {
        let h0 = area_geom(&board_with_todos(0).areas[2], A).card.h;
        let b = board_with_todos(4);
        let g = area_geom(&b.areas[2], A);
        assert_eq!(g.card.h, h0 + 4.0 * ROW_H);
        assert_eq!(g.rows.len(), 4);
        assert_eq!(g.footer.y, g.sep_y + 4.0 * ROW_H);
        assert!(g.card.bottom() >= g.footer.bottom());
    }

    #[test]
    fn rows_are_38_units_and_stack_without_gaps() {
        let b = board_with_todos(3);
        let g = area_geom(&b.areas[2], A);
        for (i, r) in g.rows.iter().enumerate() {
            assert_eq!(r.rect.h, 38.0);
            assert_eq!(r.rect.y, g.sep_y + i as f32 * 38.0);
            assert!((r.circle.y + r.circle.h / 2.0 - (r.rect.y + 19.0)).abs() < 1e-3);
        }
    }

    #[test]
    fn body_makes_a_row_taller_and_pushes_rows_below() {
        let mut b = board_with_todos(3);
        let before = area_geom(&b.areas[2], A);
        b.areas[2].todos[1].body = Some("line one\nline two".into());
        let g = area_geom(&b.areas[2], A);
        assert!(g.rows[1].rect.h > ROW_H);
        assert!(g.rows[1].body.is_some() && g.rows[0].body.is_none());
        assert_eq!(g.rows[1].body.as_ref().unwrap().lines.len(), 2);
        assert!(
            g.rows[2].rect.y > before.rows[2].rect.y,
            "rows below move down"
        );
        assert_eq!(g.rows[2].rect.y, g.rows[1].rect.bottom());
        assert!(g.card.h > before.card.h);
        // an empty-but-open body still takes one line (so it can be typed into)
        b.areas[2].todos[0].body = Some(String::new());
        assert!(area_geom(&b.areas[2], A).rows[0].rect.h > ROW_H);
    }

    #[test]
    fn long_body_wraps() {
        let mut b = board_with_todos(1);
        b.areas[2].todos[0].body = Some("word ".repeat(40));
        let g = area_geom(&b.areas[2], A);
        assert!(g.rows[0].body.as_ref().unwrap().lines.len() > 2);
    }

    #[test]
    fn hero_is_bigger() {
        let b = Board::default_board();
        let (inbox, focus) = (area_geom(&b.areas[0], A), area_geom(&b.areas[1], A));
        assert!(focus.card.w > inbox.card.w);
        assert!(focus.header.h > inbox.header.h);
    }

    fn c(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn hit_test_every_area_control() {
        let mut b = board_with_todos(2);
        b.areas[2].todos[1].body = Some("details".into());
        let g = area_geom(&b.areas[2], A);
        let ai = 2;
        let (x, y) = c(g.title);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Title(ai)));
        let (x, y) = c(g.dot);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Dot(ai)));
        let (x, y) = c(g.rows[1].circle);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Circle(ai, 1)));
        let (x, y) = c(g.rows[1].text);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Text(ai, 1)));
        let (x, y) = c(g.rows[0].prio);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Prio(ai, 0)));
        let (x, y) = c(g.rows[1].body.as_ref().unwrap().rect);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::TodoBody(ai, 1)));
        let (x, y) = c(g.footer);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::NewTodo(ai)));
        let (x, y) = (g.card.x + 4.0, g.card.bottom() - 2.0);
        assert_eq!(hit_test(&b, x, y, A), Some(Target::Body(ai)));
        assert_eq!(hit_test(&b, -9999.0, -9999.0, A), None);
    }

    #[test]
    fn note_geometry_and_hits() {
        let mut b = Board::new();
        let id = b.add_note(500.0, 500.0);
        let g = note_geom(b.note(id).unwrap(), A);
        assert_eq!(g.card.w, NOTE_W);
        let h0 = g.card.h;
        b.note_mut(id).unwrap().body = "x\ny\nz\nw\nv".into();
        let g = note_geom(b.note(id).unwrap(), A);
        assert!(g.card.h > h0, "more lines, taller note");
        assert_eq!(
            hit_test(&b, c(g.title).0, c(g.title).1, A),
            Some(Target::NoteTitle(0))
        );
        assert_eq!(
            hit_test(&b, c(g.dot).0, c(g.dot).1, A),
            Some(Target::NoteDot(0))
        );
        assert_eq!(
            hit_test(&b, c(g.body.rect).0, c(g.body.rect).1, A),
            Some(Target::NoteBody(0))
        );
    }

    #[test]
    fn notes_are_hit_before_areas_beneath() {
        let mut b = Board::default_board();
        let a = area_geom(&b.areas[2], A).card;
        let id = b.add_note(a.x + 20.0, a.y + 20.0);
        let g = note_geom(b.note(id).unwrap(), A);
        let (x, y) = c(g.body.rect);
        assert!(matches!(hit_test(&b, x, y, A), Some(Target::NoteBody(0))));
    }

    #[test]
    fn bounds_cover_cards_and_notes() {
        let mut b = Board::default_board();
        let far = b.add_note(9000.0, -500.0);
        let r = bounds(&b, A).unwrap();
        let n = note_geom(b.note(far).unwrap(), A).card;
        assert!(r.right() >= n.right() && r.y <= n.y);
        assert!(bounds(&Board::new(), A).is_none());
    }

    #[test]
    fn current_area_follows_the_view_centre() {
        let b = Board::default_board();
        let card = area_geom(&b.areas[4], A).card;
        let cam = Camera {
            x: card.x + card.w / 2.0 - 960.0,
            y: card.y + card.h / 2.0 - 540.0,
            zoom: 1.0,
        };
        assert_eq!(current_area(&b, &cam, 1920.0, 1080.0, A), Some(4));
        assert_eq!(current_area(&Board::new(), &cam, 1920.0, 1080.0, A), None);
    }

    #[test]
    fn overlay_toolbar_chips_and_dock() {
        let b = Board::default_board();
        let o = overlay(&b, 1920.0, 1080.0, Margins::default(), &tw);
        assert_eq!(o.tools.len(), 6);
        assert_eq!(o.tools[0].0, Tool::Select);
        assert!(o.tools_pill.x < 30.0, "toolbar hugs the left edge");
        let mid = o.tools_pill.y + o.tools_pill.h / 2.0;
        assert!((mid - 540.0).abs() < 1.0, "vertically centred");
        for w in o.tools.windows(2) {
            assert!(w[1].1.y >= w[0].1.bottom(), "tool buttons do not overlap");
        }
        let (t, r) = o.tools[4];
        assert_eq!(o.tool_at(r.x + 2.0, r.y + 2.0), Some(t));
        assert_eq!(o.chips.len(), 7);
        assert_eq!(o.dock.len(), 3);
        let p = o.chips_pill.unwrap();
        assert!(((p.x + p.w / 2.0) - 960.0).abs() < 1e-3, "pill is centred");
        let d = o.dock_pill.unwrap();
        assert!(
            d.bottom() < 1080.0 - 40.0,
            "dock clears a bottom shell dock: {}",
            d.bottom()
        );
        let (i, r) = o.chips[3];
        assert_eq!(o.chip_at(r.x + 2.0, r.y + 2.0), Some(i));
        let (i, r) = o.dock[1];
        assert_eq!(o.dock_at(r.x + 2.0, r.y + 2.0), Some(i));
        assert!(o.covers(p.x + 1.0, p.y + 1.0));
        assert!(o.covers(o.tools_pill.x + 1.0, o.tools_pill.y + 1.0));
        assert!(!o.covers(960.0, 500.0));
    }

    #[test]
    fn overlay_drops_chips_that_do_not_fit_and_survives_small_screens() {
        let b = Board::default_board();
        let o = overlay(&b, 300.0, 600.0, Margins::default(), &tw);
        assert!(o.chips.len() < 7);
        let p = o.chips_pill.unwrap();
        assert!(p.x >= 0.0 && p.right() <= 300.0);
        overlay(&b, 100.0, 100.0, Margins::default(), &tw); // must not panic
    }

    #[test]
    fn chips_clear_a_top_bar_and_margins_are_tunable_and_safe() {
        let b = Board::default_board();
        let o = overlay(&b, 1920.0, 1080.0, Margins::default(), &tw);
        assert!(
            o.chips_pill.unwrap().y >= 60.0,
            "default clears a ~50px shell bar"
        );
        let o2 = overlay(
            &b,
            1920.0,
            1080.0,
            Margins {
                top: 100.0,
                bottom: 10.0,
            },
            &tw,
        );
        assert_eq!(o2.chips_pill.unwrap().y, 100.0);
        assert!(o2.dock_pill.unwrap().bottom() > o.dock_pill.unwrap().bottom());
        // absurd values are clamped rather than pushing the UI off screen
        let o3 = overlay(
            &b,
            1920.0,
            1080.0,
            Margins {
                top: f32::NAN,
                bottom: 1.0e9,
            },
            &tw,
        );
        assert_eq!(o3.chips_pill.unwrap().y, 64.0);
        assert!(o3.dock_pill.unwrap().y > 0.0);
        let m: Margins = serde_json::from_str("{}").unwrap();
        assert_eq!(m, Margins::default());
    }

    #[test]
    fn overlay_without_launchers_has_no_dock() {
        let mut b = Board::default_board();
        b.launchers.clear();
        let o = overlay(&b, 1920.0, 1080.0, Margins::default(), &tw);
        assert!(o.dock_pill.is_none() && o.dock.is_empty());
    }
}

//! Geometry: where everything is. World units for cards and notes, logical px for the
//! screen-space toolbar, view menu and chips. Used by rendering, hit-testing and the editor so
//! they always agree. Pure logic: text measuring comes in as a closure.

use crate::board::{
    Area, Board, Connector, End, Note, Shape, ShapeKind, TextBox, TextSize, Theme, Todo,
};
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
    Text,
    /// Click to drop an emoji sticker.
    Sticker,
    /// Button: paste the clipboard image (never stays armed).
    Image,
    Rect,
    Ellipse,
    Line,
    Connector,
    Pen,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 13] = [
        Tool::Select,
        Tool::Group,
        Tool::Todo,
        Tool::Note,
        Tool::Text,
        Tool::Sticker,
        Tool::Image,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Line,
        Tool::Connector,
        Tool::Pen,
        Tool::Eraser,
    ];

    /// The shape this tool draws, if it is a shape tool.
    pub fn shape_kind(self) -> Option<ShapeKind> {
        match self {
            Tool::Rect => Some(ShapeKind::Rect),
            Tool::Ellipse => Some(ShapeKind::Ellipse),
            Tool::Line => Some(ShapeKind::Line),
            _ => None,
        }
    }
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
    let nw = n.width();
    let inner = nw - 2.0 * PAD;
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
    let card = Rect::new(n.x, n.y, nw, (body.rect.bottom() + PAD) - n.y);
    NoteGeom {
        card,
        title: Rect::new(n.x + PAD, n.y + PAD, inner - DOT - 8.0, NOTE_TITLE_H),
        dot: Rect::new(
            n.x + nw - PAD - DOT,
            n.y + PAD + (NOTE_TITLE_H - DOT) / 2.0,
            DOT,
            DOT,
        ),
        body,
    }
}

/// The item that is currently selected (shows resize handles, can be deleted with Delete).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sel {
    Area(u64),
    Note(u64),
    Shape(u64),
    Text(u64),
    Connector(u64),
}

/// A resize handle on the selected item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// The two end points of a line.
    End1,
    End2,
}

impl Handle {
    pub fn is_left(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::BottomLeft)
    }
}

/// Every item whose bounds touch `r` (a world rectangle), in a stable order.
pub fn items_in_rect(board: &Board, r: Rect, adv: Adv) -> Vec<Sel> {
    let all = board
        .areas
        .iter()
        .map(|a| Sel::Area(a.id))
        .chain(board.notes.iter().map(|n| Sel::Note(n.id)))
        .chain(board.shapes.iter().map(|s| Sel::Shape(s.id)))
        .chain(board.texts.iter().map(|t| Sel::Text(t.id)))
        .chain(board.connectors.iter().map(|c| Sel::Connector(c.id)));
    all.filter(|&sel| {
        selection_rect(board, sel, adv).is_some_and(|b| {
            b.x < r.x + r.w && b.x + b.w > r.x && b.y < r.y + r.h && b.y + b.h > r.y
        })
    })
    .collect()
}

/// Bounding rectangle of the selected item.
pub fn selection_rect(board: &Board, sel: Sel, adv: Adv) -> Option<Rect> {
    Some(match sel {
        Sel::Area(id) => area_geom(board.area(id)?, adv).card,
        Sel::Note(id) => note_geom(board.note(id)?, adv).card,
        Sel::Text(id) => text_geom(board.text(id)?, adv).rect,
        Sel::Shape(id) => {
            let (x, y, w, h) = board.shape(id)?.bbox();
            Rect::new(x, y, w, h)
        }
        Sel::Connector(id) => {
            let c = board.connectors.iter().find(|c| c.id == id)?;
            let path = connector_path(board, c, adv);
            let (mut x0, mut y0) = (f32::MAX, f32::MAX);
            let (mut x1, mut y1) = (f32::MIN, f32::MIN);
            for p in &path {
                (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
            }
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    })
}

/// Where the resize handles of the selection are (world points). Texts have none.
pub fn handles(board: &Board, sel: Sel, adv: Adv) -> Vec<(Handle, (f32, f32))> {
    if let Sel::Shape(id) = sel {
        if let Some(s) = board.shape(id) {
            if s.kind == ShapeKind::Line {
                return vec![(Handle::End1, (s.x1, s.y1)), (Handle::End2, (s.x2, s.y2))];
            }
        }
    }
    if let Sel::Connector(id) = sel {
        let Some(c) = board.connectors.iter().find(|c| c.id == id) else {
            return Vec::new();
        };
        let path = connector_path(board, c, adv);
        return match (path.first(), path.last()) {
            (Some(&a), Some(&b)) => vec![(Handle::End1, a), (Handle::End2, b)],
            _ => Vec::new(),
        };
    }
    if matches!(sel, Sel::Text(_)) {
        return Vec::new();
    }
    let Some(r) = selection_rect(board, sel, adv) else {
        return Vec::new();
    };
    vec![
        (Handle::TopLeft, (r.x, r.y)),
        (Handle::TopRight, (r.right(), r.y)),
        (Handle::BottomLeft, (r.x, r.bottom())),
        (Handle::BottomRight, (r.right(), r.bottom())),
    ]
}

/// The handle under a world point, if any. `tol` is the grab radius in world units.
pub fn hit_handle(board: &Board, sel: Sel, wx: f32, wy: f32, tol: f32, adv: Adv) -> Option<Handle> {
    handles(board, sel, adv)
        .into_iter()
        .filter(|(_, (hx, hy))| (hx - wx).abs() <= tol && (hy - wy).abs() <= tol)
        .min_by(|a, b| {
            let da = (a.1 .0 - wx).hypot(a.1 .1 - wy);
            let db = (b.1 .0 - wx).hypot(b.1 .1 - wy);
            da.total_cmp(&db)
        })
        .map(|(h, _)| h)
}

/// Line height of medium free text, world units (`TextBox::line_h` for any size).
#[allow(dead_code)]
pub const TEXT_LINE_H: f32 = 28.0;
const TEXT_MIN_W: f32 = 64.0;

#[derive(Debug, Clone)]
pub struct TextGeom {
    pub rect: Rect,
    /// Lines of the raw text (what the editor works on).
    pub lines: Vec<Line>,
    /// Height of one raw line.
    pub line_h: f32,
}

/// Free text is as wide as its longest line (no wrapping). The box covers both the raw text and
/// its rendered Markdown, so it does not jump when editing starts or stops.
pub fn text_geom(t: &TextBox, adv: Adv) -> TextGeom {
    let st = t.style();
    let line_h = t.line_h();
    let chars: Vec<char> = t.text.chars().collect();
    let lines = wrap(&chars, f32::MAX, &|c| adv(st, c));
    let mut w = lines
        .iter()
        .map(|l| chars[l.start..l.end].iter().map(|&c| adv(st, c)).sum::<f32>())
        .fold(0.0_f32, f32::max);
    let mut h = lines.len() as f32 * line_h;
    if !t.plain {
        let md = crate::md::layout(&t.text, t.size, t.bold, adv);
        (w, h) = (w.max(md.w), h.max(md.h));
    }
    if t.italic {
        w += t.size.px() * 0.2;
    }
    TextGeom {
        rect: Rect::new(t.x, t.y, w.max(TEXT_MIN_W), h),
        lines,
        line_h,
    }
}

/// A button on the floating format bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmtBtn {
    Bold,
    Italic,
    Underline,
    Strike,
    /// Inline `code`.
    Code,
    /// Cycle `#`, `##`, `###`, none on the line.
    Heading,
    Bullet,
    Task,
    Size(TextSize),
    /// 0 = theme text colour, else an accent index.
    Color(u8),
    /// Toggle Markdown rendering.
    Markdown,
}

/// Text colours offered on the format bar (0 = theme text colour).
pub const TEXT_COLORS: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// What the format bar is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    /// A free text box: every button, plus the drag grip.
    Text(u64),
    /// A multi-line field (note body, todo details): inline and line formats.
    Block,
    /// A single-line field (todo, titles): inline formats.
    Inline,
}

/// The format bar (and, for free text, its drag grip), in screen (logical) px.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBar {
    pub kind: BarKind,
    pub bar: Rect,
    pub btns: Vec<(FmtBtn, Rect)>,
    /// Separators between button groups (x positions).
    pub seps: Vec<f32>,
    /// The handle left of a free text: drag it to move the text, even while typing.
    pub grip: Option<Rect>,
}

impl TextBar {
    pub fn btn_at(&self, x: f32, y: f32) -> Option<FmtBtn> {
        self.btns.iter().find(|(_, r)| r.contains(x, y)).map(|(b, _)| *b)
    }
    pub fn on_grip(&self, x: f32, y: f32) -> bool {
        self.grip.is_some_and(|g| g.contains(x, y))
    }
    pub fn covers(&self, x: f32, y: f32) -> bool {
        self.bar.contains(x, y) || self.on_grip(x, y)
    }
    /// The free text this bar belongs to.
    pub fn text_id(&self) -> Option<u64> {
        match self.kind {
            BarKind::Text(id) => Some(id),
            _ => None,
        }
    }
}

const BAR_BTN: f32 = 28.0;
const BAR_SWATCH: f32 = 22.0;
const BAR_PAD: f32 = 5.0;
const BAR_SEP: f32 = 9.0;

/// The format bar for a field at world rectangle `r`, placed above it (below if no room).
pub fn format_bar(
    kind: BarKind,
    r: Rect,
    first_line_h: f32,
    cam: &crate::camera::Camera,
    (vw, vh): (f32, f32),
) -> TextBar {
    use FmtBtn::*;
    let (sx, sy) = cam.world_to_screen(r.x, r.y);
    let sh = r.h * cam.zoom;
    let inline = vec![Bold, Italic, Strike, Code];
    let lines = vec![Heading, Bullet, Task];
    let groups: Vec<Vec<FmtBtn>> = match kind {
        BarKind::Text(_) => vec![
            vec![Bold, Italic, Underline, Strike],
            vec![Code, Heading, Bullet, Task],
            TextSize::ALL.map(Size).to_vec(),
            TEXT_COLORS.map(Color).to_vec(),
            vec![Markdown],
        ],
        BarKind::Block => vec![inline, lines],
        BarKind::Inline => vec![inline],
    };
    let mut btns = Vec::new();
    let mut seps = Vec::new();
    let mut x = BAR_PAD;
    for (gi, list) in groups.iter().enumerate() {
        if gi > 0 {
            seps.push(x + BAR_SEP / 2.0 - 1.0);
            x += BAR_SEP;
        }
        let bw = match list[0] {
            Color(_) => BAR_SWATCH,
            Markdown => BAR_BTN + 10.0,
            _ => BAR_BTN,
        };
        for b in list {
            btns.push((*b, Rect::new(x, BAR_PAD, bw, BAR_BTN)));
            x += bw + 2.0;
        }
    }
    let bw = x - 2.0 + BAR_PAD;
    let bh = BAR_BTN + 2.0 * BAR_PAD;
    let bx = sx.clamp(8.0, (vw - bw - 8.0).max(8.0));
    // Above the field if there is room (clear of the top menu), else below it.
    let above = sy - 12.0 - bh;
    let by = if above >= 64.0 || sy + sh + 12.0 + bh > vh {
        above.max(8.0)
    } else {
        sy + sh + 12.0
    };
    for (_, b) in &mut btns {
        b.x += bx;
        b.y += by;
    }
    let seps = seps.into_iter().map(|s| s + bx).collect();
    let grip = matches!(kind, BarKind::Text(_)).then(|| {
        let first = first_line_h * cam.zoom;
        let gh = 26.0_f32.max(first.min(40.0));
        Rect::new(sx - 26.0, sy + (first - gh) / 2.0, 20.0, gh)
    });
    TextBar {
        kind,
        bar: Rect::new(bx, by, bw, bh),
        btns,
        seps,
        grip,
    }
}

/// The format bar and grip of free text `id`.
pub fn text_bar(
    board: &Board,
    id: u64,
    cam: &crate::camera::Camera,
    view: (f32, f32),
    adv: Adv,
) -> Option<TextBar> {
    let t = board.text(id)?;
    let r = text_geom(t, adv).rect;
    let first = t.line_h().max(t.size.px());
    Some(format_bar(BarKind::Text(id), r, first, cam, view))
}

/// True if a world point is on a shape (inside a rectangle/ellipse, near a line).
pub fn shape_hit(s: &Shape, wx: f32, wy: f32) -> bool {
    let (x, y, w, h) = s.bbox();
    match s.kind {
        ShapeKind::Rect | ShapeKind::Image | ShapeKind::Sticker => {
            Rect::new(x, y, w, h).contains(wx, wy)
        }
        ShapeKind::Ellipse => {
            let (rx, ry) = ((w / 2.0).max(1.0), (h / 2.0).max(1.0));
            let (dx, dy) = ((wx - (x + w / 2.0)) / rx, (wy - (y + h / 2.0)) / ry);
            dx * dx + dy * dy <= 1.0
        }
        ShapeKind::Line => {
            let (ax, ay, bx, by) = (s.x1, s.y1, s.x2, s.y2);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 == 0.0 {
                0.0
            } else {
                (((wx - ax) * dx + (wy - ay) * dy) / len2).clamp(0.0, 1.0)
            };
            (ax + t * dx - wx).hypot(ay + t * dy - wy) <= 8.0
        }
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
    /// Free text (index into `board.texts`).
    FreeText(usize),
    /// A shape (index into `board.shapes`).
    Shape(usize),
    /// A connector arrow (index into `board.connectors`).
    Connector(usize),
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
    for (ti, t) in board.texts.iter().enumerate().rev() {
        if text_geom(t, adv).rect.contains(wx, wy) {
            return Some(Target::FreeText(ti));
        }
    }
    for (si, s) in board.shapes.iter().enumerate().rev() {
        if shape_hit(s, wx, wy) {
            return Some(Target::Shape(si));
        }
    }
    for (ci, c) in board.connectors.iter().enumerate().rev() {
        if path_near(&connector_path(board, c, adv), wx, wy, 8.0) {
            return Some(Target::Connector(ci));
        }
    }
    None
}

/// Rectangle of a card, note, text or shape by id.
pub fn item_rect(board: &Board, id: u64, adv: Adv) -> Option<Rect> {
    if let Some(a) = board.area(id) {
        return Some(area_geom(a, adv).card);
    }
    if let Some(n) = board.note(id) {
        return Some(note_geom(n, adv).card);
    }
    if let Some(t) = board.text(id) {
        return Some(text_geom(t, adv).rect);
    }
    let (x, y, w, h) = board.shape(id)?.bbox();
    Some(Rect::new(x, y, w, h))
}

/// The id of the item under a world point (cards, notes, texts, shapes), if any.
pub fn item_at(board: &Board, wx: f32, wy: f32, adv: Adv) -> Option<u64> {
    match hit_test(board, wx, wy, adv)? {
        Target::NoteTitle(i) | Target::NoteBody(i) | Target::NoteDot(i) => {
            board.notes.get(i).map(|n| n.id)
        }
        Target::FreeText(i) => board.texts.get(i).map(|t| t.id),
        Target::Shape(i) => board.shapes.get(i).map(|s| s.id),
        Target::Connector(_) => None,
        Target::Title(a)
        | Target::Dot(a)
        | Target::NewTodo(a)
        | Target::Body(a)
        | Target::Circle(a, _)
        | Target::Text(a, _)
        | Target::Prio(a, _)
        | Target::TodoBody(a, _) => board.areas.get(a).map(|a| a.id),
    }
}

/// True if a world point is within `tol` of a polyline.
pub fn path_near(path: &[(f32, f32)], wx: f32, wy: f32, tol: f32) -> bool {
    path.windows(2).any(|w| {
        let ((ax, ay), (bx, by)) = (w[0], w[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 == 0.0 {
            0.0
        } else {
            (((wx - ax) * dx + (wy - ay) * dy) / len2).clamp(0.0, 1.0)
        };
        (ax + t * dx - wx).hypot(ay + t * dy - wy) <= tol
    })
}

/// Gap kept between an arrow's bend and the items, and the elbow corner radius.
const CONN_STUB: f32 = 28.0;
const CONN_RADIUS: f32 = 14.0;
/// Items closer than this count as touching on that axis (no room for a side-to-side run).
const CONN_GAP: f32 = 24.0;

/// A connector's route as a smooth polyline in world units: an elbow path that leaves each
/// attached item from the side facing the other end, with rounded corners. Recomputed from the
/// items' current positions, so it follows them when they move or resize.
pub fn connector_path(board: &Board, c: &Connector, adv: Adv) -> Vec<(f32, f32)> {
    let rect_of = |e: &End| {
        if e.id != 0 {
            item_rect(board, e.id, adv)
        } else {
            None
        }
    };
    round_corners(&route(
        rect_of(&c.from),
        (c.from.x, c.from.y),
        rect_of(&c.to),
        (c.to.x, c.to.y),
    ))
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Sharp-cornered elbow route between two ends (an item rect, or a free point).
pub fn route(
    ra: Option<Rect>,
    pa: (f32, f32),
    rb: Option<Rect>,
    pb: (f32, f32),
) -> Vec<(f32, f32)> {
    let ca = ra.map_or(pa, center);
    let cb = rb.map_or(pb, center);
    // Horizontal run if the ends are separated left/right, vertical if separated up/down;
    // otherwise (overlapping) follow the dominant direction between the centres.
    let (hsep, vsep) = match (ra, rb) {
        (Some(a), Some(b)) => (
            a.right() + CONN_GAP <= b.x || b.right() + CONN_GAP <= a.x,
            a.bottom() + CONN_GAP <= b.y || b.bottom() + CONN_GAP <= a.y,
        ),
        _ => (true, true),
    };
    let (dx, dy) = (cb.0 - ca.0, cb.1 - ca.1);
    let horizontal = match (hsep, vsep) {
        (true, false) => true,
        (false, true) => false,
        _ => dx.abs() >= dy.abs(),
    };
    let sgn = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    // Anchor points on the facing sides; where the two items overlap on the cross axis, both
    // anchors share the middle of the overlap so the arrow comes out straight.
    let cross = |lo_a: f32, hi_a: f32, lo_b: f32, hi_b: f32, fallback: f32| {
        let (lo, hi) = (lo_a.max(lo_b), hi_a.min(hi_b));
        if hi - lo >= 8.0 {
            (lo + hi) / 2.0
        } else {
            fallback
        }
    };
    let (p0, d0, p1, d1);
    if horizontal {
        let s = sgn(dx);
        let y_over = match (ra, rb) {
            (Some(a), Some(b)) => Some(cross(a.y, a.bottom(), b.y, b.bottom(), f32::NAN)),
            _ => None,
        }
        .filter(|y| y.is_finite());
        let ya = y_over
            .unwrap_or_else(|| ra.map_or(pa.1, |a| clampf(cb.1, a.y + 8.0, a.bottom() - 8.0)));
        let yb = y_over
            .unwrap_or_else(|| rb.map_or(pb.1, |b| clampf(ca.1, b.y + 8.0, b.bottom() - 8.0)));
        let xa = ra.map_or(pa.0, |a| if s > 0.0 { a.right() } else { a.x });
        let xb = rb.map_or(pb.0, |b| if s > 0.0 { b.x } else { b.right() });
        (p0, d0, p1, d1) = ((xa, ya), (s, 0.0), (xb, yb), (-s, 0.0));
    } else {
        let s = sgn(dy);
        let x_over = match (ra, rb) {
            (Some(a), Some(b)) => Some(cross(a.x, a.right(), b.x, b.right(), f32::NAN)),
            _ => None,
        }
        .filter(|x| x.is_finite());
        let xa =
            x_over.unwrap_or_else(|| ra.map_or(pa.0, |a| clampf(cb.0, a.x + 8.0, a.right() - 8.0)));
        let xb =
            x_over.unwrap_or_else(|| rb.map_or(pb.0, |b| clampf(ca.0, b.x + 8.0, b.right() - 8.0)));
        let ya = ra.map_or(pa.1, |a| if s > 0.0 { a.bottom() } else { a.y });
        let yb = rb.map_or(pb.1, |b| if s > 0.0 { b.y } else { b.bottom() });
        (p0, d0, p1, d1) = ((xa, ya), (0.0, s), (xb, yb), (0.0, -s));
    }
    // Both ends travel along the same axis: a Z (or a C-shaped detour if they face backwards).
    let forward = (p1.0 - p0.0) * d0.0 + (p1.1 - p0.1) * d0.1 > 1.0;
    let mut pts = vec![p0];
    if forward {
        if horizontal {
            let mx = (p0.0 + p1.0) / 2.0;
            pts.extend([(mx, p0.1), (mx, p1.1)]);
        } else {
            let my = (p0.1 + p1.1) / 2.0;
            pts.extend([(p0.0, my), (p1.0, my)]);
        }
    } else {
        let (o0, o1) = (
            (p0.0 + d0.0 * CONN_STUB, p0.1 + d0.1 * CONN_STUB),
            (p1.0 + d1.0 * CONN_STUB, p1.1 + d1.1 * CONN_STUB),
        );
        if horizontal {
            let my = (p0.1 + p1.1) / 2.0;
            let my = if (p0.1 - p1.1).abs() < 1.0 {
                p0.1 - CONN_STUB
            } else {
                my
            };
            pts.extend([(o0.0, p0.1), (o0.0, my), (o1.0, my), (o1.0, p1.1)]);
        } else {
            let mx = (p0.0 + p1.0) / 2.0;
            let mx = if (p0.0 - p1.0).abs() < 1.0 {
                p0.0 + CONN_STUB
            } else {
                mx
            };
            pts.extend([(p0.0, o0.1), (mx, o0.1), (mx, o1.1), (p1.0, o1.1)]);
        }
    }
    pts.push(p1);
    // Drop repeated points and merge collinear ones.
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01);
    let mut out: Vec<(f32, f32)> = Vec::new();
    for p in pts {
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let cross = (b.0 - a.0) * (p.1 - b.1) - (b.1 - a.1) * (p.0 - b.0);
            if cross.abs() < 0.01 {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if lo > hi {
        (lo + hi) / 2.0
    } else {
        v.clamp(lo, hi)
    }
}

/// Replace each corner of a polyline with a small quadratic curve.
pub fn round_corners(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let (a, b, c) = (pts[i - 1], pts[i], pts[i + 1]);
        let (la, lc) = ((b.0 - a.0).hypot(b.1 - a.1), (c.0 - b.0).hypot(c.1 - b.1));
        let r = CONN_RADIUS.min(la / 2.0).min(lc / 2.0);
        if r < 0.5 {
            out.push(b);
            continue;
        }
        let p = (b.0 + (a.0 - b.0) * r / la, b.1 + (a.1 - b.1) * r / la);
        let q = (b.0 + (c.0 - b.0) * r / lc, b.1 + (c.1 - b.1) * r / lc);
        const N: usize = 6;
        for k in 0..=N {
            let t = k as f32 / N as f32;
            let u = 1.0 - t;
            out.push((
                u * u * p.0 + 2.0 * u * t * b.0 + t * t * q.0,
                u * u * p.1 + 2.0 * u * t * b.1 + t * t * q.1,
            ));
        }
    }
    out.push(pts[pts.len() - 1]);
    out
}

/// Bounding box of all cards and notes.
pub fn bounds(board: &Board, adv: Adv) -> Option<Rect> {
    let rects = board
        .areas
        .iter()
        .map(|a| area_geom(a, adv).card)
        .chain(board.notes.iter().map(|n| note_geom(n, adv).card))
        .chain(board.texts.iter().map(|t| text_geom(t, adv).rect))
        .chain(board.shapes.iter().map(|s| {
            let (x, y, w, h) = s.bbox();
            Rect::new(x, y, w, h)
        }));
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
/// The saved space the camera is showing right now (same centre and zoom), if any.
pub fn active_space(board: &Board, cam: &Camera, vw: f32, vh: f32) -> Option<usize> {
    let (cx, cy) = cam.screen_to_world(vw / 2.0, vh / 2.0);
    board.spaces.iter().position(|s| {
        (s.x - cx).abs() * cam.zoom < 3.0
            && (s.y - cy).abs() * cam.zoom < 3.0
            && (s.zoom - cam.zoom).abs() < 0.01 * s.zoom
    })
}

/// Point the camera at a saved space.
pub fn show_space(cam: &mut Camera, s: &crate::board::Space, vw: f32, vh: f32) {
    cam.zoom = s
        .zoom
        .clamp(crate::camera::MIN_ZOOM, crate::camera::MAX_ZOOM);
    cam.x = s.x - vw / 2.0 / cam.zoom;
    cam.y = s.y - vh / 2.0 / cam.zoom;
}

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

/// Buttons of the right-hand view menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewBtn {
    ZoomIn,
    ZoomOut,
    /// Cycle the canvas background (the same as the `B` key).
    Background,
    /// Open the theme panel (the same as `Shift+L`).
    Theme,
    /// Show the keyboard shortcuts popup (the same as the `?` key).
    Help,
}

impl ViewBtn {
    pub const ALL: [ViewBtn; 5] = [
        ViewBtn::ZoomIn,
        ViewBtn::ZoomOut,
        ViewBtn::Background,
        ViewBtn::Theme,
        ViewBtn::Help,
    ];
}

/// Screen-space (logical px) overlay: toolbar on the left, view menu on the right, area chips
/// on top.
#[derive(Debug, Clone)]
pub struct Overlay {
    pub tools_pill: Rect,
    pub tools: Vec<(Tool, Rect)>,
    /// The view menu at the right edge: zoom in, zoom out, background colour (top to bottom).
    pub view_pill: Rect,
    pub view_btns: Vec<(ViewBtn, Rect)>,
    pub chips_pill: Option<Rect>,
    /// (area index, rect)
    pub chips: Vec<(usize, Rect)>,
    /// The "+" button that adds a space.
    pub plus: Option<Rect>,
    /// The menu is scrolled: spaces are hidden before / after the visible ones.
    pub more_left: bool,
    pub more_right: bool,
    /// The theme panel, left of the view menu (only drawn while open) and its rows.
    pub theme_panel: Rect,
    pub theme_items: Vec<(Theme, Rect)>,
}

/// What the pointer is over in the screen-space overlay (drives hover effects).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Hover {
    #[default]
    None,
    Tool(Tool),
    View(ViewBtn),
    Chip(usize),
    /// The "+" in the space menu.
    AddSpace,
    Theme(Theme),
    /// A button of the text format bar.
    Fmt(FmtBtn),
    /// The drag grip of the focused text.
    Grip,
}

pub const THEME_PANEL_W: f32 = 232.0;
const THEME_HEAD_H: f32 = 40.0;
pub const THEME_ROW_H: f32 = 48.0;

const CHIP_H: f32 = 28.0;
const CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;
const PILL_PAD: f32 = 6.0;
pub const TOOL_SIZE: f32 = 40.0;
const TOOL_GAP: f32 = 4.0;
const TOOLS_LEFT: f32 = 14.0;
/// How far the area chips stay clear of the top edge. A shell bar on the compositor's top layer
/// sits above wallboard and swallows clicks there, so this defaults to clearing a typical bar.
/// Tune it with `"ui": {"top": ..}` in board.json.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    /// Gap above the area chips (logical px).
    #[serde(default = "default_top")]
    pub top: f32,
}

fn default_top() -> f32 {
    64.0
}

impl Default for Margins {
    fn default() -> Self {
        Margins { top: default_top() }
    }
}

impl Margins {
    /// Clamp to a sane value so a bad number in the file cannot push the UI off screen.
    pub fn sane(self) -> Margins {
        let top = if self.top.is_finite() {
            self.top.clamp(0.0, 400.0)
        } else {
            default_top()
        };
        Margins { top }
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
    overlay_from(board, vw, vh, m, tw, 0)
}

/// Like `overlay`, with the space menu scrolled so it starts at space `first`.
pub fn overlay_from(
    board: &Board,
    vw: f32,
    vh: f32,
    m: Margins,
    tw: &dyn Fn(Style, &str) -> f32,
    first: usize,
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

    // View menu: a small vertical pill at the right edge, mirroring the toolbar.
    let n = ViewBtn::ALL.len() as f32;
    let vh_pill = n * TOOL_SIZE + (n - 1.0) * TOOL_GAP + 2.0 * PILL_PAD;
    let vy = ((vh - vh_pill) / 2.0).max(m.top + 50.0);
    let vx = vw - TOOLS_LEFT - (TOOL_SIZE + 2.0 * PILL_PAD);
    let view_pill = Rect::new(vx, vy, TOOL_SIZE + 2.0 * PILL_PAD, vh_pill);
    let view_btns = ViewBtn::ALL
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let y = vy + PILL_PAD + i as f32 * (TOOL_SIZE + TOOL_GAP);
            (*b, Rect::new(vx + PILL_PAD, y, TOOL_SIZE, TOOL_SIZE))
        })
        .collect();

    // Space menu: the titles from `first` on that fit, then a "+" button that is always there.
    let max_w = (vw - 40.0).max(0.0);
    let first = first.min(board.spaces.len().saturating_sub(1));
    let mut widths = Vec::new();
    let mut used = 2.0 * PILL_PAD + CHIP_H + CHIP_GAP;
    for sp in board.spaces.iter().skip(first) {
        let w = tw(Style::ChipBold, &sp.name) + 2.0 * CHIP_PAD;
        let next = used + w + if widths.is_empty() { 0.0 } else { CHIP_GAP };
        if next > max_w && !widths.is_empty() {
            break;
        }
        used = next;
        widths.push(w);
    }
    let shown = widths.len();
    widths.push(CHIP_H); // the "+" button
    let (chips_pill, mut chips) = match row_of(&widths, vw, m.top, CHIP_H, CHIP_GAP) {
        Some((p, r)) => (Some(p), r.into_iter().enumerate().collect::<Vec<_>>()),
        None => (None, Vec::new()),
    };
    let plus = chips.pop().map(|(_, r)| r);
    for c in &mut chips {
        c.0 += first;
    }
    let more_left = first > 0;
    let more_right = first + shown < board.spaces.len();

    // Theme panel: a card to the left of the view menu, kept on screen.
    let n = Theme::ALL.len() as f32;
    let ph = THEME_HEAD_H + n * THEME_ROW_H + 8.0;
    let px = (view_pill.x - 12.0 - THEME_PANEL_W).max(8.0);
    let py = view_pill.y.min(vh - ph - 10.0).max(8.0);
    let theme_panel = Rect::new(px, py, THEME_PANEL_W, ph);
    let theme_items = Theme::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let y = py + THEME_HEAD_H + i as f32 * THEME_ROW_H;
            (
                *t,
                Rect::new(px + 8.0, y, THEME_PANEL_W - 16.0, THEME_ROW_H - 4.0),
            )
        })
        .collect();

    Overlay {
        theme_panel,
        theme_items,
        tools_pill,
        tools,
        view_pill,
        view_btns,
        chips_pill,
        chips,
        plus,
        more_left,
        more_right,
    }
}

impl Overlay {
    pub fn view_at(&self, x: f32, y: f32) -> Option<ViewBtn> {
        self.view_btns
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(b, _)| *b)
    }

    /// The overlay item under the point. The theme rows only count while the panel is open.
    pub fn hover_at(&self, x: f32, y: f32, theme_open: bool) -> Hover {
        if theme_open {
            if let Some(t) = self.theme_at(x, y) {
                return Hover::Theme(t);
            }
        }
        if let Some(b) = self.view_at(x, y) {
            Hover::View(b)
        } else if let Some(t) = self.tool_at(x, y) {
            Hover::Tool(t)
        } else if let Some(i) = self.chip_at(x, y) {
            Hover::Chip(i)
        } else if self.plus_at(x, y) {
            Hover::AddSpace
        } else {
            Hover::None
        }
    }

    pub fn theme_at(&self, x: f32, y: f32) -> Option<Theme> {
        self.theme_items
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(t, _)| *t)
    }

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
    pub fn plus_at(&self, x: f32, y: f32) -> bool {
        self.plus.is_some_and(|r| r.contains(x, y))
    }
    /// True if the point is over any pill (clicks there must not reach the canvas).
    pub fn covers(&self, x: f32, y: f32) -> bool {
        self.tools_pill.contains(x, y)
            || self.view_pill.contains(x, y)
            || self.chips_pill.is_some_and(|r| r.contains(x, y))
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
        assert_eq!(g.card.w, crate::board::NOTE_W);
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

    /// A board with `n` saved spaces.
    fn spaced(n: usize) -> Board {
        let mut b = Board::default_board();
        for i in 0..n {
            b.add_space(&format!("Space number {i}"), i as f32 * 500.0, 0.0, 1.0);
        }
        b
    }

    #[test]
    fn overlay_toolbar_and_chips() {
        let b = spaced(7);
        let o = overlay(&b, 1920.0, 1080.0, Margins::default(), &tw);
        assert_eq!(o.tools.len(), 13);
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
        let p = o.chips_pill.unwrap();
        assert!(((p.x + p.w / 2.0) - 960.0).abs() < 1e-3, "pill is centred");
        let (i, r) = o.chips[3];
        assert_eq!(o.chip_at(r.x + 2.0, r.y + 2.0), Some(i));
        assert!(o.covers(p.x + 1.0, p.y + 1.0));
        assert!(o.covers(o.tools_pill.x + 1.0, o.tools_pill.y + 1.0));
        assert!(!o.covers(960.0, 500.0));
    }

    #[test]
    fn a_space_is_active_only_when_the_camera_shows_it() {
        let mut b = Board::new();
        b.add_space("A", 100.0, 200.0, 1.5);
        let mut cam = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        };
        assert_eq!(active_space(&b, &cam, 1920.0, 1080.0), None);
        show_space(&mut cam, &b.spaces[0], 1920.0, 1080.0);
        assert_eq!(active_space(&b, &cam, 1920.0, 1080.0), Some(0));
        let c = cam.screen_to_world(960.0, 540.0);
        assert!((c.0 - 100.0).abs() < 1e-3 && (c.1 - 200.0).abs() < 1e-3);
        cam.pan_by_screen(40.0, 0.0);
        assert_eq!(
            active_space(&b, &cam, 1920.0, 1080.0),
            None,
            "panning away deselects it"
        );
    }

    #[test]
    fn space_menu_has_a_plus_even_with_no_spaces() {
        let o = overlay(&Board::new(), 1920.0, 1080.0, Margins::default(), &tw);
        let plus = o.plus.expect("plus button");
        assert!(o.chips.is_empty() && o.chips_pill.is_some());
        assert!(o.plus_at(plus.x + 1.0, plus.y + 1.0) && o.covers(plus.x + 1.0, plus.y + 1.0));
        assert_eq!(
            o.hover_at(plus.x + 1.0, plus.y + 1.0, false),
            Hover::AddSpace
        );
    }

    #[test]
    fn space_menu_scrolls_and_keeps_the_plus_on_screen() {
        let b = spaced(7);
        let o0 = overlay_from(&b, 300.0, 600.0, Margins::default(), &tw, 0);
        assert!(o0.more_right && !o0.more_left);
        let first_hidden = o0.chips.last().unwrap().0 + 1;
        let o1 = overlay_from(&b, 300.0, 600.0, Margins::default(), &tw, first_hidden);
        assert!(o1.more_left);
        assert_eq!(o1.chips[0].0, first_hidden, "indices are real area indices");
        for o in [&o0, &o1] {
            let p = o.chips_pill.unwrap();
            let plus = o.plus.unwrap();
            assert!(p.x >= 0.0 && p.right() <= 300.0);
            assert!(plus.x >= p.x && plus.right() <= p.right());
            let last = o.chips.last().unwrap().1;
            assert!(last.right() <= plus.x, "plus sits after the last title");
        }
        // scrolled past the end still shows the last space
        let o = overlay_from(&b, 1920.0, 1080.0, Margins::default(), &tw, 99);
        assert_eq!(o.chips.last().unwrap().0, b.spaces.len() - 1);
        assert!(!o.more_right);
    }

    #[test]
    fn overlay_drops_chips_that_do_not_fit_and_survives_small_screens() {
        let b = spaced(7);
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
        let o2 = overlay(&b, 1920.0, 1080.0, Margins { top: 100.0 }, &tw);
        assert_eq!(o2.chips_pill.unwrap().y, 100.0);
        // absurd values are clamped rather than pushing the UI off screen
        let o3 = overlay(&b, 1920.0, 1080.0, Margins { top: f32::NAN }, &tw);
        assert_eq!(o3.chips_pill.unwrap().y, 64.0);
        let m: Margins = serde_json::from_str("{}").unwrap();
        assert_eq!(m, Margins::default());
    }

    #[test]
    fn view_menu_sits_on_the_right_and_is_hit_testable() {
        let b = Board::default_board();
        let o = overlay(&b, 1920.0, 1080.0, Margins::default(), &tw);
        let kinds: Vec<ViewBtn> = o.view_btns.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            [
                ViewBtn::ZoomIn,
                ViewBtn::ZoomOut,
                ViewBtn::Background,
                ViewBtn::Theme,
                ViewBtn::Help
            ]
        );
        for w in o.view_btns.windows(2) {
            assert!(
                w[0].1.bottom() <= w[1].1.y,
                "buttons stack top to bottom without overlap"
            );
        }
        let p = o.view_pill;
        assert!(
            p.right() <= 1920.0 && 1920.0 - p.right() < 30.0,
            "hugs the right edge"
        );
        assert!((p.y + p.h / 2.0 - 540.0).abs() < 1.0, "vertically centred");
        assert!(p.x > o.tools_pill.right() + 100.0, "far from the toolbar");
        for (btn, r) in &o.view_btns {
            assert_eq!(o.view_at(r.x + 3.0, r.y + 3.0), Some(*btn));
            assert!(
                r.right() <= p.right() && r.bottom() <= p.bottom(),
                "inside the pill"
            );
        }
        assert_eq!(o.view_at(960.0, 540.0), None);
        assert!(
            o.covers(p.x + 1.0, p.y + 1.0),
            "clicks on the pill never reach the canvas"
        );
        // tiny screens must not panic
        let _ = overlay(&b, 200.0, 300.0, Margins::default(), &tw);
    }

    #[test]
    fn free_text_width_follows_its_longest_line() {
        let mut b = Board::new();
        let id = b.add_text(10.0, 20.0);
        let g0 = text_geom(b.text(id).unwrap(), &adv);
        assert!(
            g0.rect.w >= 60.0 && g0.rect.h == TEXT_LINE_H,
            "empty text still has a click target"
        );
        b.text_mut(id).unwrap().text = "short\na much longer second line".into();
        let g = text_geom(b.text(id).unwrap(), &adv);
        assert_eq!(g.lines.len(), 2);
        assert_eq!(g.rect.h, 2.0 * TEXT_LINE_H);
        assert_eq!(
            g.rect.w,
            "a much longer second line".chars().count() as f32 * 7.0
        );
        assert_eq!((g.rect.x, g.rect.y), (10.0, 20.0));
    }

    #[test]
    fn shape_hit_testing_per_kind() {
        let mk = |kind, x2, y2| Shape {
            id: 1,
            kind,
            x1: 0.0,
            y1: 0.0,
            x2,
            y2,
            color: 0,
            group: 0,
            src: String::new(),
        };
        let rect = mk(ShapeKind::Rect, 100.0, 50.0);
        assert!(shape_hit(&rect, 50.0, 25.0) && !shape_hit(&rect, 150.0, 25.0));
        let oval = mk(ShapeKind::Ellipse, 100.0, 100.0);
        assert!(shape_hit(&oval, 50.0, 50.0));
        assert!(
            !shape_hit(&oval, 3.0, 3.0),
            "the corner of the bounding box is outside an ellipse"
        );
        let line = mk(ShapeKind::Line, 100.0, 100.0);
        assert!(shape_hit(&line, 50.0, 53.0) && !shape_hit(&line, 50.0, 80.0));
        assert!(shape_hit(&line, 100.0, 100.0), "end point");
        let dot = mk(ShapeKind::Line, 0.0, 0.0);
        assert!(
            shape_hit(&dot, 2.0, 2.0),
            "a zero-length line does not divide by zero"
        );
    }

    #[test]
    fn texts_sit_above_shapes_and_below_cards_in_hit_order() {
        let mut b = Board::default_board();
        let s = b.add_shape(ShapeKind::Rect, 5000.0, 5000.0);
        b.shape_mut(s).unwrap().x2 = 5200.0;
        b.shape_mut(s).unwrap().y2 = 5200.0;
        assert_eq!(hit_test(&b, 5100.0, 5100.0, A), Some(Target::Shape(0)));
        let t = b.add_text(5050.0, 5050.0);
        b.text_mut(t).unwrap().text = "label".into();
        assert_eq!(
            hit_test(&b, 5060.0, 5060.0, A),
            Some(Target::FreeText(0)),
            "text over the shape"
        );
        assert_eq!(hit_test(&b, 5190.0, 5190.0, A), Some(Target::Shape(0)));
        let card = area_geom(&b.areas[2], A).card;
        let s2 = b.add_shape(ShapeKind::Rect, card.x - 20.0, card.y - 20.0);
        b.shape_mut(s2).unwrap().x2 = card.right() + 20.0;
        b.shape_mut(s2).unwrap().y2 = card.bottom() + 20.0;
        assert!(
            matches!(
                hit_test(&b, card.x + 5.0, card.y + 5.0, A),
                Some(Target::Title(2))
            ),
            "a card beats a shape under it"
        );
    }

    #[test]
    fn handles_for_each_kind_and_grab_tolerance() {
        let mut b = Board::default_board();
        let a = Sel::Area(b.areas[2].id);
        let hs = handles(&b, a, A);
        let card = area_geom(&b.areas[2], A).card;
        assert_eq!(hs.len(), 4);
        assert!(hs.contains(&(Handle::BottomRight, (card.right(), card.bottom()))));
        assert_eq!(
            hit_handle(&b, a, card.right() + 4.0, card.bottom() - 4.0, 8.0, A),
            Some(Handle::BottomRight)
        );
        assert_eq!(
            hit_handle(&b, a, card.right() + 30.0, card.bottom(), 8.0, A),
            None,
            "outside the tolerance"
        );
        let s = b.add_shape(ShapeKind::Line, 0.0, 0.0);
        b.shape_mut(s).unwrap().x2 = 50.0;
        assert_eq!(
            handles(&b, Sel::Shape(s), A).len(),
            2,
            "a line has its two ends"
        );
        let t = b.add_text(0.0, 0.0);
        assert!(handles(&b, Sel::Text(t), A).is_empty());
        assert!(
            handles(&b, Sel::Note(999), A).is_empty(),
            "a missing item has none"
        );
        assert!(
            Handle::TopLeft.is_left()
                && Handle::BottomLeft.is_left()
                && !Handle::TopRight.is_left()
        );
    }

    #[test]
    fn resized_widths_change_the_card_and_note_geometry() {
        let mut b = Board::default_board();
        let w0 = area_geom(&b.areas[2], A).card.w;
        b.areas[2].w = 500.0;
        assert_eq!(area_geom(&b.areas[2], A).card.w, 500.0);
        b.areas[2].w = 10.0;
        assert_eq!(
            area_geom(&b.areas[2], A).card.w,
            w0,
            "a nonsense width falls back to the default"
        );
        let n = b.add_note(0.0, 0.0);
        b.note_mut(n).unwrap().w = 400.0;
        assert_eq!(note_geom(b.note(n).unwrap(), A).card.w, 400.0);
        b.note_mut(n).unwrap().body = "word ".repeat(40);
        let tall = note_geom(b.note(n).unwrap(), A).card.h;
        b.note_mut(n).unwrap().w = 800.0;
        assert!(
            note_geom(b.note(n).unwrap(), A).card.h < tall,
            "a wider note wraps to fewer lines"
        );
    }

    #[test]
    fn bounds_include_texts_and_shapes() {
        let mut b = Board::new();
        let s = b.add_shape(ShapeKind::Rect, 100.0, 100.0);
        b.shape_mut(s).unwrap().x2 = 900.0;
        b.shape_mut(s).unwrap().y2 = 700.0;
        let r = bounds(&b, A).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 100.0, 800.0, 600.0));
        b.add_text(2000.0, -50.0);
        assert!(bounds(&b, A).unwrap().right() >= 2064.0);
    }

    #[test]
    fn connector_routes_are_orthogonal_and_leave_the_facing_sides() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(300.0, 200.0, 100.0, 100.0);
        let p = route(Some(a), (0.0, 0.0), Some(b), (0.0, 0.0));
        assert_eq!(p[0].0, 100.0, "leaves the right side of A: {p:?}");
        assert_eq!(p[p.len() - 1].0, 300.0, "enters the left side of B: {p:?}");
        assert!(p.windows(2).all(|w| w[0].0 == w[1].0 || w[0].1 == w[1].1));
        // overlapping on the cross axis: one straight segment
        let c = Rect::new(300.0, 20.0, 100.0, 60.0);
        assert_eq!(route(Some(a), (0.0, 0.0), Some(c), (0.0, 0.0)).len(), 2);
        // stacked: vertical run, bottom of A to top of B
        let d = Rect::new(10.0, 300.0, 100.0, 50.0);
        let p = route(Some(a), (0.0, 0.0), Some(d), (0.0, 0.0));
        assert_eq!((p[0].1, p[p.len() - 1].1), (100.0, 300.0));
        // free point to free point
        let p = route(None, (0.0, 0.0), None, (200.0, 100.0));
        assert_eq!((p[0], p[p.len() - 1]), ((0.0, 0.0), (200.0, 100.0)));
        // rounded version still starts and ends on the endpoints
        let r = round_corners(&p);
        assert_eq!((r[0], r[r.len() - 1]), ((0.0, 0.0), (200.0, 100.0)));
    }
}

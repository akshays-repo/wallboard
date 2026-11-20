//! Board data model: areas (groups of todos), notes, strokes, undo history. Pure Rust, no Wayland.
//!
//! Every field added after the first release has a serde default, so older save files load.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Number of area colours (see the palettes in `render`).
pub const PALETTE_LEN: u8 = 10;
/// The hero area ("Focus") may hold at most this many open todos, so you must prioritise.
pub const MAX_HERO_OPEN: usize = 3;
pub const MAX_PRIORITY: u8 = 3;
pub const CARD_W: f32 = 320.0;
/// Narrowest and widest a card or note can be resized to.
pub const MIN_AREA_W: f32 = 240.0;
pub const MIN_NOTE_W: f32 = 160.0;
pub const MAX_W: f32 = 900.0;
pub const HERO_W: f32 = 400.0;
const GAP: f32 = 40.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    pub text: String,
    #[serde(default)]
    pub done: bool,
    /// 0 (none) to 3, shown as orange "!", "!!", "!!!".
    #[serde(default)]
    pub priority: u8,
    /// Optional multi-line details under the title. `None` = collapsed, `Some` = shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl Todo {
    pub fn new(text: &str) -> Self {
        Todo {
            text: text.to_string(),
            done: false,
            priority: 0,
            body: None,
        }
    }
}

/// A free-form sticky note on the canvas: a title and a multi-line body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub color: u8,
    /// Note width. 0 (or too small) means `NOTE_W`. Height follows the text.
    #[serde(default)]
    pub w: f32,
    /// Lock-together group id (0 = ungrouped). Moving one member moves them all.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: u64,
}

impl Note {
    pub fn width(&self) -> f32 {
        if self.w >= MIN_NOTE_W {
            self.w.min(MAX_W)
        } else {
            NOTE_W
        }
    }
}

fn is_zero(g: &u64) -> bool {
    *g == 0
}

pub const NOTE_W: f32 = 240.0;

/// Free text on the canvas: no card, just text you can type anywhere.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBox {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub color: u8,
    /// Lock-together group id (0 = ungrouped). Moving one member moves them all.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    Rect,
    Ellipse,
    Line,
    /// A PNG kept in the data folder's `images/` (`Shape::src` is its file name).
    Image,
    /// An emoji sticker (`Shape::src` is the emoji).
    Sticker,
}

/// A rectangle, ellipse or line between two world points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    pub kind: ShapeKind,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    #[serde(default)]
    pub color: u8,
    /// Lock-together group id (0 = ungrouped). Moving one member moves them all.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: u64,
    /// Image file name or sticker emoji (empty for plain shapes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub src: String,
}

/// The stickers `S` cycles through.
pub const STICKERS: [&str; 16] = [
    "⭐", "❤", "🔥", "💡", "✅", "🚀", "🎯", "📌", "⚡", "🎉", "📅", "💬", "🏆", "🌱", "☕", "🎵",
];

impl Shape {
    /// Normalised bounding box (x, y, w, h).
    pub fn bbox(&self) -> (f32, f32, f32, f32) {
        let (x, y) = (self.x1.min(self.x2), self.y1.min(self.y2));
        (x, y, (self.x1 - self.x2).abs(), (self.y1 - self.y2).abs())
    }
}

/// One end of a connector: attached to an item (`id` != 0, follows it) or a free point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct End {
    /// Id of the card, note, text or shape it is attached to; 0 = free point.
    #[serde(default)]
    pub id: u64,
    /// The free point (also the last known position while attached).
    pub x: f32,
    pub y: f32,
}

impl End {
    pub fn free(x: f32, y: f32) -> Self {
        End { id: 0, x, y }
    }
}

/// An arrow from one point or item to another. Attached ends follow their item when it moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub id: u64,
    pub from: End,
    pub to: End,
    #[serde(default)]
    pub color: u8,
}

/// Size given to a shape that was clicked rather than dragged.
pub const DEFAULT_SHAPE: (f32, f32) = (160.0, 100.0);

/// A freehand pen stroke in world coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub id: u64,
    pub pts: Vec<(f32, f32)>,
    #[serde(default)]
    pub color: u8,
    /// Line width in world units.
    #[serde(default = "default_stroke_width")]
    pub width: f32,
}

fn default_stroke_width() -> f32 {
    3.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub id: u64,
    pub name: String,
    /// Index into the colour palette (0..PALETTE_LEN).
    #[serde(default)]
    pub color: u8,
    pub x: f32,
    pub y: f32,
    /// Card width. 0 (or too small) means the default for its kind. Height follows the content.
    #[serde(default)]
    pub w: f32,
    /// The hero card: wider, accent border, limited to `MAX_HERO_OPEN` open todos.
    #[serde(default)]
    pub hero: bool,
    #[serde(default)]
    pub todos: Vec<Todo>,
    /// Lock-together group id (0 = ungrouped). Moving one member moves them all.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: u64,
}

impl Area {
    pub fn default_width(&self) -> f32 {
        if self.hero {
            HERO_W
        } else {
            CARD_W
        }
    }

    /// The card's width: its own if it was resized, else the default.
    pub fn width(&self) -> f32 {
        if self.w >= MIN_AREA_W {
            self.w.min(MAX_W)
        } else {
            self.default_width()
        }
    }

    pub fn open_count(&self) -> usize {
        self.todos.iter().filter(|t| !t.done).count()
    }

    /// True if no more open todos may be added.
    pub fn is_full(&self) -> bool {
        self.hero && self.open_count() >= MAX_HERO_OPEN
    }

    /// Insert an empty open todo at `at` (clamped). `None` if the hero area is full.
    pub fn insert_todo(&mut self, at: usize) -> Option<usize> {
        if self.is_full() {
            return None;
        }
        let at = at.min(self.todos.len());
        self.todos.insert(at, Todo::new(""));
        Some(at)
    }

    /// Toggle done. Re-opening a todo in a full hero area is refused; returns whether it changed.
    pub fn toggle_done(&mut self, i: usize) -> bool {
        let Some(t) = self.todos.get(i) else {
            return false;
        };
        if t.done && self.is_full() {
            return false;
        }
        self.todos[i].done = !self.todos[i].done;
        true
    }

    /// Whether `toggle_done(i)` would succeed (re-opening in a full hero area is refused).
    pub fn can_toggle(&self, i: usize) -> bool {
        self.todos
            .get(i)
            .is_some_and(|t| !(t.done && self.is_full()))
    }

    pub fn cycle_priority(&mut self, i: usize) {
        if let Some(t) = self.todos.get_mut(i) {
            t.priority = (t.priority + 1) % (MAX_PRIORITY + 1);
        }
    }

    pub fn cycle_color(&mut self) {
        self.color = (self.color + 1) % PALETTE_LEN;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    pub fn toggled(self) -> Theme {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

/// Canvas background: the theme colour by default, a solid colour, or a wallpaper image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Background {
    /// Solid colour as "#RRGGBB". `None` uses the theme canvas colour.
    #[serde(default)]
    pub color: Option<String>,
    /// Wallpaper image path (PNG). `None` falls back to `wallpaper.png` in the data directory.
    #[serde(default)]
    pub image: Option<String>,
    /// Show the wallpaper instead of a colour.
    #[serde(default)]
    pub image_on: bool,
    /// 0..1: how much the wallpaper is darkened so text stays readable.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Change the wallpaper every this many minutes (0 = never). Needs wallpapers in the
    /// `wallpapers/` folder of the data directory.
    #[serde(default)]
    pub rotate_mins: u32,
    /// Rotation picks a random wallpaper instead of the next one in name order.
    #[serde(default)]
    pub random: bool,
    /// Folder holding the wallpapers (PNG). `None` uses `wallpapers/` in the data directory.
    #[serde(default)]
    pub folder: Option<String>,
}

/// Minutes between wallpaper changes when rotation is switched on with the `W` key.
pub const DEFAULT_ROTATE_MINS: u32 = 30;

fn default_dim() -> f32 {
    0.35
}

impl Default for Background {
    fn default() -> Self {
        Background {
            color: None,
            image: None,
            image_on: false,
            dim: default_dim(),
            rotate_mins: 0,
            random: false,
            folder: None,
        }
    }
}

/// Colours offered by the `B` key, after the theme default.
pub const BACKGROUND_PRESETS: [&str; 6] = [
    "#0B0F19", "#14211A", "#241A2E", "#2B2118", "#E8E2D4", "#DCE6F2",
];

/// Parse "#RRGGBB" (the `#` is optional).
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl Background {
    pub fn solid(&self) -> Option<(u8, u8, u8)> {
        if self.image_on {
            return None;
        }
        self.color.as_deref().and_then(parse_hex)
    }

    /// Next background for the `B` key: theme default -> each preset -> each wallpaper in
    /// `images` (in order) -> theme default.
    pub fn cycle(&mut self, images: &[String]) {
        if self.image_on {
            let pos = self
                .image
                .as_ref()
                .and_then(|p| images.iter().position(|i| i == p));
            if let Some(i) = pos.filter(|i| i + 1 < images.len()) {
                self.image = Some(images[i + 1].clone());
            } else {
                self.image_on = false;
                self.color = None;
            }
            return;
        }
        let idx = self.color.as_deref().and_then(|c| {
            BACKGROUND_PRESETS
                .iter()
                .position(|p| p.eq_ignore_ascii_case(c))
        });
        match (self.color.is_some(), idx) {
            (false, _) => self.color = Some(BACKGROUND_PRESETS[0].to_string()),
            (true, Some(i)) if i + 1 < BACKGROUND_PRESETS.len() => {
                self.color = Some(BACKGROUND_PRESETS[i + 1].to_string());
            }
            _ => {
                self.color = None;
                if let Some(first) = images.first() {
                    self.image_on = true;
                    self.image = Some(first.clone());
                }
            }
        }
    }

    /// Rotation step: switch to the next wallpaper in `images`, or a random other one when
    /// `random` is set (`rnd` is any random number). Returns false if there is nothing to
    /// switch to.
    pub fn advance(&mut self, images: &[String], rnd: u64) -> bool {
        if images.len() < 2 {
            return false;
        }
        let cur = self
            .image
            .as_ref()
            .and_then(|p| images.iter().position(|i| i == p));
        let next = match (self.random, cur) {
            // A random pick that is never the current one.
            (true, Some(c)) => (c + 1 + (rnd % (images.len() as u64 - 1)) as usize) % images.len(),
            (true, None) => (rnd % images.len() as u64) as usize,
            (false, Some(c)) => (c + 1) % images.len(),
            (false, None) => 0,
        };
        self.image = Some(images[next].clone());
        true
    }

    /// The `W` key: off -> in order -> random -> off.
    pub fn cycle_rotation(&mut self) {
        if self.rotate_mins == 0 {
            self.rotate_mins = DEFAULT_ROTATE_MINS;
            self.random = false;
        } else if !self.random {
            self.random = true;
        } else {
            self.rotate_mins = 0;
            self.random = false;
        }
    }
}

#[derive(Debug, Default)]
pub struct Board {
    /// Left-to-right order; keys 1-9 jump to the area at that position.
    pub areas: Vec<Area>,
    pub notes: Vec<Note>,
    pub strokes: Vec<Stroke>,
    pub texts: Vec<TextBox>,
    pub shapes: Vec<Shape>,
    pub connectors: Vec<Connector>,
    next_id: u64,
}

impl Board {
    pub fn new() -> Self {
        Board {
            areas: Vec::new(),
            notes: Vec::new(),
            strokes: Vec::new(),
            texts: Vec::new(),
            shapes: Vec::new(),
            connectors: Vec::new(),
            next_id: 1,
        }
    }

    /// Attach loaded connectors, repairing bad values and ids.
    pub fn with_connectors(mut self, mut cs: Vec<Connector>) -> Self {
        let mut seen = std::collections::HashSet::new();
        for c in &mut cs {
            c.color %= PALETTE_LEN;
            for e in [&mut c.from, &mut c.to] {
                if !e.x.is_finite() || !e.y.is_finite() {
                    (e.x, e.y) = (0.0, 0.0);
                }
            }
            if c.id == 0 || !seen.insert(c.id) {
                c.id = self.take_id();
                seen.insert(c.id);
            }
            self.next_id = self.next_id.max(c.id.saturating_add(1));
        }
        self.connectors = cs;
        self
    }

    /// True if `id` is a card, note, text or shape on the board.
    pub fn has_item(&self, id: u64) -> bool {
        self.areas.iter().any(|a| a.id == id)
            || self.notes.iter().any(|n| n.id == id)
            || self.texts.iter().any(|t| t.id == id)
            || self.shapes.iter().any(|s| s.id == id)
    }

    /// Detach connector ends whose item was deleted (they stay as free points).
    /// A connector with both ends on the same missing item is dropped.
    pub fn prune_connectors(&mut self) {
        let mut cs = std::mem::take(&mut self.connectors);
        for c in &mut cs {
            for e in [&mut c.from, &mut c.to] {
                if e.id != 0 && !self.has_item(e.id) {
                    e.id = 0;
                }
            }
        }
        self.connectors = cs;
    }

    pub fn add_connector(&mut self, from: End, to: End, color: u8) -> u64 {
        let id = self.take_id();
        self.connectors.push(Connector {
            id,
            from,
            to,
            color,
        });
        id
    }

    pub fn connector_mut(&mut self, id: u64) -> Option<&mut Connector> {
        self.connectors.iter_mut().find(|c| c.id == id)
    }

    /// The life-OS layout: Inbox, Focus (hero), Work, Startups, Health, Learning, Life.
    pub fn default_board() -> Self {
        let mut b = Board::new();
        // (name, colour index: blue 0, green 1, orange 2, red 3, yellow 4, teal 5, purple 6, pink 7, indigo 8, gray 9)
        let layout: [(&str, u8, bool); 7] = [
            ("Inbox", 9, false),
            ("Focus", 0, true),
            ("Work", 8, false),
            ("Startups", 6, false),
            ("Health", 1, false),
            ("Learning", 5, false),
            ("Life", 7, false),
        ];
        let mut x = 0.0;
        for (name, color, hero) in layout {
            let id = b.add_area(x, 0.0);
            if let Some(a) = b.area_mut(id) {
                a.name = name.to_string();
                a.color = color;
                a.hero = hero;
                x += a.width() + GAP;
            }
        }
        b
    }

    /// Build a board from loaded data, repairing anything that would break invariants:
    /// colours and priorities are clamped, duplicate ids renumbered, `next_id` kept above all ids.
    pub fn from_parts(
        mut areas: Vec<Area>,
        mut notes: Vec<Note>,
        mut strokes: Vec<Stroke>,
        mut texts: Vec<TextBox>,
        mut shapes: Vec<Shape>,
        next_id: u64,
    ) -> Self {
        let mut next = next_id.max(1);
        for id in areas
            .iter()
            .map(|a| a.id)
            .chain(notes.iter().map(|n| n.id))
            .chain(strokes.iter().map(|s| s.id))
            .chain(texts.iter().map(|t| t.id))
            .chain(shapes.iter().map(|s| s.id))
        {
            next = next.max(id.saturating_add(1));
        }
        for n in &mut notes {
            n.color %= PALETTE_LEN;
        }
        for st in &mut strokes {
            st.color %= PALETTE_LEN;
            st.width = if st.width.is_finite() {
                st.width.clamp(0.5, 40.0)
            } else {
                3.0
            };
            st.pts.retain(|p| p.0.is_finite() && p.1.is_finite());
        }
        strokes.retain(|s| !s.pts.is_empty());
        let mut seen_n = std::collections::HashSet::new();
        for n in &mut notes {
            if !seen_n.insert(n.id) {
                n.id = next;
                next += 1;
            }
        }
        let mut seen_s = std::collections::HashSet::new();
        for s in &mut strokes {
            if !seen_s.insert(s.id) {
                s.id = next;
                next += 1;
            }
        }
        let mut seen = std::collections::HashSet::new();
        for a in &mut areas {
            if !seen.insert(a.id) {
                a.id = next;
                seen.insert(next);
                next += 1;
            }
            a.color %= PALETTE_LEN;
            for t in &mut a.todos {
                t.priority = t.priority.min(MAX_PRIORITY);
            }
        }
        for t in &mut texts {
            t.color %= PALETTE_LEN;
            if !t.x.is_finite() || !t.y.is_finite() {
                (t.x, t.y) = (0.0, 0.0);
            }
        }
        for s in &mut shapes {
            if s.kind == ShapeKind::Image
                && !s
                    .src
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
            {
                s.src.clear(); // never let a saved file name point outside the images folder
            }
            s.color %= PALETTE_LEN;
            for v in [&mut s.x1, &mut s.y1, &mut s.x2, &mut s.y2] {
                if !v.is_finite() {
                    *v = 0.0;
                }
            }
        }
        let mut seen_t = std::collections::HashSet::new();
        for t in &mut texts {
            if !seen_t.insert(t.id) {
                t.id = next;
                next += 1;
            }
        }
        let mut seen_h = std::collections::HashSet::new();
        for s in &mut shapes {
            if !seen_h.insert(s.id) {
                s.id = next;
                next += 1;
            }
        }
        Board {
            areas,
            notes,
            strokes,
            texts,
            shapes,
            connectors: Vec::new(),
            next_id: next,
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    // ---- lock-together groups

    pub fn group_of(&self, sel: crate::layout::Sel) -> u64 {
        use crate::layout::Sel;
        match sel {
            Sel::Area(id) => self.area(id).map_or(0, |a| a.group),
            Sel::Note(id) => self.note(id).map_or(0, |n| n.group),
            Sel::Text(id) => self.text(id).map_or(0, |t| t.group),
            Sel::Shape(id) => self.shape(id).map_or(0, |s| s.group),
            Sel::Connector(_) => 0,
        }
    }

    fn set_group_of(&mut self, sel: crate::layout::Sel, g: u64) {
        use crate::layout::Sel;
        match sel {
            Sel::Area(id) => self.area_mut(id).map(|a| a.group = g),
            Sel::Note(id) => self.note_mut(id).map(|n| n.group = g),
            Sel::Text(id) => self.text_mut(id).map(|t| t.group = g),
            Sel::Shape(id) => self.shape_mut(id).map(|s| s.group = g),
            Sel::Connector(_) => None,
        };
    }

    /// Every item in group `g` (empty for 0).
    pub fn group_members(&self, g: u64) -> Vec<crate::layout::Sel> {
        use crate::layout::Sel;
        if g == 0 {
            return Vec::new();
        }
        let mut v = Vec::new();
        v.extend(
            self.areas
                .iter()
                .filter(|a| a.group == g)
                .map(|a| Sel::Area(a.id)),
        );
        v.extend(
            self.notes
                .iter()
                .filter(|n| n.group == g)
                .map(|n| Sel::Note(n.id)),
        );
        v.extend(
            self.texts
                .iter()
                .filter(|t| t.group == g)
                .map(|t| Sel::Text(t.id)),
        );
        v.extend(
            self.shapes
                .iter()
                .filter(|s| s.group == g)
                .map(|s| Sel::Shape(s.id)),
        );
        v
    }

    /// Lock the given items (and anything already grouped with them) into one group.
    /// Needs at least two items; returns the new group id.
    pub fn group_items(&mut self, items: &[crate::layout::Sel]) -> Option<u64> {
        let mut all = Vec::new();
        for &s in items {
            let g = self.group_of(s);
            let part = if g == 0 {
                vec![s]
            } else {
                self.group_members(g)
            };
            for m in part {
                if !all.contains(&m) {
                    all.push(m);
                }
            }
        }
        if all.len() < 2 {
            return None;
        }
        let g = self.take_id();
        for m in all {
            self.set_group_of(m, g);
        }
        Some(g)
    }

    /// Release every member of the group `sel` belongs to. True if it was grouped.
    pub fn ungroup(&mut self, sel: crate::layout::Sel) -> bool {
        let g = self.group_of(sel);
        let members = self.group_members(g);
        for m in &members {
            self.set_group_of(*m, 0);
        }
        !members.is_empty()
    }

    /// Shift one item by (dx, dy) world units.
    pub fn translate(&mut self, sel: crate::layout::Sel, dx: f32, dy: f32) {
        use crate::layout::Sel;
        match sel {
            Sel::Area(id) => {
                if let Some(a) = self.area_mut(id) {
                    a.x += dx;
                    a.y += dy;
                }
            }
            Sel::Note(id) => {
                if let Some(n) = self.note_mut(id) {
                    n.x += dx;
                    n.y += dy;
                }
            }
            Sel::Text(id) => {
                if let Some(t) = self.text_mut(id) {
                    t.x += dx;
                    t.y += dy;
                }
            }
            Sel::Connector(id) => {
                if let Some(c) = self.connector_mut(id) {
                    for e in [&mut c.from, &mut c.to] {
                        if e.id == 0 {
                            e.x += dx;
                            e.y += dy;
                        }
                    }
                }
            }
            Sel::Shape(id) => {
                if let Some(s) = self.shape_mut(id) {
                    s.x1 += dx;
                    s.x2 += dx;
                    s.y1 += dy;
                    s.y2 += dy;
                }
            }
        }
    }

    pub fn add_area(&mut self, x: f32, y: f32) -> u64 {
        let id = self.take_id();
        let color = (self.areas.len() as u8) % PALETTE_LEN;
        self.areas.push(Area {
            id,
            name: "New group".to_string(),
            color,
            x,
            y,
            w: 0.0,
            hero: false,
            todos: Vec::new(),
            group: 0,
        });
        id
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_note(&mut self, x: f32, y: f32) -> u64 {
        let id = self.take_id();
        let color = (id % 5) as u8; // the first five palette colours
        self.notes.push(Note {
            id,
            x,
            y,
            title: String::new(),
            body: String::new(),
            color,
            w: 0.0,
            group: 0,
        });
        id
    }

    pub fn note(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn note_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    pub fn remove_note(&mut self, id: u64) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        self.notes.len() != before
    }

    pub fn add_text(&mut self, x: f32, y: f32) -> u64 {
        let id = self.take_id();
        self.texts.push(TextBox {
            id,
            x,
            y,
            text: String::new(),
            color: 0,
            group: 0,
        });
        id
    }

    pub fn text(&self, id: u64) -> Option<&TextBox> {
        self.texts.iter().find(|t| t.id == id)
    }

    pub fn text_mut(&mut self, id: u64) -> Option<&mut TextBox> {
        self.texts.iter_mut().find(|t| t.id == id)
    }

    pub fn add_shape(&mut self, kind: ShapeKind, x: f32, y: f32) -> u64 {
        let id = self.take_id();
        self.shapes.push(Shape {
            id,
            kind,
            x1: x,
            y1: y,
            x2: x,
            y2: y,
            color: 0,
            group: 0,
            src: String::new(),
        });
        id
    }

    /// An image or sticker with its top-left corner at (x, y).
    pub fn add_picture(
        &mut self,
        kind: ShapeKind,
        src: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> u64 {
        let id = self.add_shape(kind, x, y);
        if let Some(s) = self.shape_mut(id) {
            (s.x2, s.y2) = (x + w, y + h);
            s.src = src.to_string();
            s.color = (id % PALETTE_LEN as u64) as u8;
        }
        id
    }

    pub fn shape(&self, id: u64) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn shape_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn add_stroke(&mut self, x: f32, y: f32, color: u8) -> u64 {
        let id = self.take_id();
        self.strokes.push(Stroke {
            id,
            pts: vec![(x, y)],
            color,
            width: default_stroke_width(),
        });
        id
    }

    pub fn stroke_mut(&mut self, id: u64) -> Option<&mut Stroke> {
        self.strokes.iter_mut().find(|s| s.id == id)
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.areas.iter().position(|a| a.id == id)
    }

    pub fn area(&self, id: u64) -> Option<&Area> {
        self.areas.iter().find(|a| a.id == id)
    }

    pub fn area_mut(&mut self, id: u64) -> Option<&mut Area> {
        self.areas.iter_mut().find(|a| a.id == id)
    }

    #[cfg(test)]
    pub fn remove_area(&mut self, id: u64) -> bool {
        match self.index_of(id) {
            Some(i) => {
                self.areas.remove(i);
                true
            }
            None => false,
        }
    }

    /// Where `C` captures to: the area named Inbox, else the first area.
    pub fn inbox_index(&self) -> Option<usize> {
        self.areas
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case("inbox"))
            .or(if self.areas.is_empty() { None } else { Some(0) })
    }
}

/// Undo/redo by snapshot of areas, notes, strokes, texts and shapes .
/// Boards are small, so this is simpler and sturdier than inverse commands; the stack is
/// capped to bound memory. Consecutive edits with the same group key within `GROUP_WINDOW`
/// (typing in one field) share a single step.
pub const HISTORY_CAP: usize = 64;
const GROUP_WINDOW: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone)]
struct Snap {
    areas: Vec<Area>,
    notes: Vec<Note>,
    strokes: Vec<Stroke>,
    texts: Vec<TextBox>,
    shapes: Vec<Shape>,
    connectors: Vec<Connector>,
}

impl Snap {
    fn of(b: &Board) -> Snap {
        Snap {
            areas: b.areas.clone(),
            notes: b.notes.clone(),
            strokes: b.strokes.clone(),
            texts: b.texts.clone(),
            shapes: b.shapes.clone(),
            connectors: b.connectors.clone(),
        }
    }
    /// Swap this snapshot with the board's current content.
    fn swap(&mut self, b: &mut Board) {
        std::mem::swap(&mut self.areas, &mut b.areas);
        std::mem::swap(&mut self.notes, &mut b.notes);
        std::mem::swap(&mut self.strokes, &mut b.strokes);
        std::mem::swap(&mut self.texts, &mut b.texts);
        std::mem::swap(&mut self.shapes, &mut b.shapes);
        std::mem::swap(&mut self.connectors, &mut b.connectors);
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last_group: Option<((u64, usize), Instant)>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call right BEFORE mutating the board. `group` coalesces repeated edits to one target.
    pub fn record(&mut self, board: &Board, group: Option<(u64, usize)>) {
        let now = Instant::now();
        if let (Some(g), Some((lg, t))) = (group, self.last_group) {
            if g == lg && now.duration_since(t) < GROUP_WINDOW {
                self.last_group = Some((g, now));
                return;
            }
        }
        self.undo.push(Snap::of(board));
        if self.undo.len() > HISTORY_CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_group = group.map(|g| (g, now));
    }

    pub fn undo(&mut self, board: &mut Board) -> bool {
        let Some(mut prev) = self.undo.pop() else {
            return false;
        };
        prev.swap(board); // `prev` now holds the state we are leaving
        self.redo.push(prev);
        self.last_group = None;
        true
    }

    pub fn redo(&mut self, board: &mut Board) -> bool {
        let Some(mut next) = self.redo.pop() else {
            return false;
        };
        next.swap(board);
        self.undo.push(next);
        self.last_group = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hero_with(open: usize, done: usize) -> Area {
        let mut a = Board::default_board().areas.remove(1);
        assert!(a.hero);
        a.todos.clear();
        for i in 0..open {
            a.todos.push(Todo::new(&format!("open{i}")));
        }
        for i in 0..done {
            a.todos.push(Todo {
                text: format!("done{i}"),
                done: true,
                priority: 0,
                body: None,
            });
        }
        a
    }

    #[test]
    fn default_board_is_the_life_os_layout() {
        let b = Board::default_board();
        let names: Vec<&str> = b.areas.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            ["Inbox", "Focus", "Work", "Startups", "Health", "Learning", "Life"]
        );
        assert_eq!(b.areas.iter().filter(|a| a.hero).count(), 1);
        assert!(b.areas[1].hero && b.areas[1].width() == HERO_W);
        for w in b.areas.windows(2) {
            assert!(
                w[1].x >= w[0].x + w[0].width(),
                "areas run left to right without overlap"
            );
        }
        let ids: std::collections::HashSet<u64> = b.areas.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), 7);
        assert_eq!(b.inbox_index(), Some(0));
    }

    #[test]
    fn hero_is_limited_to_three_open_todos() {
        let mut a = hero_with(2, 0);
        assert_eq!(a.insert_todo(2), Some(2));
        assert!(a.is_full(), "3 open todos is full");
        assert_eq!(a.insert_todo(0), None, "4th open todo refused");
        a.todos[0].done = true;
        assert!(!a.is_full());
        assert!(a.insert_todo(0).is_some());
    }

    #[test]
    fn done_todos_do_not_count_toward_the_limit() {
        let a = hero_with(3, 5);
        assert_eq!(a.open_count(), 3);
        assert!(a.is_full());
        let mut b = Board::default_board().areas.remove(2);
        for _ in 0..10 {
            assert!(b.insert_todo(0).is_some(), "only the hero area is limited");
        }
    }

    #[test]
    fn reopening_in_a_full_hero_area_is_refused() {
        let mut a = hero_with(3, 1);
        let done_idx = 3;
        assert!(!a.toggle_done(done_idx), "re-open would make 4 open");
        assert!(a.todos[done_idx].done);
        assert!(a.toggle_done(0), "completing is always allowed");
        assert!(a.toggle_done(done_idx), "now there is room to re-open");
    }

    #[test]
    fn priority_cycles_zero_to_three_and_wraps() {
        let mut a = hero_with(1, 0);
        let seen: Vec<u8> = (0..5)
            .map(|_| {
                a.cycle_priority(0);
                a.todos[0].priority
            })
            .collect();
        assert_eq!(seen, [1, 2, 3, 0, 1]);
        a.cycle_priority(99); // out of range is a no-op
    }

    #[test]
    fn colour_cycles_through_the_palette() {
        let mut a = hero_with(0, 0);
        let start = a.color;
        for _ in 0..PALETTE_LEN {
            a.cycle_color();
        }
        assert_eq!(a.color, start);
    }

    #[test]
    fn add_and_remove_areas() {
        let mut b = Board::new();
        let a = b.add_area(5.0, 6.0);
        let c = b.add_area(0.0, 0.0);
        assert_ne!(a, c);
        assert_eq!(b.area(a).unwrap().name, "New group");
        assert!(b.remove_area(a));
        assert!(!b.remove_area(a));
        assert_eq!(b.areas.len(), 1);
    }

    #[test]
    fn inbox_falls_back_to_first_area() {
        let mut b = Board::new();
        assert_eq!(b.inbox_index(), None);
        b.add_area(0.0, 0.0);
        b.add_area(0.0, 0.0);
        assert_eq!(b.inbox_index(), Some(0));
        b.areas[1].name = "INBOX".into();
        assert_eq!(b.inbox_index(), Some(1));
    }

    #[test]
    fn older_files_load_via_serde_defaults() {
        // An old file that predates `hero`, `color`, `done` and `priority`.
        let json = r#"[{"id":1,"name":"Old","x":0,"y":0,"todos":[{"text":"keep"}]}]"#;
        let areas: Vec<Area> = serde_json::from_str(json).unwrap();
        assert!(!areas[0].hero && areas[0].color == 0);
        assert_eq!(areas[0].todos[0], Todo::new("keep"));
    }

    #[test]
    fn json_roundtrip() {
        let b = Board::default_board();
        let json = serde_json::to_string(&b.areas).unwrap();
        let back: Vec<Area> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b.areas);
    }

    #[test]
    fn from_parts_repairs() {
        let mk = |id, color, prio| Area {
            id,
            name: "a".into(),
            color,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            hero: false,
            todos: vec![Todo {
                text: "t".into(),
                done: false,
                priority: prio,
                body: None,
            }],
            group: 0,
        };
        let b = Board::from_parts(
            vec![mk(7, 99, 9), mk(7, 1, 1), mk(3, 2, 2)],
            vec![],
            vec![],
            vec![],
            vec![],
            1,
        );
        let ids: Vec<u64> = b.areas.iter().map(|a| a.id).collect();
        assert!(
            ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2],
            "{ids:?}"
        );
        assert!(b.next_id() > *ids.iter().max().unwrap());
        assert!(b.areas[0].color < PALETTE_LEN);
        assert_eq!(b.areas[0].todos[0].priority, MAX_PRIORITY);
    }

    #[test]
    fn history_undo_redo_and_clears_redo() {
        let (mut b, mut h) = (Board::new(), History::new());
        h.record(&b, None);
        let id = b.add_area(0.0, 0.0);
        assert!(h.undo(&mut b));
        assert!(b.areas.is_empty());
        assert!(h.redo(&mut b));
        assert_eq!(b.areas.len(), 1);
        assert!(b.next_id() > id);
        assert!(!h.redo(&mut b));
        h.undo(&mut b);
        h.record(&b, None);
        b.add_area(1.0, 1.0);
        assert!(!h.redo(&mut b), "a new change clears redo");
    }

    #[test]
    fn history_groups_typing_but_not_other_targets() {
        let (mut b, mut h) = (Board::new(), History::new());
        let id = b.add_area(0.0, 0.0);
        for _ in 0..5 {
            h.record(&b, Some((id, 0)));
            b.area_mut(id).unwrap().name.push('a');
        }
        assert!(h.undo(&mut b));
        assert_eq!(
            b.area(id).unwrap().name,
            "New group",
            "quick keystrokes are one step"
        );
        assert!(!h.undo(&mut b));
        h.redo(&mut b);
        h.record(&b, Some((id, 1)));
        b.area_mut(id).unwrap().name.push('b');
        assert!(h.undo(&mut b));
        assert!(b.area(id).unwrap().name.ends_with("aaaaa"));
    }

    #[test]
    fn history_is_capped() {
        let (mut b, mut h) = (Board::new(), History::new());
        for _ in 0..(HISTORY_CAP + 30) {
            h.record(&b, None);
            b.add_area(0.0, 0.0);
        }
        let mut n = 0;
        while h.undo(&mut b) {
            n += 1;
        }
        assert_eq!(n, HISTORY_CAP);
    }

    #[test]
    fn notes_and_strokes_round_trip_with_defaults() {
        let json = r#"{"id":4,"x":1,"y":2}"#;
        let n: Note = serde_json::from_str(json).unwrap();
        assert_eq!((n.title.as_str(), n.body.as_str(), n.color), ("", "", 0));
        let st: Stroke = serde_json::from_str(r#"{"id":5,"pts":[[0,0],[1,1]]}"#).unwrap();
        assert_eq!(st.width, 3.0);
        let mut b = Board::new();
        let id = b.add_note(5.0, 6.0);
        b.note_mut(id).unwrap().body = "line1\nline2".into();
        let back: Note =
            serde_json::from_str(&serde_json::to_string(&b.notes[0]).unwrap()).unwrap();
        assert_eq!(back, b.notes[0]);
    }

    #[test]
    fn todo_body_is_optional_and_skipped_when_none() {
        let t = Todo::new("x");
        assert!(!serde_json::to_string(&t).unwrap().contains("body"));
        let mut t2 = Todo::new("y");
        t2.body = Some("details\nmore".into());
        let back: Todo = serde_json::from_str(&serde_json::to_string(&t2).unwrap()).unwrap();
        assert_eq!(back.body.as_deref(), Some("details\nmore"));
    }

    #[test]
    fn history_restores_notes_and_strokes_too() {
        let (mut b, mut h) = (Board::new(), History::new());
        h.record(&b, None);
        b.add_note(0.0, 0.0);
        h.record(&b, None);
        b.add_stroke(0.0, 0.0, 0);
        assert!(h.undo(&mut b));
        assert!(b.strokes.is_empty() && b.notes.len() == 1);
        assert!(h.undo(&mut b));
        assert!(b.notes.is_empty());
        assert!(h.redo(&mut b) && h.redo(&mut b));
        assert!(b.notes.len() == 1 && b.strokes.len() == 1);
    }

    #[test]
    fn ids_are_unique_across_item_kinds() {
        let mut b = Board::new();
        let a = b.add_area(0.0, 0.0);
        let n = b.add_note(0.0, 0.0);
        let s = b.add_stroke(0.0, 0.0, 0);
        assert!(a != n && n != s && a != s);
    }

    #[test]
    fn from_parts_cleans_strokes() {
        let st = |id, pts: Vec<(f32, f32)>, w: f32| Stroke {
            id,
            pts,
            color: 99,
            width: w,
        };
        let b = Board::from_parts(
            vec![],
            vec![],
            vec![
                st(1, vec![], 3.0),
                st(2, vec![(0.0, 0.0)], f32::NAN),
                st(2, vec![(f32::NAN, 0.0), (1.0, 1.0)], 999.0),
            ],
            vec![],
            vec![],
            1,
        );
        assert_eq!(b.strokes.len(), 2, "empty stroke dropped");
        assert!(b.strokes[0].width == 3.0 && b.strokes[1].width == 40.0);
        assert_eq!(b.strokes[1].pts.len(), 1, "non-finite points dropped");
        assert_ne!(b.strokes[0].id, b.strokes[1].id);
        assert!(b.strokes[0].color < PALETTE_LEN);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#0A84FF"), Some((0x0a, 0x84, 0xff)));
        assert_eq!(parse_hex("141416"), Some((0x14, 0x14, 0x16)));
        assert_eq!(parse_hex("#12"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn background_cycle_without_image_visits_every_preset_then_default() {
        let mut b = Background::default();
        assert_eq!(b.solid(), None);
        let mut seen = Vec::new();
        for _ in 0..BACKGROUND_PRESETS.len() {
            b.cycle(&[]);
            seen.push(b.color.clone().unwrap());
        }
        assert_eq!(seen, BACKGROUND_PRESETS);
        b.cycle(&[]);
        assert_eq!(
            (b.color.clone(), b.image_on),
            (None, false),
            "back to the theme default"
        );
    }

    fn two() -> Vec<String> {
        vec!["/w/a.png".to_string(), "/w/b.png".to_string()]
    }

    #[test]
    fn background_cycle_with_images_visits_each_wallpaper_then_default() {
        let imgs = two();
        let mut b = Background::default();
        for _ in 0..BACKGROUND_PRESETS.len() {
            b.cycle(&imgs);
        }
        b.cycle(&imgs);
        assert!(b.image_on && b.color.is_none() && b.solid().is_none());
        assert_eq!(b.image.as_deref(), Some("/w/a.png"));
        b.cycle(&imgs);
        assert_eq!(b.image.as_deref(), Some("/w/b.png"));
        b.cycle(&imgs);
        assert!(!b.image_on && b.color.is_none(), "last wallpaper -> default");
    }

    #[test]
    fn advance_walks_in_order_and_random_never_repeats() {
        let imgs = two();
        let mut b = Background {
            image_on: true,
            image: Some(imgs[0].clone()),
            ..Background::default()
        };
        assert!(b.advance(&imgs, 7));
        assert_eq!(b.image.as_deref(), Some("/w/b.png"));
        assert!(b.advance(&imgs, 7));
        assert_eq!(b.image.as_deref(), Some("/w/a.png"), "wraps around");
        b.random = true;
        let three: Vec<String> = (0..3).map(|i| format!("/w/{i}.png")).collect();
        b.image = Some(three[1].clone());
        for rnd in 0..20 {
            let before = b.image.clone();
            assert!(b.advance(&three, rnd));
            assert_ne!(b.image, before, "random pick differs from the current one");
        }
        assert!(!b.advance(&three[..1], 0), "one wallpaper: nothing to rotate to");
    }

    #[test]
    fn rotation_key_goes_off_ordered_random_off() {
        let mut b = Background::default();
        b.cycle_rotation();
        assert_eq!((b.rotate_mins, b.random), (DEFAULT_ROTATE_MINS, false));
        b.cycle_rotation();
        assert_eq!((b.rotate_mins, b.random), (DEFAULT_ROTATE_MINS, true));
        b.cycle_rotation();
        assert_eq!((b.rotate_mins, b.random), (0, false));
    }

    #[test]
    fn unknown_or_invalid_colour_falls_back_cleanly() {
        let mut b = Background {
            color: Some("not-a-colour".into()),
            ..Background::default()
        };
        assert_eq!(b.solid(), None);
        b.cycle(&[]);
        assert_eq!(
            b.color, None,
            "an unknown colour resets to the default instead of looping"
        );
    }

    #[test]
    fn background_defaults_load_from_an_empty_object() {
        let b: Background = serde_json::from_str("{}").unwrap();
        assert_eq!(b, Background::default());
        assert!((b.dim - 0.35).abs() < 1e-6);
    }

    #[test]
    fn texts_and_shapes_have_unique_ids_defaults_and_roundtrip() {
        let mut b = Board::new();
        let t = b.add_text(5.0, 6.0);
        let s = b.add_shape(ShapeKind::Ellipse, 1.0, 2.0);
        let n = b.add_note(0.0, 0.0);
        assert!(t != s && s != n && t != n);
        b.text_mut(t).unwrap().text = "hello\nworld".into();
        b.shape_mut(s).unwrap().x2 = 50.0;
        let tj = serde_json::to_string(&b.texts[0]).unwrap();
        assert_eq!(serde_json::from_str::<TextBox>(&tj).unwrap(), b.texts[0]);
        let sj = serde_json::to_string(&b.shapes[0]).unwrap();
        assert!(sj.contains("\"kind\":\"ellipse\""), "{sj}");
        assert_eq!(serde_json::from_str::<Shape>(&sj).unwrap(), b.shapes[0]);
        let t2: TextBox = serde_json::from_str(r#"{"id":9,"x":1,"y":2}"#).unwrap();
        assert_eq!((t2.text.as_str(), t2.color), ("", 0));
    }

    #[test]
    fn shape_bbox_is_normalised_whatever_the_drag_direction() {
        let mut b = Board::new();
        let s = b.add_shape(ShapeKind::Rect, 100.0, 80.0);
        let sh = b.shape_mut(s).unwrap();
        (sh.x2, sh.y2) = (40.0, 20.0);
        assert_eq!(b.shape(s).unwrap().bbox(), (40.0, 20.0, 60.0, 60.0));
    }

    #[test]
    fn history_restores_texts_and_shapes_and_from_parts_cleans_them() {
        let (mut b, mut h) = (Board::new(), History::new());
        h.record(&b, None);
        b.add_text(0.0, 0.0);
        h.record(&b, None);
        b.add_shape(ShapeKind::Line, 0.0, 0.0);
        assert!(h.undo(&mut b));
        assert!(b.shapes.is_empty() && b.texts.len() == 1);
        assert!(h.undo(&mut b) && b.texts.is_empty());
        assert!(h.redo(&mut b) && h.redo(&mut b));
        assert!(b.texts.len() == 1 && b.shapes.len() == 1);
        let tx = |id| TextBox {
            id,
            x: f32::NAN,
            y: 1.0,
            text: String::new(),
            color: 77,
            group: 0,
        };
        let sp = |id| Shape {
            src: String::new(),
            id,
            kind: ShapeKind::Rect,
            x1: f32::INFINITY,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
            color: 99,
            group: 0,
        };
        let c = Board::from_parts(
            vec![],
            vec![],
            vec![],
            vec![tx(4), tx(4)],
            vec![sp(4), sp(4)],
            1,
        );
        assert_ne!(c.texts[0].id, c.texts[1].id);
        assert_ne!(c.shapes[0].id, c.shapes[1].id);
        assert!(c.texts[0].x.is_finite() && c.shapes[0].x1.is_finite());
        assert!(c.texts[0].color < PALETTE_LEN && c.shapes[0].color < PALETTE_LEN);
        // ids only need to be unique within a kind (every lookup is per kind)
        assert_eq!((c.texts.len(), c.shapes.len()), (2, 2));
    }

    #[test]
    fn grouped_items_move_together_and_ungroup() {
        use crate::layout::Sel;
        let mut b = Board::new();
        let n = b.add_note(0.0, 0.0);
        let t = b.add_text(100.0, 100.0);
        let s = b.add_shape(ShapeKind::Rect, 10.0, 10.0);
        assert!(
            b.group_items(&[Sel::Note(n)]).is_none(),
            "one item is no group"
        );
        let g = b.group_items(&[Sel::Note(n), Sel::Text(t)]).unwrap();
        assert_eq!(b.group_members(g).len(), 2);
        // adding a member of an existing group pulls in the whole group
        let g2 = b.group_items(&[Sel::Text(t), Sel::Shape(s)]).unwrap();
        assert_eq!(b.group_members(g2).len(), 3);
        assert!(b.group_members(g).is_empty());
        b.translate(Sel::Note(n), 5.0, 6.0);
        assert_eq!((b.note(n).unwrap().x, b.note(n).unwrap().y), (5.0, 6.0));
        assert!(b.ungroup(Sel::Shape(s)));
        assert_eq!(b.group_of(Sel::Note(n)), 0);
    }
}

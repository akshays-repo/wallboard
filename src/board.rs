//! Board data model: areas (groups of todos), launchers, undo history. Pure Rust, no Wayland.
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
}

pub const NOTE_W: f32 = 240.0;

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
    /// The hero card: wider, accent border, limited to `MAX_HERO_OPEN` open todos.
    #[serde(default)]
    pub hero: bool,
    #[serde(default)]
    pub todos: Vec<Todo>,
}

impl Area {
    pub fn width(&self) -> f32 {
        if self.hero {
            HERO_W
        } else {
            CARD_W
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Launcher {
    pub label: String,
    /// Run with `sh -c`, detached.
    pub command: String,
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
}

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

    /// Next background for the `B` key: theme default -> each preset -> wallpaper (if
    /// `have_image`) -> theme default.
    pub fn cycle(&mut self, have_image: bool) {
        if self.image_on {
            self.image_on = false;
            self.color = None;
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
                self.image_on = have_image;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Board {
    /// Left-to-right order; keys 1-9 jump to the area at that position.
    pub areas: Vec<Area>,
    pub notes: Vec<Note>,
    pub strokes: Vec<Stroke>,
    pub launchers: Vec<Launcher>,
    next_id: u64,
}

pub fn default_launchers() -> Vec<Launcher> {
    vec![
        Launcher {
            label: "Files".into(),
            command: "xdg-open \"$HOME\"".into(),
        },
        Launcher {
            label: "Browser".into(),
            command: "xdg-open about:blank".into(),
        },
        Launcher {
            label: "Terminal".into(),
            command: "alacritty".into(),
        },
    ]
}

impl Board {
    pub fn new() -> Self {
        Board {
            areas: Vec::new(),
            notes: Vec::new(),
            strokes: Vec::new(),
            launchers: default_launchers(),
            next_id: 1,
        }
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
        launchers: Vec<Launcher>,
        next_id: u64,
    ) -> Self {
        let mut next = next_id.max(1);
        for id in areas
            .iter()
            .map(|a| a.id)
            .chain(notes.iter().map(|n| n.id))
            .chain(strokes.iter().map(|s| s.id))
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
        Board {
            areas,
            notes,
            strokes,
            launchers,
            next_id: next,
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
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
            hero: false,
            todos: Vec::new(),
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

/// Undo/redo by snapshot of areas, notes and strokes (launchers are edited in the file).
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
}

impl Snap {
    fn of(b: &Board) -> Snap {
        Snap {
            areas: b.areas.clone(),
            notes: b.notes.clone(),
            strokes: b.strokes.clone(),
        }
    }
    /// Swap this snapshot with the board's current content.
    fn swap(&mut self, b: &mut Board) {
        std::mem::swap(&mut self.areas, &mut b.areas);
        std::mem::swap(&mut self.notes, &mut b.notes);
        std::mem::swap(&mut self.strokes, &mut b.strokes);
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
        assert_eq!(b.launchers.len(), 3);
        assert_eq!(b.launchers[0].label, "Files");
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
            hero: false,
            todos: vec![Todo {
                text: "t".into(),
                done: false,
                priority: prio,
                body: None,
            }],
        };
        let b = Board::from_parts(
            vec![mk(7, 99, 9), mk(7, 1, 1), mk(3, 2, 2)],
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
            b.cycle(false);
            seen.push(b.color.clone().unwrap());
        }
        assert_eq!(seen, BACKGROUND_PRESETS);
        b.cycle(false);
        assert_eq!(
            (b.color.clone(), b.image_on),
            (None, false),
            "back to the theme default"
        );
    }

    #[test]
    fn background_cycle_with_image_ends_on_the_wallpaper() {
        let mut b = Background::default();
        for _ in 0..BACKGROUND_PRESETS.len() {
            b.cycle(true);
        }
        b.cycle(true);
        assert!(b.image_on && b.color.is_none() && b.solid().is_none());
        b.cycle(true);
        assert!(!b.image_on && b.color.is_none(), "wallpaper -> default");
    }

    #[test]
    fn unknown_or_invalid_colour_falls_back_cleanly() {
        let mut b = Background {
            color: Some("not-a-colour".into()),
            ..Background::default()
        };
        assert_eq!(b.solid(), None);
        b.cycle(false);
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
}

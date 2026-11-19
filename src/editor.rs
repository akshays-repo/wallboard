//! Editor state machine: tools, selection-free direct manipulation, text editing, pen/eraser,
//! undo bookkeeping. Pure logic: takes decoded input and a char-advance function, so it can be
//! unit-tested without Wayland or fonts.

use crate::board::{Board, History, Todo};
use crate::camera::Camera;
use crate::layout::{area_geom, hit_test, note_geom, Adv, Target, Tool};
use crate::text::{index_at_x, line_of, Style};

/// Pointer must travel this many screen px before a press on a card becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;
/// Max gap and distance for a double click.
const DOUBLE_CLICK_MS: u32 = 400;
const DOUBLE_CLICK_PX: f32 = 8.0;
/// Pen points closer than this many screen px to the previous one are dropped.
const PEN_MIN_STEP_PX: f32 = 2.0;
/// Eraser radius in screen px.
const ERASER_PX: f32 = 9.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Text(String),
    Backspace,
    Delete,
    Enter,
    /// Ctrl+Enter
    ToggleDone,
    /// Ctrl+P
    CyclePriority,
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Escape,
}

/// The text field being edited. Areas and notes are addressed by id, todos by (area id, row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    AreaTitle(u64),
    TodoText(u64, usize),
    TodoBody(u64, usize),
    NoteTitle(u64),
    NoteBody(u64),
}

impl EditTarget {
    fn owner(self) -> u64 {
        match self {
            EditTarget::AreaTitle(i) | EditTarget::NoteTitle(i) | EditTarget::NoteBody(i) => i,
            EditTarget::TodoText(i, _) | EditTarget::TodoBody(i, _) => i,
        }
    }
    /// Distinct per field so typing in different fields never shares an undo step.
    fn key(self) -> usize {
        match self {
            EditTarget::AreaTitle(_) => 0,
            EditTarget::NoteTitle(_) => 1,
            EditTarget::NoteBody(_) => 2,
            EditTarget::TodoText(_, r) => 3 + 2 * r,
            EditTarget::TodoBody(_, r) => 4 + 2 * r,
        }
    }
    fn multiline(self) -> bool {
        matches!(self, EditTarget::TodoBody(..) | EditTarget::NoteBody(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edit {
    pub target: EditTarget,
    /// Caret as a char index into the field's text.
    pub caret: usize,
}

#[derive(Debug, Clone, Copy)]
enum Mover {
    Area(u64),
    Note(u64),
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    None,
    Pan {
        last: (f32, f32),
    },
    /// Pressed on a card/note title: becomes a move past the threshold. For notes, releasing
    /// without moving starts editing the title.
    Pending {
        who: Mover,
        start: (f32, f32),
        off: (f32, f32),
    },
    Move {
        who: Mover,
        off: (f32, f32),
    },
    Draw {
        id: u64,
    },
    Erase {
        recorded: bool,
    },
}

/// What a todo/note removal shifted, so a click aimed at the old layout can be re-aimed.
#[derive(Debug, Clone, Copy)]
enum Removed {
    Todo { area: usize, row: usize },
    Note { index: usize },
}

#[derive(Debug)]
pub struct Editor {
    pub tool: Tool,
    pub editing: Option<Edit>,
    drag: Drag,
    last_click: Option<(u32, f32, f32)>,
    pub history: History,
    /// Colour index used by the pen.
    pub pen_color: u8,
    /// Saved content changed since the flag was last taken (drives autosave and backup).
    modified: bool,
    /// The camera moved by dragging.
    view_changed: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Horizontal scroll (same unit as `text_w`) that keeps the caret visible in a single-line field.
pub fn scroll_x(text_w: f32, caret_x: f32) -> f32 {
    (caret_x - (text_w - 6.0)).max(0.0)
}

pub fn text_of(board: &Board, t: EditTarget) -> Option<&str> {
    match t {
        EditTarget::AreaTitle(id) => board.area(id).map(|a| a.name.as_str()),
        EditTarget::TodoText(id, r) => board.area(id)?.todos.get(r).map(|t| t.text.as_str()),
        EditTarget::TodoBody(id, r) => board.area(id)?.todos.get(r)?.body.as_deref(),
        EditTarget::NoteTitle(id) => board.note(id).map(|n| n.title.as_str()),
        EditTarget::NoteBody(id) => board.note(id).map(|n| n.body.as_str()),
    }
}

fn text_mut(board: &mut Board, t: EditTarget) -> Option<&mut String> {
    match t {
        EditTarget::AreaTitle(id) => board.area_mut(id).map(|a| &mut a.name),
        EditTarget::TodoText(id, r) => board.area_mut(id)?.todos.get_mut(r).map(|t| &mut t.text),
        EditTarget::TodoBody(id, r) => board.area_mut(id)?.todos.get_mut(r)?.body.as_mut(),
        EditTarget::NoteTitle(id) => board.note_mut(id).map(|n| &mut n.title),
        EditTarget::NoteBody(id) => board.note_mut(id).map(|n| &mut n.body),
    }
}

/// The text style a field is drawn in (also what the editor measures with).
pub fn style_of(board: &Board, t: EditTarget) -> Style {
    match t {
        EditTarget::AreaTitle(id) => {
            if board.area(id).is_some_and(|a| a.hero) {
                Style::HeroTitle
            } else {
                Style::Title
            }
        }
        EditTarget::TodoText(..) => Style::Row,
        EditTarget::TodoBody(..) => Style::Chip,
        EditTarget::NoteTitle(_) => Style::Title,
        EditTarget::NoteBody(_) => Style::Row,
    }
}

/// The wrapped lines of a multi-line field (`None` for single-line fields).
pub fn lines_of(board: &Board, t: EditTarget, adv: Adv) -> Option<Vec<crate::text::Line>> {
    match t {
        EditTarget::TodoBody(id, r) => {
            let a = board.area(id)?;
            area_geom(a, adv)
                .rows
                .get(r)?
                .body
                .as_ref()
                .map(|b| b.lines.clone())
        }
        EditTarget::NoteBody(id) => Some(note_geom(board.note(id)?, adv).body.lines),
        _ => None,
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            tool: Tool::Select,
            editing: None,
            drag: Drag::None,
            last_click: None,
            history: History::new(),
            pen_color: 0,
            modified: false,
            view_changed: false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// True once after any change to saved content.
    pub fn take_modified(&mut self) -> bool {
        std::mem::take(&mut self.modified)
    }

    /// True once after the camera was moved by a drag.
    pub fn take_view_changed(&mut self) -> bool {
        std::mem::take(&mut self.view_changed)
    }

    pub fn set_tool(&mut self, board: &mut Board, tool: Tool) {
        self.finish_edit(board);
        self.drag = Drag::None;
        self.tool = tool;
    }

    fn start_editing(&mut self, target: EditTarget, caret: usize) {
        self.editing = Some(Edit { target, caret });
    }

    fn start_editing_end(&mut self, board: &Board, target: EditTarget) {
        let caret = text_of(board, target).map_or(0, |s| s.chars().count());
        self.start_editing(target, caret);
    }

    /// Stop editing, tidying up: an empty todo or note is discarded, an empty body collapses.
    fn finish_edit(&mut self, board: &mut Board) -> Option<Removed> {
        let edit = self.editing.take()?;
        match edit.target {
            EditTarget::TodoBody(id, r) => {
                let empty = text_of(board, edit.target).is_some_and(str::is_empty);
                if empty {
                    self.history.record(board, None);
                    if let Some(t) = board.area_mut(id).and_then(|a| a.todos.get_mut(r)) {
                        t.body = None;
                    }
                    self.modified = true;
                }
                None
            }
            EditTarget::TodoText(id, r) => {
                let empty = board
                    .area(id)
                    .and_then(|a| a.todos.get(r))
                    .is_some_and(|t| t.text.is_empty() && t.body.is_none());
                if empty {
                    self.history.record(board, None);
                    let ai = board.index_of(id)?;
                    board.areas[ai].todos.remove(r);
                    self.modified = true;
                    return Some(Removed::Todo { area: ai, row: r });
                }
                None
            }
            EditTarget::NoteTitle(id) | EditTarget::NoteBody(id) => {
                let empty = board
                    .note(id)
                    .is_some_and(|n| n.title.is_empty() && n.body.is_empty());
                if empty {
                    self.history.record(board, None);
                    let ni = board.notes.iter().position(|n| n.id == id)?;
                    board.remove_note(id);
                    self.modified = true;
                    return Some(Removed::Note { index: ni });
                }
                None
            }
            EditTarget::AreaTitle(_) => None,
        }
    }

    /// Re-aim a hit-test result computed before `removed` shifted the layout.
    fn remap(t: Option<Target>, removed: Option<Removed>) -> Option<Target> {
        let Some(rm) = removed else {
            return t;
        };
        match (t?, rm) {
            (
                Target::Circle(a, r)
                | Target::Text(a, r)
                | Target::Prio(a, r)
                | Target::TodoBody(a, r),
                Removed::Todo { area, row },
            ) if a == area => {
                if r == row {
                    return None; // the click landed on the row that just vanished
                }
                let r2 = if r > row { r - 1 } else { r };
                Some(match t? {
                    Target::Circle(..) => Target::Circle(a, r2),
                    Target::Text(..) => Target::Text(a, r2),
                    Target::Prio(..) => Target::Prio(a, r2),
                    _ => Target::TodoBody(a, r2),
                })
            }
            (
                Target::NoteTitle(n) | Target::NoteBody(n) | Target::NoteDot(n),
                Removed::Note { index },
            ) => {
                if n == index {
                    None
                } else {
                    let n2 = if n > index { n - 1 } else { n };
                    Some(match t? {
                        Target::NoteTitle(_) => Target::NoteTitle(n2),
                        Target::NoteBody(_) => Target::NoteBody(n2),
                        _ => Target::NoteDot(n2),
                    })
                }
            }
            (
                Target::NoteTitle(n) | Target::NoteBody(n) | Target::NoteDot(n),
                Removed::Todo { .. },
            ) => {
                let _ = n;
                t
            }
            _ => t,
        }
    }

    // ---- creation helpers

    /// New group at a world point; its title is edited immediately.
    pub fn create_area_at(&mut self, board: &mut Board, wx: f32, wy: f32) -> u64 {
        self.history.record(board, None);
        let id = board.add_area(wx, wy);
        self.start_editing(
            EditTarget::AreaTitle(id),
            board.area(id).map_or(0, |a| a.name.chars().count()),
        );
        self.modified = true;
        id
    }

    /// New free note at a world point; its title is edited immediately.
    pub fn create_note_at(&mut self, board: &mut Board, wx: f32, wy: f32) -> u64 {
        self.history.record(board, None);
        let id = board.add_note(wx, wy);
        self.start_editing(EditTarget::NoteTitle(id), 0);
        self.modified = true;
        id
    }

    /// Append an empty todo to area `ai` and edit it. False if the hero area is full.
    pub fn add_todo(&mut self, board: &mut Board, ai: usize) -> bool {
        let Some(a) = board.areas.get(ai) else {
            return false;
        };
        if a.is_full() {
            return false;
        }
        self.history.record(board, None);
        let id = board.areas[ai].id;
        let at = board.areas[ai].todos.len();
        board.areas[ai].insert_todo(at);
        self.start_editing(EditTarget::TodoText(id, at), 0);
        self.modified = true;
        true
    }

    // ---- pointer

    /// A button pressed at screen point `p`. `time` is in ms. Returns true if redraw needed.
    pub fn press(
        &mut self,
        board: &mut Board,
        cam: &Camera,
        p: (f32, f32),
        button: Button,
        time: u32,
        adv: Adv,
    ) -> bool {
        let (wx, wy) = cam.screen_to_world(p.0, p.1);
        self.drag = Drag::None;
        match button {
            Button::Middle => {
                self.finish_edit(board);
                self.drag = Drag::Pan { last: p };
                true
            }
            Button::Right => self.right_press(board, wx, wy, adv),
            Button::Left => self.left_press(board, cam, p, (wx, wy), time, adv),
        }
    }

    fn right_press(&mut self, board: &mut Board, wx: f32, wy: f32, adv: Adv) -> bool {
        let tgt = hit_test(board, wx, wy, adv);
        let removed = self.finish_edit(board);
        let Some(tgt) = Self::remap(tgt, removed) else {
            return true;
        };
        match tgt {
            Target::Circle(ai, ri)
            | Target::Text(ai, ri)
            | Target::Prio(ai, ri)
            | Target::TodoBody(ai, ri) => {
                self.history.record(board, None);
                if let Some(a) = board.areas.get_mut(ai) {
                    if ri < a.todos.len() {
                        a.todos.remove(ri);
                    }
                }
                self.modified = true;
            }
            Target::Title(ai) | Target::Dot(ai) | Target::NewTodo(ai) | Target::Body(ai) => {
                if board.areas.get(ai).is_some_and(|a| a.todos.is_empty()) {
                    self.history.record(board, None);
                    board.areas.remove(ai);
                    self.modified = true;
                }
            }
            Target::NoteTitle(ni) | Target::NoteBody(ni) | Target::NoteDot(ni) => {
                if ni < board.notes.len() {
                    self.history.record(board, None);
                    board.notes.remove(ni);
                    self.modified = true;
                }
            }
        }
        true
    }

    fn left_press(
        &mut self,
        board: &mut Board,
        cam: &Camera,
        p: (f32, f32),
        (wx, wy): (f32, f32),
        time: u32,
        adv: Adv,
    ) -> bool {
        let double = matches!(self.last_click, Some((t, x, y))
            if time.wrapping_sub(t) <= DOUBLE_CLICK_MS
                && (x - p.0).abs() <= DOUBLE_CLICK_PX
                && (y - p.1).abs() <= DOUBLE_CLICK_PX);
        self.last_click = if double { None } else { Some((time, p.0, p.1)) };

        match self.tool {
            Tool::Pen => {
                self.finish_edit(board);
                self.history.record(board, None);
                let id = board.add_stroke(wx, wy, self.pen_color);
                self.drag = Drag::Draw { id };
                self.modified = true;
                true
            }
            Tool::Eraser => {
                self.finish_edit(board);
                self.drag = Drag::Erase { recorded: false };
                self.erase_at(board, wx, wy, ERASER_PX / cam.zoom);
                true
            }
            Tool::Group | Tool::Note | Tool::Todo => self.place_press(board, wx, wy, adv),
            Tool::Select => self.select_press(board, p, (wx, wy), double, adv),
        }
    }

    /// A click with Group / Note / Todo selected: place one thing, then go back to Select.
    fn place_press(&mut self, board: &mut Board, wx: f32, wy: f32, adv: Adv) -> bool {
        let tgt = hit_test(board, wx, wy, adv);
        let removed = self.finish_edit(board);
        let tgt = Self::remap(tgt, removed);
        let tool = self.tool;
        match tool {
            Tool::Group if tgt.is_none() => {
                self.create_area_at(board, wx, wy);
            }
            Tool::Note if tgt.is_none() => {
                self.create_note_at(board, wx, wy);
            }
            Tool::Todo => {
                let ai = match tgt {
                    Some(
                        Target::Title(a)
                        | Target::Dot(a)
                        | Target::NewTodo(a)
                        | Target::Body(a)
                        | Target::Circle(a, _)
                        | Target::Text(a, _)
                        | Target::Prio(a, _)
                        | Target::TodoBody(a, _),
                    ) => Some(a),
                    _ => board.inbox_index(),
                };
                if let Some(ai) = ai {
                    self.add_todo(board, ai);
                }
            }
            _ => return true, // a Group/Note click on a card does nothing; keep the tool
        }
        self.tool = Tool::Select;
        true
    }

    fn select_press(
        &mut self,
        board: &mut Board,
        p: (f32, f32),
        (wx, wy): (f32, f32),
        double: bool,
        adv: Adv,
    ) -> bool {
        let tgt = hit_test(board, wx, wy, adv);
        // A click inside the field already being edited only moves the caret.
        let wanted = tgt.and_then(|t| Self::edit_target_of(board, t));
        let keep_edit = wanted.is_some() && wanted == self.editing.map(|e| e.target);
        let removed = if keep_edit {
            None
        } else {
            self.finish_edit(board)
        };
        let Some(tgt) = Self::remap(tgt, removed) else {
            if tgt.is_none() {
                return self.empty_press(board, p, (wx, wy), double);
            }
            return true;
        };

        match tgt {
            Target::Dot(ai) => {
                self.history.record(board, None);
                if let Some(a) = board.areas.get_mut(ai) {
                    a.cycle_color();
                }
                self.modified = true;
            }
            Target::NoteDot(ni) => {
                self.history.record(board, None);
                if let Some(n) = board.notes.get_mut(ni) {
                    n.color = (n.color + 1) % crate::board::PALETTE_LEN;
                }
                self.modified = true;
            }
            Target::Circle(ai, ri) => {
                let allowed = board.areas.get(ai).is_some_and(|a| a.can_toggle(ri));
                if allowed {
                    self.history.record(board, None);
                    board.areas[ai].toggle_done(ri);
                    self.modified = true;
                }
            }
            Target::Prio(ai, ri) => {
                if board.areas.get(ai).is_some_and(|a| ri < a.todos.len()) {
                    self.history.record(board, None);
                    board.areas[ai].cycle_priority(ri);
                    self.modified = true;
                }
            }
            Target::NewTodo(ai) => {
                self.add_todo(board, ai);
            }
            Target::Title(ai) => {
                let Some(a) = board.areas.get(ai) else {
                    return true;
                };
                let (id, off) = (a.id, (wx - a.x, wy - a.y));
                if double {
                    let caret = self.caret_for(board, EditTarget::AreaTitle(id), (wx, wy), adv);
                    self.start_editing(EditTarget::AreaTitle(id), caret);
                } else {
                    self.drag = Drag::Pending {
                        who: Mover::Area(id),
                        start: p,
                        off,
                    };
                }
            }
            Target::NoteTitle(ni) => {
                if let Some(n) = board.notes.get(ni) {
                    self.drag = Drag::Pending {
                        who: Mover::Note(n.id),
                        start: p,
                        off: (wx - n.x, wy - n.y),
                    };
                }
            }
            Target::Text(..) | Target::TodoBody(..) | Target::NoteBody(_) => {
                if let Some(t) = Self::edit_target_of(board, tgt) {
                    let caret = self.caret_for(board, t, (wx, wy), adv);
                    self.start_editing(t, caret);
                }
            }
            Target::Body(_) => self.drag = Drag::Pan { last: p },
        }
        true
    }

    fn empty_press(
        &mut self,
        board: &mut Board,
        p: (f32, f32),
        (wx, wy): (f32, f32),
        double: bool,
    ) -> bool {
        if double {
            self.create_area_at(board, wx, wy);
        } else {
            self.drag = Drag::Pan { last: p };
        }
        true
    }

    /// The field a hit would edit, if it is a text target.
    fn edit_target_of(board: &Board, t: Target) -> Option<EditTarget> {
        Some(match t {
            Target::Title(ai) => EditTarget::AreaTitle(board.areas.get(ai)?.id),
            Target::Text(ai, ri) => EditTarget::TodoText(board.areas.get(ai)?.id, ri),
            Target::TodoBody(ai, ri) => EditTarget::TodoBody(board.areas.get(ai)?.id, ri),
            Target::NoteTitle(ni) => EditTarget::NoteTitle(board.notes.get(ni)?.id),
            Target::NoteBody(ni) => EditTarget::NoteBody(board.notes.get(ni)?.id),
            _ => return None,
        })
    }

    /// Caret index under a world point for `target`.
    fn caret_for(
        &self,
        board: &Board,
        target: EditTarget,
        (wx, wy): (f32, f32),
        adv: Adv,
    ) -> usize {
        let style = style_of(board, target);
        let a = |c: char| adv(style, c);
        let Some(text) = text_of(board, target) else {
            return 0;
        };
        match target {
            EditTarget::TodoBody(id, r) => {
                let Some(b) = board
                    .area(id)
                    .and_then(|ar| area_geom(ar, adv).rows.get(r).and_then(|g| g.body.clone()))
                else {
                    return 0;
                };
                caret_in_block(
                    text,
                    &b.lines,
                    b.rect.x,
                    b.rect.y,
                    crate::layout::BODY_LINE_H,
                    wx,
                    wy,
                    &a,
                )
            }
            EditTarget::NoteBody(id) => {
                let Some(n) = board.note(id) else {
                    return 0;
                };
                let g = note_geom(n, adv);
                caret_in_block(
                    text,
                    &g.body.lines,
                    g.body.rect.x,
                    g.body.rect.y,
                    crate::layout::NOTE_LINE_H,
                    wx,
                    wy,
                    &a,
                )
            }
            EditTarget::TodoText(id, r) => {
                let Some(g) = board.area(id).map(|ar| area_geom(ar, adv)) else {
                    return 0;
                };
                let Some(row) = g.rows.get(r) else {
                    return 0;
                };
                // If this field is already being edited its text may be scrolled.
                let scroll = match self.editing {
                    Some(e) if e.target == target => {
                        let cx: f32 = text.chars().take(e.caret).map(&a).sum();
                        scroll_x(row.text.w, cx)
                    }
                    _ => 0.0,
                };
                index_at_x(text, wx - row.text.x + scroll, &a)
            }
            EditTarget::AreaTitle(id) => {
                let Some(g) = board.area(id).map(|ar| area_geom(ar, adv)) else {
                    return 0;
                };
                index_at_x(text, wx - g.title.x, &a)
            }
            EditTarget::NoteTitle(id) => {
                let Some(n) = board.note(id) else {
                    return 0;
                };
                let g = note_geom(n, adv);
                index_at_x(text, wx - g.title.x, &a)
            }
        }
    }

    /// Pointer moved. The camera is moved here when panning.
    pub fn motion(&mut self, board: &mut Board, cam: &mut Camera, p: (f32, f32)) -> bool {
        let (wx, wy) = cam.screen_to_world(p.0, p.1);
        match self.drag {
            Drag::None => false,
            Drag::Pan { last } => {
                cam.pan_by_screen(p.0 - last.0, p.1 - last.1);
                self.drag = Drag::Pan { last: p };
                self.view_changed = true;
                true
            }
            Drag::Pending { who, start, off } => {
                if (p.0 - start.0).hypot(p.1 - start.1) < DRAG_THRESHOLD {
                    return false;
                }
                self.history.record(board, None);
                self.drag = Drag::Move { who, off };
                self.apply_move(board, who, off, wx, wy)
            }
            Drag::Move { who, off } => self.apply_move(board, who, off, wx, wy),
            Drag::Draw { id } => {
                let min = PEN_MIN_STEP_PX / cam.zoom;
                if let Some(s) = board.stroke_mut(id) {
                    let last = *s.pts.last().unwrap_or(&(wx, wy));
                    if (wx - last.0).hypot(wy - last.1) >= min {
                        s.pts.push((wx, wy));
                        self.modified = true;
                        return true;
                    }
                }
                false
            }
            Drag::Erase { .. } => {
                self.erase_at(board, wx, wy, ERASER_PX / cam.zoom);
                true
            }
        }
    }

    fn apply_move(
        &mut self,
        board: &mut Board,
        who: Mover,
        off: (f32, f32),
        wx: f32,
        wy: f32,
    ) -> bool {
        let (x, y) = (wx - off.0, wy - off.1);
        match who {
            Mover::Area(id) => {
                if let Some(a) = board.area_mut(id) {
                    (a.x, a.y) = (x, y);
                    self.modified = true;
                    return true;
                }
            }
            Mover::Note(id) => {
                if let Some(n) = board.note_mut(id) {
                    (n.x, n.y) = (x, y);
                    self.modified = true;
                    return true;
                }
            }
        }
        false
    }

    /// Remove every stroke passing within `radius` (world units) of a point.
    fn erase_at(&mut self, board: &mut Board, wx: f32, wy: f32, radius: f32) {
        let hit: Vec<u64> = board
            .strokes
            .iter()
            .filter(|s| stroke_near(&s.pts, wx, wy, radius + s.width / 2.0))
            .map(|s| s.id)
            .collect();
        if hit.is_empty() {
            return;
        }
        if let Drag::Erase { recorded } = self.drag {
            if !recorded {
                self.history.record(board, None);
                self.drag = Drag::Erase { recorded: true };
            }
        }
        board.strokes.retain(|s| !hit.contains(&s.id));
        self.modified = true;
    }

    /// Button released.
    pub fn release(&mut self, board: &mut Board, adv: Adv) -> bool {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Pending {
                who: Mover::Note(id),
                ..
            } => {
                // Click without dragging on a note title: edit it.
                if board.note(id).is_some() {
                    let t = EditTarget::NoteTitle(id);
                    self.start_editing_end(board, t);
                    let _ = adv;
                    return true;
                }
                false
            }
            Drag::Draw { id } => {
                if let Some(s) = board.stroke_mut(id) {
                    s.pts = smooth(&s.pts);
                    self.modified = true;
                }
                true
            }
            _ => false,
        }
    }

    // ---- undo / redo

    pub fn undo(&mut self, board: &mut Board) -> bool {
        self.finish_edit_quiet();
        let ok = self.history.undo(board);
        if ok {
            self.modified = true;
        }
        ok
    }

    pub fn redo(&mut self, board: &mut Board) -> bool {
        self.finish_edit_quiet();
        let ok = self.history.redo(board);
        if ok {
            self.modified = true;
        }
        ok
    }

    /// Leave edit mode without tidying (used before undo/redo, which replace the content).
    fn finish_edit_quiet(&mut self) {
        self.editing = None;
        self.drag = Drag::None;
    }

    // ---- keyboard

    /// Keyboard input. Returns true if redraw needed. Only acts while editing.
    pub fn key(&mut self, board: &mut Board, key: Key, adv: Adv) -> bool {
        let Some(edit) = self.editing else {
            return false;
        };
        if text_of(board, edit.target).is_none() {
            self.editing = None; // the field vanished (e.g. after undo)
            return true;
        }
        match key {
            Key::Escape => {
                self.finish_edit(board);
            }
            Key::ToggleDone => self.toggle_done_key(board, edit.target),
            Key::CyclePriority => self.priority_key(board, edit.target),
            Key::Tab => self.tab(board, edit.target, false),
            Key::BackTab => self.tab(board, edit.target, true),
            Key::Enter => self.enter(board, edit, adv),
            Key::Up | Key::Down => self.vertical(board, edit, key == Key::Up, adv),
            Key::Text(_)
            | Key::Backspace
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End => self.text_key(board, edit, key, adv),
        }
        true
    }

    fn set_caret(&mut self, caret: usize) {
        if let Some(e) = &mut self.editing {
            e.caret = caret;
        }
    }

    /// Replace the edited field's text, recording undo (typing in one field coalesces).
    fn commit_text(&mut self, board: &mut Board, target: EditTarget, text: String) {
        self.history
            .record(board, Some((target.owner(), target.key())));
        if let Some(t) = text_mut(board, target) {
            *t = text;
        }
        self.modified = true;
    }

    fn text_key(&mut self, board: &mut Board, edit: Edit, key: Key, adv: Adv) {
        let target = edit.target;
        let text = text_of(board, target).unwrap_or("").to_string();
        let mut chars: Vec<char> = text.chars().collect();
        let caret = edit.caret.min(chars.len());
        match key {
            Key::Text(s) => {
                let mut c = caret;
                for ch in s.chars() {
                    if ch == '\n' && !target.multiline() {
                        continue;
                    }
                    chars.insert(c, ch);
                    c += 1;
                }
                self.commit_text(board, target, chars.iter().collect());
                self.set_caret(c);
            }
            Key::Backspace => {
                if caret > 0 {
                    chars.remove(caret - 1);
                    self.commit_text(board, target, chars.iter().collect());
                    self.set_caret(caret - 1);
                } else {
                    self.backspace_at_start(board, target);
                }
            }
            Key::Delete => {
                if caret < chars.len() {
                    chars.remove(caret);
                    self.commit_text(board, target, chars.iter().collect());
                    self.set_caret(caret);
                }
            }
            Key::Left => self.set_caret(caret.saturating_sub(1)),
            Key::Right => self.set_caret((caret + 1).min(chars.len())),
            Key::Home | Key::End => {
                let home = key == Key::Home;
                let c = match lines_of(board, target, adv) {
                    Some(lines) => {
                        let l = lines[line_of(&lines, caret)];
                        if home {
                            l.start
                        } else {
                            l.end
                        }
                    }
                    None => {
                        if home {
                            0
                        } else {
                            chars.len()
                        }
                    }
                };
                self.set_caret(c);
            }
            _ => {}
        }
    }

    /// Backspace with the caret at the very start of a field.
    fn backspace_at_start(&mut self, board: &mut Board, target: EditTarget) {
        match target {
            EditTarget::TodoText(id, r) => {
                let empty = board
                    .area(id)
                    .and_then(|a| a.todos.get(r))
                    .is_some_and(|t| t.text.is_empty() && t.body.is_none());
                if !empty {
                    return;
                }
                // Remove the empty todo and move to the end of the previous one (or the title).
                self.history.record(board, None);
                if let Some(a) = board.area_mut(id) {
                    a.todos.remove(r);
                }
                self.modified = true;
                if r > 0 {
                    self.start_editing_end(board, EditTarget::TodoText(id, r - 1));
                } else {
                    self.start_editing_end(board, EditTarget::AreaTitle(id));
                }
            }
            EditTarget::TodoBody(id, r) => {
                if text_of(board, target).is_some_and(str::is_empty) {
                    self.history.record(board, None);
                    if let Some(t) = board.area_mut(id).and_then(|a| a.todos.get_mut(r)) {
                        t.body = None;
                    }
                    self.modified = true;
                    self.start_editing_end(board, EditTarget::TodoText(id, r));
                }
            }
            EditTarget::NoteBody(id) if text_of(board, target).is_some_and(str::is_empty) => {
                self.start_editing_end(board, EditTarget::NoteTitle(id));
            }
            _ => {}
        }
    }

    fn enter(&mut self, board: &mut Board, edit: Edit, _adv: Adv) {
        let caret = edit.caret;
        match edit.target {
            EditTarget::AreaTitle(_) => {
                self.finish_edit(board);
            }
            EditTarget::NoteTitle(id) => self.start_editing(EditTarget::NoteBody(id), 0),
            EditTarget::NoteBody(_) | EditTarget::TodoBody(..) => {
                self.text_key(board, edit, Key::Text("\n".into()), _adv);
            }
            EditTarget::TodoText(id, r) => {
                let empty = board
                    .area(id)
                    .and_then(|a| a.todos.get(r))
                    .is_some_and(|t| t.text.is_empty() && t.body.is_none());
                let Some(ai) = board.index_of(id) else {
                    return;
                };
                if empty {
                    // Enter on an empty todo discards it and ends the list.
                    self.history.record(board, None);
                    board.areas[ai].todos.remove(r);
                    self.modified = true;
                    self.editing = None;
                    return;
                }
                if board.areas[ai].is_full() {
                    self.finish_edit(board); // no room for another open todo
                    return;
                }
                // Split at the caret: the tail moves into a new todo below.
                let chars: Vec<char> = board.areas[ai].todos[r].text.chars().collect();
                let c = caret.min(chars.len());
                self.history.record(board, None);
                let tail: String = chars[c..].iter().collect();
                board.areas[ai].todos[r].text = chars[..c].iter().collect();
                board.areas[ai].todos.insert(r + 1, Todo::new(&tail));
                self.modified = true;
                self.start_editing(EditTarget::TodoText(id, r + 1), 0);
            }
        }
    }

    fn tab(&mut self, board: &mut Board, target: EditTarget, back: bool) {
        match (target, back) {
            (EditTarget::TodoText(id, r), false) => {
                if text_of(board, EditTarget::TodoBody(id, r)).is_none() {
                    self.history.record(board, None);
                    if let Some(t) = board.area_mut(id).and_then(|a| a.todos.get_mut(r)) {
                        t.body = Some(String::new());
                    }
                    self.modified = true;
                }
                self.start_editing_end(board, EditTarget::TodoBody(id, r));
            }
            (EditTarget::TodoBody(id, r), true) => {
                if text_of(board, target).is_some_and(str::is_empty) {
                    self.history.record(board, None);
                    if let Some(t) = board.area_mut(id).and_then(|a| a.todos.get_mut(r)) {
                        t.body = None;
                    }
                    self.modified = true;
                }
                self.start_editing_end(board, EditTarget::TodoText(id, r));
            }
            (EditTarget::NoteTitle(id), false) => {
                self.start_editing_end(board, EditTarget::NoteBody(id))
            }
            (EditTarget::NoteBody(id), true) => {
                self.start_editing_end(board, EditTarget::NoteTitle(id))
            }
            _ => {}
        }
    }

    fn todo_of(target: EditTarget) -> Option<(u64, usize)> {
        match target {
            EditTarget::TodoText(id, r) | EditTarget::TodoBody(id, r) => Some((id, r)),
            _ => None,
        }
    }

    fn toggle_done_key(&mut self, board: &mut Board, target: EditTarget) {
        let Some((id, r)) = Self::todo_of(target) else {
            return;
        };
        if board.area(id).is_some_and(|a| a.can_toggle(r)) {
            self.history.record(board, None);
            if let Some(a) = board.area_mut(id) {
                a.toggle_done(r);
            }
            self.modified = true;
        }
    }

    fn priority_key(&mut self, board: &mut Board, target: EditTarget) {
        let Some((id, r)) = Self::todo_of(target) else {
            return;
        };
        self.history.record(board, None);
        if let Some(a) = board.area_mut(id) {
            a.cycle_priority(r);
            self.modified = true;
        }
    }

    /// Up/Down: within a multi-line field by wrapped line, otherwise between fields.
    fn vertical(&mut self, board: &mut Board, edit: Edit, up: bool, adv: Adv) {
        let target = edit.target;
        let caret = edit.caret;
        let style = style_of(board, target);
        let text: Vec<char> = text_of(board, target).unwrap_or("").chars().collect();

        if let Some(lines) = lines_of(board, target, adv) {
            let cur = line_of(&lines, caret);
            let x: f32 = text[lines[cur].start..caret.min(text.len())]
                .iter()
                .map(|&c| adv(style, c))
                .sum();
            let to = if up {
                cur.checked_sub(1)
            } else {
                Some(cur + 1).filter(|&n| n < lines.len())
            };
            if let Some(t) = to {
                let s: String = text.iter().collect();
                let line = lines[t];
                let seg: String = text[line.start..line.end].iter().collect();
                let col = index_at_x(&seg, x, &|c| adv(style, c));
                let _ = s;
                self.set_caret(line.start + col);
                return;
            }
        }
        // Between fields.
        let next = Self::neighbour(board, target, up);
        if let Some(n) = next {
            let len = text_of(board, n).map_or(0, |s| s.chars().count());
            self.start_editing(n, caret.min(len));
        }
    }

    fn neighbour(board: &Board, t: EditTarget, up: bool) -> Option<EditTarget> {
        let has_body = |id: u64, r: usize| {
            board
                .area(id)
                .and_then(|a| a.todos.get(r))
                .is_some_and(|t| t.body.is_some())
        };
        let n_todos = |id: u64| board.area(id).map_or(0, |a| a.todos.len());
        match (t, up) {
            (EditTarget::AreaTitle(id), false) => {
                (n_todos(id) > 0).then_some(EditTarget::TodoText(id, 0))
            }
            (EditTarget::TodoText(id, 0), true) => Some(EditTarget::AreaTitle(id)),
            (EditTarget::TodoText(id, r), true) => Some(if has_body(id, r - 1) {
                EditTarget::TodoBody(id, r - 1)
            } else {
                EditTarget::TodoText(id, r - 1)
            }),
            (EditTarget::TodoText(id, r), false) => {
                if has_body(id, r) {
                    Some(EditTarget::TodoBody(id, r))
                } else {
                    (r + 1 < n_todos(id)).then_some(EditTarget::TodoText(id, r + 1))
                }
            }
            (EditTarget::TodoBody(id, r), true) => Some(EditTarget::TodoText(id, r)),
            (EditTarget::TodoBody(id, r), false) => {
                (r + 1 < n_todos(id)).then_some(EditTarget::TodoText(id, r + 1))
            }
            (EditTarget::NoteTitle(id), false) => Some(EditTarget::NoteBody(id)),
            (EditTarget::NoteBody(id), true) => Some(EditTarget::NoteTitle(id)),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn caret_in_block(
    text: &str,
    lines: &[crate::text::Line],
    bx: f32,
    by: f32,
    line_h: f32,
    wx: f32,
    wy: f32,
    adv: &dyn Fn(char) -> f32,
) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let li = (((wy - by) / line_h).floor().max(0.0) as usize).min(lines.len().saturating_sub(1));
    let Some(line) = lines.get(li) else {
        return 0;
    };
    let seg: String = chars[line.start..line.end].iter().collect();
    line.start + index_at_x(&seg, wx - bx, adv)
}

/// True if any part of the polyline is within `r` of the point.
pub fn stroke_near(pts: &[(f32, f32)], x: f32, y: f32, r: f32) -> bool {
    match pts {
        [] => false,
        [p] => (p.0 - x).hypot(p.1 - y) <= r,
        _ => pts.windows(2).any(|w| seg_dist(w[0], w[1], x, y) <= r),
    }
}

fn seg_dist(a: (f32, f32), b: (f32, f32), x: f32, y: f32) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    (a.0 + t * dx - x).hypot(a.1 + t * dy - y)
}

/// Chaikin corner cutting (two passes), keeping the end points. A one-point stroke becomes a dot.
pub fn smooth(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if pts.len() < 2 {
        return pts.iter().chain(pts.iter()).copied().collect();
    }
    let mut cur = pts.to_vec();
    for _ in 0..2 {
        if cur.len() < 3 {
            break;
        }
        let mut next = Vec::with_capacity(cur.len() * 2);
        next.push(cur[0]);
        for w in cur.windows(2) {
            next.push((0.75 * w[0].0 + 0.25 * w[1].0, 0.75 * w[0].1 + 0.25 * w[1].1));
            next.push((0.25 * w[0].0 + 0.75 * w[1].0, 0.25 * w[0].1 + 0.75 * w[1].1));
        }
        next.push(*cur.last().unwrap());
        cur = next;
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::area_geom;

    fn adv(_: Style, _: char) -> f32 {
        7.0
    }
    const A: Adv = &adv;

    fn cam() -> Camera {
        Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }

    fn typed(e: &mut Editor, b: &mut Board, s: &str) {
        e.key(b, Key::Text(s.to_string()), A);
    }

    fn click(e: &mut Editor, b: &mut Board, p: (f32, f32), t: u32) {
        e.press(b, &cam(), p, Button::Left, t, A);
        e.release(b, A);
    }

    fn centre(r: crate::layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn setup() -> (Editor, Board) {
        let mut b = Board::default_board();
        // Park the Work area (index 2) at a known spot with two todos.
        b.areas[2].x = 100.0;
        b.areas[2].y = 100.0;
        b.areas[2].todos = vec![Todo::new("alpha"), Todo::new("beta")];
        (Editor::new(), b)
    }

    fn work(b: &Board) -> crate::layout::AreaGeom {
        area_geom(&b.areas[2], A)
    }

    fn texts(b: &Board) -> Vec<String> {
        b.areas[2].todos.iter().map(|t| t.text.clone()).collect()
    }

    // ---- areas

    #[test]
    fn double_click_empty_space_creates_a_group_and_renames_it() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        click(&mut e, &mut b, (500.0, 500.0), 1000);
        assert!(b.areas.is_empty(), "single click creates nothing");
        e.press(&mut b, &cam(), (501.0, 500.0), Button::Left, 1200, A);
        assert_eq!(b.areas.len(), 1);
        assert!(matches!(
            e.editing,
            Some(Edit {
                target: EditTarget::AreaTitle(_),
                ..
            })
        ));
        e.key(&mut b, Key::Home, A);
        for _ in 0..9 {
            e.key(&mut b, Key::Delete, A);
        }
        typed(&mut e, &mut b, "Reading");
        e.key(&mut b, Key::Enter, A);
        assert_eq!(b.areas[0].name, "Reading");
        assert!(e.editing.is_none());
    }

    #[test]
    fn double_click_title_renames() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).title);
        click(&mut e, &mut b, p, 100);
        assert!(e.editing.is_none(), "single click on a title does not edit");
        click(&mut e, &mut b, p, 300);
        assert!(matches!(
            e.editing,
            Some(Edit {
                target: EditTarget::AreaTitle(_),
                ..
            })
        ));
    }

    #[test]
    fn dragging_the_title_moves_the_group_and_is_one_undo_step() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).title);
        let mut c = cam();
        e.press(&mut b, &c, p, Button::Left, 0, A);
        e.motion(&mut b, &mut c, (p.0 + 60.0, p.1 + 40.0));
        e.motion(&mut b, &mut c, (p.0 + 120.0, p.1 + 80.0));
        e.release(&mut b, A);
        assert_eq!((b.areas[2].x, b.areas[2].y), (220.0, 180.0));
        assert!(e.undo(&mut b));
        assert_eq!((b.areas[2].x, b.areas[2].y), (100.0, 100.0));
        assert!(!e.undo(&mut b));
    }

    #[test]
    fn colour_dot_cycles_the_area_colour() {
        let (mut e, mut b) = setup();
        let before = b.areas[2].color;
        {
            let p = centre(work(&b).dot);
            click(&mut e, &mut b, p, 0);
        }
        assert_eq!(b.areas[2].color, (before + 1) % crate::board::PALETTE_LEN);
    }

    #[test]
    fn dragging_empty_space_pans_the_camera() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let mut c = cam();
        e.press(&mut b, &c, (400.0, 400.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (450.0, 430.0));
        e.release(&mut b, A);
        assert_eq!((c.x, c.y), (-50.0, -30.0), "content follows the pointer");
        assert!(e.take_view_changed() && !e.take_modified());
    }

    #[test]
    fn middle_drag_pans_even_over_a_card() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).rows[0].text);
        let mut c = cam();
        e.press(&mut b, &c, p, Button::Middle, 0, A);
        e.motion(&mut b, &mut c, (p.0 + 10.0, p.1));
        e.release(&mut b, A);
        assert_eq!(c.x, -10.0);
        assert!(e.editing.is_none());
    }

    #[test]
    fn right_click_removes_empty_group_only() {
        let (mut e, mut b) = setup();
        let n = b.areas.len();
        {
            let p = centre(work(&b).title);
            e.press(&mut b, &cam(), p, Button::Right, 0, A);
        }
        assert_eq!(b.areas.len(), n, "group with todos is kept");
        b.areas[2].todos.clear();
        {
            let p = centre(work(&b).title);
            e.press(&mut b, &cam(), p, Button::Right, 10, A);
        }
        assert_eq!(b.areas.len(), n - 1);
    }

    // ---- todos

    #[test]
    fn click_circle_toggles_done() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).rows[1].circle);
        click(&mut e, &mut b, p, 0);
        assert!(b.areas[2].todos[1].done && !b.areas[2].todos[0].done);
        assert!(e.editing.is_none());
        click(&mut e, &mut b, p, 5000);
        assert!(!b.areas[2].todos[1].done);
    }

    #[test]
    fn click_right_edge_cycles_priority() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).rows[0].prio);
        for expected in [1, 2, 3, 0] {
            click(&mut e, &mut b, p, 0);
            assert_eq!(b.areas[2].todos[0].priority, expected);
            e.last_click = None;
        }
    }

    #[test]
    fn click_row_edits_with_caret_at_click() {
        let (mut e, mut b) = setup();
        let r = work(&b).rows[0].clone();
        click(
            &mut e,
            &mut b,
            (r.text.x + 7.0 * 2.0 + 1.0, r.text.y + 10.0),
            0,
        );
        let ed = e.editing.unwrap();
        assert_eq!(ed.target, EditTarget::TodoText(b.areas[2].id, 0));
        assert_eq!(ed.caret, 2);
    }

    #[test]
    fn new_todo_row_adds_and_edits() {
        let (mut e, mut b) = setup();
        {
            let p = centre(work(&b).footer);
            click(&mut e, &mut b, p, 0);
        }
        assert_eq!(b.areas[2].todos.len(), 3);
        assert_eq!(
            e.editing.unwrap().target,
            EditTarget::TodoText(b.areas[2].id, 2)
        );
        typed(&mut e, &mut b, "gamma");
        assert_eq!(texts(&b), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn enter_adds_below_and_splits_at_caret() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 2);
        e.key(&mut b, Key::Enter, A);
        assert_eq!(texts(&b), ["al", "pha", "beta"]);
        assert_eq!(e.editing.unwrap().target, EditTarget::TodoText(id, 1));
        assert_eq!(e.editing.unwrap().caret, 0);
    }

    #[test]
    fn enter_on_empty_todo_discards_it() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        {
            let p = centre(work(&b).footer);
            click(&mut e, &mut b, p, 0);
        }
        assert_eq!(b.areas[2].todos.len(), 3);
        e.key(&mut b, Key::Enter, A);
        assert_eq!(b.areas[2].todos.len(), 2, "empty todo discarded");
        assert!(e.editing.is_none());
        let _ = id;
    }

    #[test]
    fn escape_on_a_fresh_empty_todo_discards_it() {
        let (mut e, mut b) = setup();
        {
            let p = centre(work(&b).footer);
            click(&mut e, &mut b, p, 0);
        }
        e.key(&mut b, Key::Escape, A);
        assert_eq!(b.areas[2].todos.len(), 2);
    }

    #[test]
    fn clicking_elsewhere_discards_an_empty_todo_and_reaims_the_click() {
        let (mut e, mut b) = setup();
        // New empty todo at the END; then click on row 1 ("beta") which sits above it: indices unchanged.
        {
            let p = centre(work(&b).footer);
            click(&mut e, &mut b, p, 0);
        }
        let beta = work(&b).rows[1].text;
        click(&mut e, &mut b, (beta.x + 3.0, beta.y + 10.0), 5000);
        assert_eq!(b.areas[2].todos.len(), 2, "empty todo removed");
        assert_eq!(
            e.editing.unwrap().target,
            EditTarget::TodoText(b.areas[2].id, 1)
        );
    }

    #[test]
    fn removing_an_empty_row_above_the_click_shifts_the_target() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        // Make row 0 empty and being edited; then click row 1 ("beta"): row 0 vanishes, beta becomes row 0.
        b.areas[2].todos[0].text.clear();
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        let beta = work(&b).rows[1].text;
        click(&mut e, &mut b, (beta.x + 3.0, beta.y + 10.0), 0);
        assert_eq!(texts(&b), ["beta"]);
        assert_eq!(
            e.editing.unwrap().target,
            EditTarget::TodoText(id, 0),
            "still editing beta"
        );
    }

    #[test]
    fn ctrl_enter_and_ctrl_p_while_editing() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.key(&mut b, Key::ToggleDone, A);
        assert!(b.areas[2].todos[0].done);
        e.key(&mut b, Key::CyclePriority, A);
        e.key(&mut b, Key::CyclePriority, A);
        assert_eq!(b.areas[2].todos[0].priority, 2);
    }

    #[test]
    fn hero_open_limit_blocks_enter_new_row_and_reopen() {
        let mut b = Board::default_board();
        let mut e = Editor::new();
        let id = b.areas[1].id;
        b.areas[1].todos = vec![Todo::new("a"), Todo::new("b"), Todo::new("c")];
        e.start_editing(EditTarget::TodoText(id, 2), 1);
        e.key(&mut b, Key::Enter, A);
        assert_eq!(b.areas[1].todos.len(), 3, "no 4th open todo");
        assert!(e.editing.is_none());
        let footer = area_geom(&b.areas[1], A).footer;
        let before = b.areas[1].todos.len();
        click(&mut e, &mut b, centre(footer), 0);
        assert_eq!(
            b.areas[1].todos.len(),
            before,
            "New todo is refused when Focus is full"
        );
        // done todos can't be re-opened while full
        b.areas[1].todos.push(Todo {
            text: "old".into(),
            done: true,
            priority: 0,
            body: None,
        });
        let circle = area_geom(&b.areas[1], A).rows[3].circle;
        click(&mut e, &mut b, centre(circle), 9000);
        assert!(b.areas[1].todos[3].done);
    }

    #[test]
    fn typing_editing_keys() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 5);
        typed(&mut e, &mut b, "!\nx");
        assert_eq!(
            texts(&b)[0],
            "alpha!x",
            "newlines are dropped in single-line fields"
        );
        e.key(&mut b, Key::Home, A);
        e.key(&mut b, Key::Delete, A);
        assert_eq!(texts(&b)[0], "lpha!x");
        e.key(&mut b, Key::End, A);
        e.key(&mut b, Key::Backspace, A);
        assert_eq!(texts(&b)[0], "lpha!");
        e.key(&mut b, Key::Left, A);
        e.key(&mut b, Key::Right, A);
        assert_eq!(e.editing.unwrap().caret, 5);
    }

    #[test]
    fn backspace_on_empty_todo_removes_it_and_goes_back() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        b.areas[2].todos[1].text.clear();
        e.start_editing(EditTarget::TodoText(id, 1), 0);
        e.key(&mut b, Key::Backspace, A);
        assert_eq!(texts(&b), ["alpha"]);
        assert_eq!(
            e.editing.unwrap(),
            Edit {
                target: EditTarget::TodoText(id, 0),
                caret: 5
            }
        );
    }

    #[test]
    fn up_down_walk_through_title_and_todos() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 1);
        e.key(&mut b, Key::Up, A);
        assert_eq!(e.editing.unwrap().target, EditTarget::AreaTitle(id));
        e.key(&mut b, Key::Down, A);
        e.key(&mut b, Key::Down, A);
        assert_eq!(e.editing.unwrap().target, EditTarget::TodoText(id, 1));
    }

    // ---- body (details)

    #[test]
    fn tab_opens_the_body_and_shift_tab_returns() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.key(&mut b, Key::Tab, A);
        assert_eq!(e.editing.unwrap().target, EditTarget::TodoBody(id, 0));
        assert_eq!(b.areas[2].todos[0].body.as_deref(), Some(""));
        typed(&mut e, &mut b, "first line");
        e.key(&mut b, Key::Enter, A);
        typed(&mut e, &mut b, "second");
        assert_eq!(
            b.areas[2].todos[0].body.as_deref(),
            Some("first line\nsecond"),
            "Enter in a body is a newline"
        );
        e.key(&mut b, Key::BackTab, A);
        assert_eq!(e.editing.unwrap().target, EditTarget::TodoText(id, 0));
        assert!(b.areas[2].todos[0].body.is_some());
    }

    #[test]
    fn empty_body_collapses_on_leave() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.key(&mut b, Key::Tab, A);
        e.key(&mut b, Key::Escape, A);
        assert_eq!(b.areas[2].todos[0].body, None);
        // and a todo with a body is not discarded even if its title is empty
        b.areas[2].todos[1].text.clear();
        b.areas[2].todos[1].body = Some("keep me".into());
        e.start_editing(EditTarget::TodoText(id, 1), 0);
        e.key(&mut b, Key::Escape, A);
        assert_eq!(b.areas[2].todos.len(), 2);
    }

    #[test]
    fn click_into_a_body_edits_it_and_up_down_move_between_lines() {
        let (mut e, mut b) = setup();
        b.areas[2].todos[0].body = Some("one\ntwo\nthree".into());
        let g = work(&b);
        let body = g.rows[0].body.clone().unwrap();
        // click on the second line, after 2 chars ("tw|o")
        click(
            &mut e,
            &mut b,
            (
                body.rect.x + 7.0 * 2.0 + 1.0,
                body.rect.y + crate::layout::BODY_LINE_H * 1.5,
            ),
            0,
        );
        let ed = e.editing.unwrap();
        assert!(matches!(ed.target, EditTarget::TodoBody(..)));
        assert_eq!(ed.caret, 4 + 2, "line 2 starts at char 4");
        e.key(&mut b, Key::Down, A);
        assert_eq!(e.editing.unwrap().caret, 8 + 2, "same column on line 3");
        e.key(&mut b, Key::Up, A);
        e.key(&mut b, Key::Up, A);
        assert_eq!(e.editing.unwrap().caret, 2);
        e.key(&mut b, Key::Up, A);
        assert!(
            matches!(e.editing.unwrap().target, EditTarget::TodoText(..)),
            "Up from the first body line goes to the title"
        );
    }

    #[test]
    fn backspace_in_empty_body_removes_it() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.key(&mut b, Key::Tab, A);
        e.key(&mut b, Key::Backspace, A);
        assert_eq!(b.areas[2].todos[0].body, None);
        assert_eq!(e.editing.unwrap().target, EditTarget::TodoText(id, 0));
    }

    // ---- notes and tools

    #[test]
    fn note_tool_places_one_note_then_returns_to_select() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Note);
        click(&mut e, &mut b, (300.0, 300.0), 0);
        assert_eq!(b.notes.len(), 1);
        assert_eq!(e.tool, Tool::Select);
        assert!(matches!(
            e.editing.unwrap().target,
            EditTarget::NoteTitle(_)
        ));
        typed(&mut e, &mut b, "Ideas");
        e.key(&mut b, Key::Enter, A);
        assert!(
            matches!(e.editing.unwrap().target, EditTarget::NoteBody(_)),
            "Enter in a note title moves to the body"
        );
        typed(&mut e, &mut b, "a");
        e.key(&mut b, Key::Enter, A);
        typed(&mut e, &mut b, "b");
        assert_eq!(
            (b.notes[0].title.as_str(), b.notes[0].body.as_str()),
            ("Ideas", "a\nb")
        );
        e.key(&mut b, Key::Escape, A);
        assert_eq!(b.notes.len(), 1);
    }

    #[test]
    fn empty_note_is_discarded() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Note);
        click(&mut e, &mut b, (300.0, 300.0), 0);
        e.key(&mut b, Key::Escape, A);
        assert!(b.notes.is_empty());
    }

    #[test]
    fn note_click_edits_and_drag_moves() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_note(200.0, 200.0);
        b.note_mut(id).unwrap().title = "T".into();
        b.note_mut(id).unwrap().body = "body text".into();
        let g = note_geom(b.note(id).unwrap(), A);
        let mut c = cam();
        let p = centre(g.title);
        e.press(&mut b, &c, p, Button::Left, 0, A);
        e.motion(&mut b, &mut c, (p.0 + 30.0, p.1 + 20.0));
        e.release(&mut b, A);
        assert_eq!((b.notes[0].x, b.notes[0].y), (230.0, 220.0));
        assert!(e.editing.is_none(), "a drag does not start editing");
        let g = note_geom(b.note(id).unwrap(), A);
        click(&mut e, &mut b, centre(g.title), 9000);
        assert!(
            matches!(e.editing.unwrap().target, EditTarget::NoteTitle(_)),
            "click without moving edits"
        );
    }

    #[test]
    fn note_dot_and_right_click() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_note(0.0, 0.0);
        b.note_mut(id).unwrap().title = "x".into();
        let g = note_geom(b.note(id).unwrap(), A);
        let c0 = b.notes[0].color;
        click(&mut e, &mut b, centre(g.dot), 0);
        assert_ne!(b.notes[0].color, c0);
        e.press(&mut b, &cam(), centre(g.body.rect), Button::Right, 10, A);
        assert!(b.notes.is_empty());
    }

    #[test]
    fn group_tool_only_places_on_empty_canvas() {
        let (mut e, mut b) = setup();
        let n = b.areas.len();
        e.set_tool(&mut b, Tool::Group);
        {
            let p = centre(work(&b).title);
            click(&mut e, &mut b, p, 0);
        }
        assert_eq!(
            b.areas.len(),
            n,
            "clicking a card with the Group tool does nothing"
        );
        assert_eq!(e.tool, Tool::Group, "and the tool stays armed");
        click(&mut e, &mut b, (-4000.0, -4000.0), 5000);
        assert_eq!(b.areas.len(), n + 1);
        assert_eq!(e.tool, Tool::Select);
    }

    #[test]
    fn todo_tool_adds_to_the_card_under_the_click_or_inbox() {
        let (mut e, mut b) = setup();
        e.set_tool(&mut b, Tool::Todo);
        {
            let p = centre(work(&b).title);
            click(&mut e, &mut b, p, 0);
        }
        assert_eq!(b.areas[2].todos.len(), 3);
        assert_eq!(e.tool, Tool::Select);
        e.key(&mut b, Key::Escape, A);
        e.set_tool(&mut b, Tool::Todo);
        click(&mut e, &mut b, (-4000.0, -4000.0), 5000);
        assert_eq!(b.areas[0].todos.len(), 1, "empty canvas captures to Inbox");
    }

    // ---- pen and eraser

    #[test]
    fn pen_draws_a_smoothed_stroke_dropping_near_duplicates() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Pen);
        let mut c = cam();
        e.press(&mut b, &c, (100.0, 100.0), Button::Left, 0, A);
        for i in 1..=10 {
            e.motion(
                &mut b,
                &mut c,
                (100.0 + i as f32 * 10.0, 100.0 + (i % 2) as f32 * 8.0),
            );
            e.motion(&mut b, &mut c, (100.0 + i as f32 * 10.0 + 0.5, 100.0)); // < 2px step: dropped
        }
        e.release(&mut b, A);
        assert_eq!(b.strokes.len(), 1);
        let pts = &b.strokes[0].pts;
        assert!(pts.len() > 11, "chaikin adds points: {}", pts.len());
        assert_eq!(pts[0], (100.0, 100.0), "start point kept");
        assert_eq!(e.tool, Tool::Pen, "pen stays active");
        assert!(e.undo(&mut b));
        assert!(b.strokes.is_empty(), "one stroke = one undo step");
    }

    #[test]
    fn pen_click_makes_a_dot() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Pen);
        click(&mut e, &mut b, (50.0, 50.0), 0);
        assert_eq!(b.strokes[0].pts.len(), 2);
        assert_eq!(b.strokes[0].pts[0], b.strokes[0].pts[1]);
    }

    #[test]
    fn eraser_removes_whole_strokes_it_touches() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        for y in [100.0, 300.0] {
            let id = b.add_stroke(0.0, y, 0);
            b.stroke_mut(id).unwrap().pts = vec![(0.0, y), (100.0, y), (200.0, y)];
        }
        e.set_tool(&mut b, Tool::Eraser);
        let mut c = cam();
        e.press(&mut b, &c, (50.0, 104.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (60.0, 200.0)); // moves between strokes: touches nothing new
        e.release(&mut b, A);
        assert_eq!(b.strokes.len(), 1);
        assert_eq!(b.strokes[0].pts[0].1, 300.0, "the far stroke survives");
        assert!(e.undo(&mut b));
        assert_eq!(b.strokes.len(), 2, "erase is undoable");
    }

    #[test]
    fn eraser_radius_scales_with_zoom() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_stroke(0.0, 0.0, 0);
        b.stroke_mut(id).unwrap().pts = vec![(0.0, 0.0), (100.0, 0.0)];
        e.set_tool(&mut b, Tool::Eraser);
        let zoomed = Camera {
            x: 0.0,
            y: 0.0,
            zoom: 4.0,
        };
        // 30 screen px away at zoom 4 = 7.5 world units: outside a 9px/4 = 2.25 radius
        e.press(&mut b, &zoomed, (50.0 * 4.0, 30.0), Button::Left, 0, A);
        e.release(&mut b, A);
        assert_eq!(b.strokes.len(), 1);
        e.press(&mut b, &zoomed, (50.0 * 4.0, 5.0), Button::Left, 0, A);
        assert!(b.strokes.is_empty());
    }

    #[test]
    fn smoothing_helpers() {
        assert_eq!(smooth(&[(1.0, 2.0)]), vec![(1.0, 2.0), (1.0, 2.0)]);
        let s = smooth(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
        assert_eq!(s[0], (0.0, 0.0));
        assert_eq!(*s.last().unwrap(), (10.0, 10.0));
        assert!(s.len() > 3);
        assert!(stroke_near(&[(0.0, 0.0), (10.0, 0.0)], 5.0, 2.0, 2.5));
        assert!(!stroke_near(&[(0.0, 0.0), (10.0, 0.0)], 5.0, 4.0, 2.5));
        assert!(!stroke_near(&[], 0.0, 0.0, 99.0));
    }

    // ---- misc

    #[test]
    fn modified_flag_tracks_content_changes_only() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.key(&mut b, Key::Left, A);
        assert!(!e.take_modified(), "caret movement is not content");
        typed(&mut e, &mut b, "x");
        assert!(e.take_modified());
        assert!(!e.take_modified());
    }

    #[test]
    fn typing_is_one_undo_step_and_undo_leaves_edit_mode() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 5);
        for ch in "abc".chars() {
            typed(&mut e, &mut b, &ch.to_string());
        }
        assert_eq!(texts(&b)[0], "alphaabc");
        assert!(e.undo(&mut b));
        assert_eq!(texts(&b)[0], "alpha");
        assert!(e.editing.is_none());
        assert!(e.redo(&mut b));
        assert_eq!(texts(&b)[0], "alphaabc");
    }

    #[test]
    fn scroll_keeps_the_caret_visible() {
        assert_eq!(scroll_x(200.0, 50.0), 0.0);
        assert!(scroll_x(200.0, 400.0) > 0.0);
        assert!(400.0 - scroll_x(200.0, 400.0) <= 200.0);
    }

    #[test]
    fn refused_reopen_leaves_no_phantom_undo_or_redo_step() {
        let mut b = Board::default_board();
        let mut e = Editor::new();
        b.areas[1].todos = vec![
            Todo::new("a"),
            Todo::new("b"),
            Todo::new("c"),
            Todo {
                text: "old".into(),
                done: true,
                priority: 0,
                body: None,
            },
        ];
        let circle = area_geom(&b.areas[1], A).rows[3].circle;
        click(&mut e, &mut b, centre(circle), 0); // refused: Focus is full
        assert!(b.areas[1].todos[3].done);
        assert!(!e.take_modified(), "a refused action changes nothing");
        assert!(!e.undo(&mut b), "and records nothing to undo");
    }
}

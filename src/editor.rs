//! Editor state machine: tools, selection-free direct manipulation, text editing, pen/eraser,
//! undo bookkeeping. Pure logic: takes decoded input and a char-advance function, so it can be
//! unit-tested without Wayland or fonts.

use crate::board::{Board, End, History, ShapeKind, Todo, DEFAULT_SHAPE};
use crate::board::{MAX_W, MIN_AREA_W, MIN_NOTE_W};
use crate::camera::Camera;
use crate::layout::{
    area_geom, hit_handle, hit_test, item_at, note_geom, text_geom, Adv, FmtBtn, Handle, Sel,
    Target, Tool,
};
use crate::text::{index_at_x, line_of, Style};

/// Pointer must travel this many screen px before a press on a card becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;
/// Max gap and distance for a double click.
const DOUBLE_CLICK_MS: u32 = 400;
const DOUBLE_CLICK_PX: f32 = 8.0;
/// Pen points closer than this many screen px to the previous one are dropped.
const PEN_MIN_STEP_PX: f32 = 2.0;
/// Resize handles can be grabbed within this many screen px.
pub const HANDLE_PX: f32 = 12.0;
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
    /// Ctrl+A
    SelectAll,
    /// Ctrl+X (the text goes to the clipboard via `Editor::selected_text`).
    Cut,
}

/// The text field being edited. Areas and notes are addressed by id, todos by (area id, row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    AreaTitle(u64),
    TodoText(u64, usize),
    TodoBody(u64, usize),
    NoteTitle(u64),
    NoteBody(u64),
    /// Free text on the canvas.
    FreeText(u64),
}

impl EditTarget {
    fn owner(self) -> u64 {
        match self {
            EditTarget::AreaTitle(i)
            | EditTarget::NoteTitle(i)
            | EditTarget::NoteBody(i)
            | EditTarget::FreeText(i) => i,
            EditTarget::TodoText(i, _) | EditTarget::TodoBody(i, _) => i,
        }
    }
    /// Distinct per field so typing in different fields never shares an undo step.
    fn key(self) -> usize {
        match self {
            EditTarget::AreaTitle(_) => 0,
            EditTarget::NoteTitle(_) => 1,
            EditTarget::NoteBody(_) => 2,
            EditTarget::FreeText(_) => 5_000_000,
            EditTarget::TodoText(_, r) => 3 + 2 * r,
            EditTarget::TodoBody(_, r) => 4 + 2 * r,
        }
    }
    pub fn multiline(self) -> bool {
        matches!(
            self,
            EditTarget::TodoBody(..) | EditTarget::NoteBody(_) | EditTarget::FreeText(_)
        )
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
    Text(u64),
    /// A shape, with its original end points so a move is exact however far the pointer went.
    Shape {
        id: u64,
        orig: (f32, f32, f32, f32),
    },
    /// A connector's free ends (attached ends follow their items and stay put).
    Connector {
        id: u64,
        orig: (f32, f32, f32, f32),
    },
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    None,
    Pan {
        last: (f32, f32),
    },
    /// Rubber-band selection on empty canvas: world start and current corner, plus the screen
    /// start (a tiny drag is just a click that deselects).
    Marquee {
        start: (f32, f32),
        cur: (f32, f32),
        screen: (f32, f32),
        moved: bool,
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
    /// Dragging out a new shape.
    DrawShape {
        id: u64,
    },
    /// Dragging out a new connector arrow; its end follows the pointer.
    Connect {
        id: u64,
    },
    /// Dragging a resize handle of the selected item.
    Resize {
        sel: Sel,
        handle: Handle,
        /// (x, width, 0, 0) for cards and notes; (x1, y1, x2, y2) for shapes.
        orig: (f32, f32, f32, f32),
        recorded: bool,
    },
    Erase {
        recorded: bool,
    },
    /// Dragging the pointer over the text being edited to select it.
    TextSelect {
        target: EditTarget,
    },
}

/// What a todo/note removal shifted, so a click aimed at the old layout can be re-aimed.
#[derive(Debug, Clone, Copy)]
enum Removed {
    Todo { area: usize, row: usize },
    Note { index: usize },
    Text { index: usize },
}

#[derive(Debug)]
pub struct Editor {
    pub tool: Tool,
    /// The selected item (shows resize handles; Delete removes it).
    pub selected: Option<Sel>,
    /// Extra items added with Shift+click, to be locked together with `selected` (Ctrl+G).
    pub extra: Vec<Sel>,
    /// Shift is held (set by the app).
    pub shift: bool,
    /// Space is held outside text editing (set by the app): a left drag pans from anywhere.
    pub hand: bool,
    /// Other end of the text selection (the caret is the moving end). None: nothing selected.
    anchor: Option<usize>,
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
        EditTarget::FreeText(id) => board.text(id).map(|t| t.text.as_str()),
    }
}

fn text_mut(board: &mut Board, t: EditTarget) -> Option<&mut String> {
    match t {
        EditTarget::AreaTitle(id) => board.area_mut(id).map(|a| &mut a.name),
        EditTarget::TodoText(id, r) => board.area_mut(id)?.todos.get_mut(r).map(|t| &mut t.text),
        EditTarget::TodoBody(id, r) => board.area_mut(id)?.todos.get_mut(r)?.body.as_mut(),
        EditTarget::NoteTitle(id) => board.note_mut(id).map(|n| &mut n.title),
        EditTarget::NoteBody(id) => board.note_mut(id).map(|n| &mut n.body),
        EditTarget::FreeText(id) => board.text_mut(id).map(|t| &mut t.text),
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
        EditTarget::FreeText(id) => board
            .text(id)
            .map_or(Style::Free(Default::default(), false), |t| t.style()),
    }
}

/// Where a field is on the board (world units).
pub fn field_rect(board: &Board, t: EditTarget, adv: Adv) -> Option<crate::layout::Rect> {
    Some(match t {
        EditTarget::AreaTitle(id) => area_geom(board.area(id)?, adv).title,
        EditTarget::TodoText(id, r) => area_geom(board.area(id)?, adv).rows.get(r)?.text,
        EditTarget::TodoBody(id, r) => {
            area_geom(board.area(id)?, adv)
                .rows
                .get(r)?
                .body
                .as_ref()?
                .rect
        }
        EditTarget::NoteTitle(id) => note_geom(board.note(id)?, adv).title,
        EditTarget::NoteBody(id) => note_geom(board.note(id)?, adv).body.rect,
        EditTarget::FreeText(id) => text_geom(board.text(id)?, adv).rect,
    })
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
        EditTarget::FreeText(id) => Some(text_geom(board.text(id)?, adv).lines),
        _ => None,
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            tool: Tool::Select,
            selected: None,
            extra: Vec::new(),
            shift: false,
            hand: false,
            anchor: None,
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

    /// Record that something outside the editor (the todo detail page) changed the board.
    pub fn board_changed_externally(&mut self, board: &Board) {
        self.history.record(board, None);
        self.modified = true;
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
        self.anchor = None;
    }

    /// Start (or continue) editing at a click and arm drag-selection from that point.
    fn begin_text_select(&mut self, target: EditTarget, caret: usize) {
        self.start_editing(target, caret);
        self.anchor = Some(caret);
        self.drag = Drag::TextSelect { target };
    }

    /// The selected range of the edited field, as char indices (start < end).
    pub fn selection(&self) -> Option<(usize, usize)> {
        let e = self.editing?;
        let a = self.anchor?;
        (a != e.caret).then(|| (a.min(e.caret), a.max(e.caret)))
    }

    /// The selected text, for copy / cut.
    pub fn selected_text(&self, board: &Board) -> Option<String> {
        let (s, e) = self.selection()?;
        let t = text_of(board, self.editing?.target)?;
        Some(t.chars().skip(s).take(e - s).collect())
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
            EditTarget::FreeText(id) => {
                let empty = board.text(id).is_some_and(|t| t.text.is_empty());
                if empty {
                    self.history.record(board, None);
                    let ti = board.texts.iter().position(|t| t.id == id)?;
                    board.texts.remove(ti);
                    self.modified = true;
                    return Some(Removed::Text { index: ti });
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
            (Target::FreeText(n), Removed::Text { index }) => {
                if n == index {
                    None
                } else {
                    Some(Target::FreeText(if n > index { n - 1 } else { n }))
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
    /// Drop a sticker (the next emoji in the set) centred on a world point and select it.
    pub fn add_sticker(&mut self, board: &mut Board, (wx, wy): (f32, f32)) -> u64 {
        self.history.record(board, None);
        let n = board
            .shapes
            .iter()
            .filter(|s| s.kind == crate::board::ShapeKind::Sticker)
            .count();
        let e = crate::board::STICKERS[n % crate::board::STICKERS.len()];
        let side = 96.0;
        let id = board.add_picture(
            crate::board::ShapeKind::Sticker,
            e,
            wx - side / 2.0,
            wy - side / 2.0,
            side,
            side,
        );
        self.selected = Some(Sel::Shape(id));
        self.modified = true;
        id
    }

    /// Place a stored image (`name` in the images folder, `iw` x `ih` px) centred on a world point.
    pub fn add_image(
        &mut self,
        board: &mut Board,
        name: &str,
        (iw, ih): (u32, u32),
        (wx, wy): (f32, f32),
    ) -> u64 {
        self.history.record(board, None);
        let k = (360.0 / iw.max(ih).max(1) as f32).min(1.0);
        let (w, h) = ((iw as f32 * k).max(16.0), (ih as f32 * k).max(16.0));
        let id = board.add_picture(
            crate::board::ShapeKind::Image,
            name,
            wx - w / 2.0,
            wy - h / 2.0,
            w,
            h,
        );
        self.selected = Some(Sel::Shape(id));
        self.modified = true;
        id
    }

    pub fn create_note_at(&mut self, board: &mut Board, wx: f32, wy: f32) -> u64 {
        self.history.record(board, None);
        let id = board.add_note(wx, wy);
        self.start_editing(EditTarget::NoteTitle(id), 0);
        self.modified = true;
        id
    }

    /// Append an empty todo to area `ai` and edit it. False if the hero area is full.
    /// Save the view as a space (undoable). Returns its id.
    pub fn add_space(&mut self, board: &mut Board, name: &str, c: (f32, f32), zoom: f32) -> u64 {
        self.history.record(board, None);
        self.modified = true;
        board.add_space(name, c.0, c.1, zoom)
    }

    pub fn rename_space(&mut self, board: &mut Board, id: u64, name: &str) -> bool {
        if board.space(id).is_none_or(|s| s.name == name) {
            return false;
        }
        self.history.record(board, None);
        if let Some(s) = board.space_mut(id) {
            s.name = name.to_string();
        }
        self.modified = true;
        true
    }

    pub fn delete_space(&mut self, board: &mut Board, id: u64) -> bool {
        if board.space(id).is_none() {
            return false;
        }
        self.history.record(board, None);
        board.spaces.retain(|s| s.id != id);
        self.modified = true;
        true
    }

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

    /// New free text at a world point; typing starts immediately.
    pub fn create_text_at(&mut self, board: &mut Board, wx: f32, wy: f32) -> u64 {
        self.history.record(board, None);
        let id = board.add_text(wx, wy);
        self.start_editing(EditTarget::FreeText(id), 0);
        self.modified = true;
        id
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
            Button::Left if self.hand && self.editing.is_none() => {
                // Whatever is under the pointer is left alone, whichever tool is armed.
                self.last_click = None;
                self.drag = Drag::Pan { last: p };
                true
            }
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
            Target::FreeText(ti) => {
                if ti < board.texts.len() {
                    self.history.record(board, None);
                    board.texts.remove(ti);
                    self.modified = true;
                }
            }
            Target::Shape(si) => {
                if si < board.shapes.len() {
                    self.history.record(board, None);
                    board.shapes.remove(si);
                    self.modified = true;
                }
            }
            Target::Connector(ci) => {
                if ci < board.connectors.len() {
                    self.history.record(board, None);
                    board.connectors.remove(ci);
                    self.modified = true;
                }
            }
        }
        board.prune_connectors();
        self.validate_selection(board);
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
            Tool::Rect | Tool::Ellipse | Tool::Line => {
                self.finish_edit(board);
                self.history.record(board, None);
                let kind = self.tool.shape_kind().unwrap_or(ShapeKind::Rect);
                let id = board.add_shape(kind, wx, wy);
                self.drag = Drag::DrawShape { id };
                self.modified = true;
                true
            }
            Tool::Connector => {
                self.finish_edit(board);
                self.history.record(board, None);
                // Start on an item to attach to it (it then follows that item), else a free point.
                let from = match item_at(board, wx, wy, adv) {
                    Some(id) => End { id, x: wx, y: wy },
                    None => End::free(wx, wy),
                };
                let id = board.add_connector(from, End::free(wx, wy), self.pen_color);
                self.drag = Drag::Connect { id };
                self.modified = true;
                true
            }
            Tool::Group | Tool::Note | Tool::Todo | Tool::Text | Tool::Sticker => {
                self.place_press(board, wx, wy, adv)
            }
            Tool::Select | Tool::Image => {
                self.select_press(board, p, (wx, wy), double, cam.zoom, adv)
            }
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
            Tool::Sticker if tgt.is_none() => {
                self.add_sticker(board, (wx, wy));
            }
            Tool::Text => match tgt {
                None => {
                    self.create_text_at(board, wx, wy);
                }
                Some(Target::FreeText(ti)) => {
                    if let Some(id) = board.texts.get(ti).map(|t| t.id) {
                        self.start_editing_end(board, EditTarget::FreeText(id));
                    }
                }
                _ => return true, // a Text click on a card does nothing; keep the tool
            },
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
        zoom: f32,
        adv: Adv,
    ) -> bool {
        // A grab on a resize handle of the selected item wins over everything else.
        if let Some(sel) = self.selected {
            if let Some(handle) = hit_handle(board, sel, wx, wy, HANDLE_PX / zoom, adv) {
                self.finish_edit(board);
                if let Some(orig) = Self::resize_orig(board, sel) {
                    self.drag = Drag::Resize {
                        sel,
                        handle,
                        orig,
                        recorded: false,
                    };
                    return true;
                }
            }
        }
        let tgt = hit_test(board, wx, wy, adv);
        // Shift+click builds a multi-selection for grouping.
        if self.shift {
            if let Some(sel) = tgt.and_then(|t| Self::sel_of(board, t)) {
                self.finish_edit(board);
                if let Some(cur) = self.selected {
                    if cur == sel {
                        self.selected = self.extra.pop();
                    } else if let Some(i) = self.extra.iter().position(|e| *e == sel) {
                        self.extra.remove(i);
                    } else {
                        self.extra.push(sel);
                    }
                } else {
                    self.selected = Some(sel);
                }
                return true;
            }
        }
        if !self.extra.is_empty() {
            self.extra.clear();
        }
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
                self.selected = None;
                return self.empty_press(board, p, (wx, wy), double);
            }
            return true;
        };
        self.selected = Self::sel_of(board, tgt);

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
                    self.begin_text_select(EditTarget::AreaTitle(id), caret);
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
                    self.begin_text_select(t, caret);
                }
            }
            Target::FreeText(ti) => {
                let Some(id) = board.texts.get(ti).map(|t| t.id) else {
                    return true;
                };
                if keep_edit {
                    let t = EditTarget::FreeText(id);
                    let caret = self.caret_for(board, t, (wx, wy), adv);
                    self.begin_text_select(t, caret);
                } else if let Some(t) = board.text(id) {
                    self.drag = Drag::Pending {
                        who: Mover::Text(id),
                        start: p,
                        off: (wx - t.x, wy - t.y),
                    };
                }
            }
            Target::Shape(si) => {
                let Some(s) = board.shapes.get(si) else {
                    return true;
                };
                if double {
                    let id = s.id;
                    self.history.record(board, None);
                    if let Some(s) = board.shape_mut(id) {
                        if s.kind == crate::board::ShapeKind::Sticker {
                            // Double-click a sticker: the next emoji.
                            let st = crate::board::STICKERS;
                            let i = st.iter().position(|e| *e == s.src).map_or(0, |i| i + 1);
                            s.src = st[i % st.len()].to_string();
                        } else {
                            s.color = (s.color + 1) % crate::board::PALETTE_LEN;
                        }
                    }
                    self.modified = true;
                } else {
                    self.drag = Drag::Pending {
                        who: Mover::Shape {
                            id: s.id,
                            orig: (s.x1, s.y1, s.x2, s.y2),
                        },
                        start: p,
                        off: (wx, wy),
                    };
                }
            }
            Target::Connector(ci) => {
                let Some(c) = board.connectors.get(ci) else {
                    return true;
                };
                let (id, orig) = (c.id, (c.from.x, c.from.y, c.to.x, c.to.y));
                let free = c.from.id == 0 || c.to.id == 0;
                if double {
                    // Double-click recolours.
                    self.history.record(board, None);
                    if let Some(c) = board.connector_mut(id) {
                        c.color = (c.color + 1) % crate::board::PALETTE_LEN;
                    }
                    self.modified = true;
                } else if free {
                    self.drag = Drag::Pending {
                        who: Mover::Connector { id, orig },
                        start: p,
                        off: (wx, wy),
                    };
                }
            }
            Target::Body(_) => self.drag = Drag::Pan { last: p },
        }
        true
    }

    /// The selection a hit puts the item under the pointer into.
    fn sel_of(board: &Board, t: Target) -> Option<Sel> {
        Some(match t {
            Target::Title(a)
            | Target::Dot(a)
            | Target::NewTodo(a)
            | Target::Body(a)
            | Target::Circle(a, _)
            | Target::Text(a, _)
            | Target::Prio(a, _)
            | Target::TodoBody(a, _) => Sel::Area(board.areas.get(a)?.id),
            Target::NoteTitle(n) | Target::NoteBody(n) | Target::NoteDot(n) => {
                Sel::Note(board.notes.get(n)?.id)
            }
            Target::FreeText(i) => Sel::Text(board.texts.get(i)?.id),
            Target::Shape(i) => Sel::Shape(board.shapes.get(i)?.id),
            Target::Connector(i) => Sel::Connector(board.connectors.get(i)?.id),
        })
    }

    /// The starting geometry a resize drag works from.
    fn resize_orig(board: &Board, sel: Sel) -> Option<(f32, f32, f32, f32)> {
        Some(match sel {
            Sel::Area(id) => {
                let a = board.area(id)?;
                (a.x, a.width(), 0.0, 0.0)
            }
            Sel::Note(id) => {
                let n = board.note(id)?;
                (n.x, n.width(), 0.0, 0.0)
            }
            Sel::Shape(id) => {
                let s = board.shape(id)?;
                (s.x1, s.y1, s.x2, s.y2)
            }
            Sel::Text(_) => return None,
            Sel::Connector(_) => (0.0, 0.0, 0.0, 0.0),
        })
    }

    /// Drop the selection if its item no longer exists.
    fn validate_selection(&mut self, board: &Board) {
        let alive = match self.selected {
            None => true,
            Some(Sel::Area(id)) => board.area(id).is_some(),
            Some(Sel::Note(id)) => board.note(id).is_some(),
            Some(Sel::Shape(id)) => board.shape(id).is_some(),
            Some(Sel::Text(id)) => board.text(id).is_some(),
            Some(Sel::Connector(id)) => board.connectors.iter().any(|c| c.id == id),
        };
        if !alive {
            self.selected = None;
        }
    }

    /// Escape with nothing being edited: drop the selection. True if something changed.
    pub fn deselect(&mut self) -> bool {
        let had = !self.extra.is_empty();
        self.extra.clear();
        self.selected.take().is_some() || had
    }

    /// Ctrl+G: lock the multi-selection (or the selected item's group) together.
    pub fn group_selected(&mut self, board: &mut Board) -> bool {
        let Some(first) = self.selected else {
            return false;
        };
        if self.extra.is_empty() {
            return false;
        }
        let mut items = vec![first];
        items.extend(self.extra.iter().copied());
        self.history.record(board, None);
        board.group_items(&items);
        self.extra.clear();
        self.modified = true;
        true
    }

    /// Ctrl+Shift+G: release the selected item's group.
    pub fn ungroup_selected(&mut self, board: &mut Board) -> bool {
        let Some(first) = self.selected else {
            return false;
        };
        if board.group_of(first) == 0 {
            return false;
        }
        self.history.record(board, None);
        board.ungroup(first);
        self.modified = true;
        true
    }

    /// Delete key: remove the selected items (a card only if it is empty, like right-click).
    pub fn delete_selected(&mut self, board: &mut Board) -> bool {
        let Some(first) = self.selected else {
            return false;
        };
        let mut items = vec![first];
        items.extend(self.extra.iter().copied());
        let removable = |sel: &Sel| match *sel {
            Sel::Area(id) => board.area(id).is_some_and(|a| a.todos.is_empty()),
            _ => true,
        };
        let doomed: Vec<Sel> = items.iter().copied().filter(removable).collect();
        if doomed.is_empty() {
            return false;
        }
        self.history.record(board, None);
        for sel in &doomed {
            match *sel {
                Sel::Area(id) => board.areas.retain(|a| a.id != id),
                Sel::Note(id) => board.notes.retain(|n| n.id != id),
                Sel::Shape(id) => board.shapes.retain(|s| s.id != id),
                Sel::Text(id) => board.texts.retain(|t| t.id != id),
                Sel::Connector(id) => board.connectors.retain(|c| c.id != id),
            }
        }
        board.prune_connectors();
        // Whatever could not be removed (a card that still has todos) stays selected.
        let mut kept = items.into_iter().filter(|s| !doomed.contains(s));
        self.selected = kept.next();
        self.extra = kept.collect();
        self.modified = true;
        true
    }

    /// Apply a handle drag: cards and notes change width (height follows content), shapes move
    /// the grabbed corner or end point.
    fn apply_resize(
        &mut self,
        board: &mut Board,
        sel: Sel,
        handle: Handle,
        orig: (f32, f32, f32, f32),
        (wx, wy): (f32, f32),
    ) -> bool {
        match sel {
            Sel::Area(_) | Sel::Note(_) => {
                let (ox, ow) = (orig.0, orig.1);
                let (min, id_area) = match sel {
                    Sel::Area(_) => (MIN_AREA_W, true),
                    _ => (MIN_NOTE_W, false),
                };
                // The opposite edge stays put.
                let (left, right) = (ox, ox + ow);
                let (nl, nr) = if handle.is_left() {
                    (wx.min(right - min), right)
                } else {
                    (left, wx.max(left + min))
                };
                let w = (nr - nl).clamp(min, MAX_W);
                let x = if handle.is_left() { right - w } else { left };
                match sel {
                    Sel::Area(id) if id_area => {
                        if let Some(a) = board.area_mut(id) {
                            (a.x, a.w) = (x, w);
                        }
                    }
                    Sel::Note(id) => {
                        if let Some(n) = board.note_mut(id) {
                            (n.x, n.w) = (x, w);
                        }
                    }
                    _ => {}
                }
            }
            Sel::Connector(id) => {
                let Some(c) = board.connector_mut(id) else {
                    return false;
                };
                match handle {
                    Handle::End1 => c.from = End::free(wx, wy),
                    _ => c.to = End::free(wx, wy),
                }
                self.modified = true;
                return true;
            }
            Sel::Shape(id) => {
                let Some(s) = board.shape_mut(id) else {
                    return false;
                };
                let (x1, y1, x2, y2) = orig;
                match handle {
                    Handle::End1 => (s.x1, s.y1) = (wx, wy),
                    Handle::End2 => (s.x2, s.y2) = (wx, wy),
                    corner => {
                        let (lx, rx) = (x1.min(x2), x1.max(x2));
                        let (ty, by) = (y1.min(y2), y1.max(y2));
                        // The corner opposite the grabbed one stays fixed.
                        let (fx, fy) = match corner {
                            Handle::TopLeft => (rx, by),
                            Handle::TopRight => (lx, by),
                            Handle::BottomLeft => (rx, ty),
                            _ => (lx, ty),
                        };
                        (s.x1, s.y1, s.x2, s.y2) = (fx.min(wx), fy.min(wy), fx.max(wx), fy.max(wy));
                    }
                }
            }
            Sel::Text(_) => return false,
        }
        self.modified = true;
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
            self.drag = Drag::Marquee {
                start: (wx, wy),
                cur: (wx, wy),
                screen: p,
                moved: false,
            };
        }
        true
    }

    /// The rubber-band rectangle being dragged, in world coordinates (x, y, w, h).
    pub fn marquee_rect(&self) -> Option<(f32, f32, f32, f32)> {
        match self.drag {
            Drag::Marquee {
                start,
                cur,
                moved: true,
                ..
            } => Some((
                start.0.min(cur.0),
                start.1.min(cur.1),
                (start.0 - cur.0).abs(),
                (start.1 - cur.1).abs(),
            )),
            _ => None,
        }
    }

    /// The field a hit would edit, if it is a text target.
    fn edit_target_of(board: &Board, t: Target) -> Option<EditTarget> {
        Some(match t {
            Target::Title(ai) => EditTarget::AreaTitle(board.areas.get(ai)?.id),
            Target::Text(ai, ri) => EditTarget::TodoText(board.areas.get(ai)?.id, ri),
            Target::TodoBody(ai, ri) => EditTarget::TodoBody(board.areas.get(ai)?.id, ri),
            Target::NoteTitle(ni) => EditTarget::NoteTitle(board.notes.get(ni)?.id),
            Target::NoteBody(ni) => EditTarget::NoteBody(board.notes.get(ni)?.id),
            Target::FreeText(ti) => EditTarget::FreeText(board.texts.get(ti)?.id),
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
            EditTarget::FreeText(id) => {
                let Some(t) = board.text(id) else {
                    return 0;
                };
                let g = text_geom(t, adv);
                caret_in_block(text, &g.lines, g.rect.x, g.rect.y, g.line_h, wx, wy, &a)
            }
        }
    }

    /// The free text the format bar and grip belong to: the one being typed, else the only
    /// selected item if it is a text.
    pub fn text_focus(&self) -> Option<u64> {
        if let Some(e) = self.editing {
            return match e.target {
                EditTarget::FreeText(id) => Some(id),
                _ => None,
            };
        }
        match self.selected {
            Some(Sel::Text(id)) if self.extra.is_empty() => Some(id),
            _ => None,
        }
    }

    /// Apply a format bar button to text `id`. One undo step; true if anything changed.
    pub fn format_text(&mut self, board: &mut Board, id: u64, btn: FmtBtn) -> bool {
        let Some(cur) = board.text(id).cloned() else {
            return false;
        };
        let mut t = cur.clone();
        match btn {
            FmtBtn::Bold => t.bold = !t.bold,
            FmtBtn::Italic => t.italic = !t.italic,
            FmtBtn::Underline => t.underline = !t.underline,
            FmtBtn::Strike => t.strike = !t.strike,
            FmtBtn::Size(sz) => t.size = sz,
            FmtBtn::Color(c) => t.color = c % crate::board::PALETTE_LEN,
            FmtBtn::Markdown => t.plain = !t.plain,
            FmtBtn::Code | FmtBtn::Heading | FmtBtn::Bullet | FmtBtn::Task => return false,
        }
        if t == cur {
            return false;
        }
        self.history.record(board, None);
        if let Some(dst) = board.text_mut(id) {
            *dst = t;
        }
        self.modified = true;
        true
    }

    /// Apply a Markdown edit to the field being typed in (its selection, else the word or line
    /// at the caret), or to the whole of a selected text box. One undo step; true if it changed.
    pub fn format_md(&mut self, board: &mut Board, op: crate::md::MdOp) -> bool {
        let (target, sel, caret) = match self.editing {
            Some(e) => (e.target, self.selection(), e.caret),
            None => {
                let Some(id) = self.text_focus() else {
                    return false;
                };
                let len = board.text(id).map_or(0, |t| t.text.chars().count());
                (EditTarget::FreeText(id), Some((0, len)), len)
            }
        };
        let Some(text) = text_of(board, target) else {
            return false;
        };
        let Some((t, nsel, ncaret)) = crate::md::apply(text, sel, caret, op, target.multiline())
        else {
            return false;
        };
        self.history.record(board, None);
        if let Some(dst) = text_mut(board, target) {
            *dst = t;
        }
        self.modified = true;
        if self.editing.is_some() {
            self.anchor = nsel.map(|(a, _)| a);
            self.set_caret(ncaret);
        }
        true
    }

    /// A format bar button or shortcut. Bold / italic / strike on a free text box style the whole
    /// box unless text in it is selected; everywhere else (and for code, headings and lists) they
    /// edit Markdown markers around the selection or the word / line at the caret.
    pub fn apply_fmt(&mut self, board: &mut Board, btn: FmtBtn) -> bool {
        use crate::md::{LineKind, MdOp};
        let op = match btn {
            FmtBtn::Bold => Some(MdOp::Wrap("**")),
            FmtBtn::Italic => Some(MdOp::Wrap("*")),
            FmtBtn::Strike => Some(MdOp::Wrap("~~")),
            FmtBtn::Code => Some(MdOp::Wrap("`")),
            FmtBtn::Heading => Some(MdOp::Line(LineKind::Heading)),
            FmtBtn::Bullet => Some(MdOp::Line(LineKind::Bullet)),
            FmtBtn::Task => Some(MdOp::Line(LineKind::Task)),
            _ => None,
        };
        let free = self.text_focus();
        let whole_box = matches!(btn, FmtBtn::Bold | FmtBtn::Italic | FmtBtn::Strike)
            && free.is_some()
            && self.selection().is_none();
        match (op, free) {
            (Some(op), _) if !whole_box => self.format_md(board, op),
            (_, Some(id)) => self.format_text(board, id, btn),
            _ => false,
        }
    }

    /// Step the size of text `id` up or down (Ctrl+] / Ctrl+[).
    pub fn step_text_size(&mut self, board: &mut Board, id: u64, up: bool) -> bool {
        let Some(sz) = board.text(id).map(|t| t.size.step(up)) else {
            return false;
        };
        self.format_text(board, id, FmtBtn::Size(sz))
    }

    /// Press on the grip of text `id`: dragging moves it (typing goes on if it was being typed).
    pub fn grab_text(&mut self, board: &Board, cam: &Camera, p: (f32, f32), id: u64) -> bool {
        let Some(t) = board.text(id) else {
            return false;
        };
        let (wx, wy) = cam.screen_to_world(p.0, p.1);
        self.anchor = None;
        self.drag = Drag::Pending {
            who: Mover::Text(id),
            start: p,
            off: (wx - t.x, wy - t.y),
        };
        if self.editing.is_none() {
            self.selected = Some(Sel::Text(id));
            self.extra.clear();
        }
        true
    }

    /// Paste a text box at (wx, wy): a copy of `tmpl` (text and style) if given, else plain
    /// `text`. Selects the new box. Returns its id.
    pub fn paste_text_box(
        &mut self,
        board: &mut Board,
        tmpl: Option<&crate::board::TextBox>,
        text: &str,
        (wx, wy): (f32, f32),
    ) -> u64 {
        self.finish_edit(board);
        self.history.record(board, None);
        let (mut x, mut y) = (wx, wy);
        // Pasting twice at the same spot staggers the copies instead of stacking them.
        while board
            .texts
            .iter()
            .any(|t| (t.x - x).abs() < 1.0 && (t.y - y).abs() < 1.0)
        {
            (x, y) = (x + 24.0, y + 24.0);
        }
        let id = match tmpl {
            Some(t) => board.add_text_like(t, x, y),
            None => {
                let id = board.add_text(x, y);
                if let Some(t) = board.text_mut(id) {
                    t.text = text.to_string();
                }
                id
            }
        };
        self.tool = Tool::Select;
        self.selected = Some(Sel::Text(id));
        self.extra.clear();
        self.modified = true;
        id
    }

    /// Pointer moved while drag-selecting text: the caret follows it. True if it moved.
    pub fn drag_text(&mut self, board: &Board, cam: &Camera, p: (f32, f32), adv: Adv) -> bool {
        let Drag::TextSelect { target } = self.drag else {
            return false;
        };
        let w = cam.screen_to_world(p.0, p.1);
        let caret = self.caret_for(board, target, w, adv);
        let changed = self.editing.is_some_and(|e| e.caret != caret);
        self.set_caret(caret);
        changed
    }

    /// Pointer moved. The camera is moved here when panning.
    pub fn motion(&mut self, board: &mut Board, cam: &mut Camera, p: (f32, f32)) -> bool {
        let (wx, wy) = cam.screen_to_world(p.0, p.1);
        match self.drag {
            Drag::None | Drag::TextSelect { .. } => false,
            Drag::Pan { last } => {
                cam.pan_by_screen(p.0 - last.0, p.1 - last.1);
                self.drag = Drag::Pan { last: p };
                self.view_changed = true;
                true
            }
            Drag::Marquee {
                start,
                screen,
                moved,
                ..
            } => {
                let moved = moved || (p.0 - screen.0).hypot(p.1 - screen.1) >= DRAG_THRESHOLD;
                self.drag = Drag::Marquee {
                    start,
                    cur: (wx, wy),
                    screen,
                    moved,
                };
                moved
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
            Drag::Connect { id } => match board.connector_mut(id) {
                Some(c) => {
                    c.to = End::free(wx, wy);
                    self.modified = true;
                    true
                }
                None => false,
            },
            Drag::DrawShape { id } => match board.shape_mut(id) {
                Some(s) => {
                    (s.x2, s.y2) = (wx, wy);
                    self.modified = true;
                    true
                }
                None => false,
            },
            Drag::Resize {
                sel,
                handle,
                orig,
                recorded,
            } => {
                if !recorded {
                    self.history.record(board, None);
                    self.drag = Drag::Resize {
                        sel,
                        handle,
                        orig,
                        recorded: true,
                    };
                }
                self.apply_resize(board, sel, handle, orig, (wx, wy))
            }
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
        let sel = match who {
            Mover::Area(id) => Sel::Area(id),
            Mover::Note(id) => Sel::Note(id),
            Mover::Text(id) => Sel::Text(id),
            Mover::Shape { id, .. } => Sel::Shape(id),
            Mover::Connector { id, .. } => Sel::Connector(id),
        };
        let before = Self::origin_of(board, sel);
        let moved = self.apply_move_one(board, who, off, wx, wy);
        if let (true, Some(b), Some(a)) = (moved, before, Self::origin_of(board, sel)) {
            let g = board.group_of(sel);
            for m in board.group_members(g) {
                if m != sel {
                    board.translate(m, a.0 - b.0, a.1 - b.1);
                }
            }
        }
        moved
    }

    fn origin_of(board: &Board, sel: Sel) -> Option<(f32, f32)> {
        Some(match sel {
            Sel::Area(id) => board.area(id).map(|a| (a.x, a.y))?,
            Sel::Note(id) => board.note(id).map(|n| (n.x, n.y))?,
            Sel::Text(id) => board.text(id).map(|t| (t.x, t.y))?,
            Sel::Shape(id) => board.shape(id).map(|s| (s.x1, s.y1))?,
            Sel::Connector(id) => board
                .connectors
                .iter()
                .find(|c| c.id == id)
                .map(|c| (c.from.x, c.from.y))?,
        })
    }

    fn apply_move_one(
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
            Mover::Text(id) => {
                if let Some(t) = board.text_mut(id) {
                    (t.x, t.y) = (x, y);
                    self.modified = true;
                    return true;
                }
            }
            Mover::Connector { id, orig } => {
                if let Some(c) = board.connector_mut(id) {
                    if c.from.id == 0 {
                        (c.from.x, c.from.y) = (orig.0 + x, orig.1 + y);
                    }
                    if c.to.id == 0 {
                        (c.to.x, c.to.y) = (orig.2 + x, orig.3 + y);
                    }
                    self.modified = true;
                    return true;
                }
            }
            Mover::Shape { id, orig } => {
                // `off` is the grab point, so (wx - off) is the pointer's travel.
                let (dx, dy) = (x, y);
                if let Some(s) = board.shape_mut(id) {
                    (s.x1, s.y1, s.x2, s.y2) = (orig.0 + dx, orig.1 + dy, orig.2 + dx, orig.3 + dy);
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
            Drag::TextSelect { .. } => false,
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
            Drag::Pending {
                who: Mover::Text(id),
                ..
            } => {
                let t = EditTarget::FreeText(id);
                if self.editing.is_some_and(|e| e.target == t) {
                    // A click on the grip of the text being typed: keep the caret where it is.
                    return false;
                }
                if board.text(id).is_some() {
                    self.start_editing_end(board, EditTarget::FreeText(id));
                    return true;
                }
                false
            }
            Drag::DrawShape { id } => {
                if let Some(s) = board.shape_mut(id) {
                    let (_, _, w, h) = s.bbox();
                    let tiny = if s.kind == ShapeKind::Line {
                        w.hypot(h) < 8.0
                    } else {
                        w < 8.0 && h < 8.0
                    };
                    if tiny {
                        // A click, not a drag: give it a sensible default size.
                        s.x2 = s.x1 + DEFAULT_SHAPE.0;
                        s.y2 = if s.kind == ShapeKind::Line {
                            s.y1
                        } else {
                            s.y1 + DEFAULT_SHAPE.1
                        };
                    }
                    self.selected = Some(Sel::Shape(id));
                    self.modified = true;
                }
                self.tool = Tool::Select;
                true
            }
            Drag::Connect { id } => {
                let end = board
                    .connectors
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| (c.from, c.to));
                if let Some((from, to)) = end {
                    // Landing on an item attaches the end to it (but not back onto the start).
                    let hit = item_at(board, to.x, to.y, adv).filter(|&i| i != from.id);
                    let moved = (to.x - from.x).hypot(to.y - from.y);
                    if hit.is_none() && from.id == 0 && moved < 8.0 {
                        // A click on empty canvas: nothing to connect.
                        board.connectors.retain(|c| c.id != id);
                    } else if let Some(c) = board.connector_mut(id) {
                        if let Some(h) = hit {
                            c.to.id = h;
                        } else if moved < 8.0 {
                            // A click on an item: give the arrow a default length.
                            c.to = End::free(from.x + 120.0, from.y);
                        }
                    }
                }
                self.modified = true;
                self.tool = Tool::Select;
                true
            }
            Drag::Resize {
                sel: Sel::Connector(id),
                handle,
                ..
            } => {
                // Dropping an end on an item attaches it.
                let other = board.connectors.iter().find(|c| c.id == id).map(|c| {
                    if handle == Handle::End1 {
                        (c.from, c.to.id)
                    } else {
                        (c.to, c.from.id)
                    }
                });
                if let Some((end, other_id)) = other {
                    let hit = item_at(board, end.x, end.y, adv).filter(|&i| i != other_id);
                    if let (Some(h), Some(c)) = (hit, board.connector_mut(id)) {
                        if handle == Handle::End1 {
                            c.from.id = h;
                        } else {
                            c.to.id = h;
                        }
                    }
                }
                true
            }
            Drag::Marquee {
                start,
                cur,
                moved: true,
                ..
            } => {
                let r = crate::layout::Rect::new(
                    start.0.min(cur.0),
                    start.1.min(cur.1),
                    (start.0 - cur.0).abs(),
                    (start.1 - cur.1).abs(),
                );
                let mut items = crate::layout::items_in_rect(board, r, adv).into_iter();
                self.selected = items.next();
                self.extra = items.collect();
                true
            }
            Drag::Resize { .. } => true,
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
            self.validate_selection(board);
        }
        ok
    }

    pub fn redo(&mut self, board: &mut Board) -> bool {
        self.finish_edit_quiet();
        let ok = self.history.redo(board);
        if ok {
            self.modified = true;
            self.validate_selection(board);
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
            Key::Up | Key::Down => {
                self.anchor = None;
                self.vertical(board, edit, key == Key::Up, adv)
            }
            Key::Text(_)
            | Key::SelectAll
            | Key::Cut
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
        let sel = self
            .selection()
            .map(|(s, e)| (s.min(chars.len()), e.min(chars.len())));
        match key {
            Key::SelectAll => {
                self.anchor = Some(0);
                self.set_caret(chars.len());
                return;
            }
            Key::Cut | Key::Text(_) | Key::Backspace | Key::Delete if sel.is_some() => {
                // Typing, Backspace, Delete and cut all first remove the selected text.
                let (s, e) = sel.unwrap();
                chars.drain(s..e);
                let mut c = s;
                if let Key::Text(t) = &key {
                    for ch in t.chars() {
                        if ch == '\n' && !target.multiline() {
                            continue;
                        }
                        chars.insert(c, ch);
                        c += 1;
                    }
                }
                self.anchor = None;
                self.commit_text(board, target, chars.iter().collect());
                self.set_caret(c);
                return;
            }
            Key::Cut => return,
            _ => {}
        }
        // Plain cursor movement: Shift extends the selection, anything else drops it.
        let moving = matches!(key, Key::Left | Key::Right | Key::Home | Key::End);
        if moving {
            if self.editor_shift() {
                self.anchor.get_or_insert(caret);
            } else if let Some((s, e)) = sel {
                // Left / Right collapse to the matching edge of the selection.
                match key {
                    Key::Left => {
                        self.anchor = None;
                        self.set_caret(s);
                        return;
                    }
                    Key::Right => {
                        self.anchor = None;
                        self.set_caret(e);
                        return;
                    }
                    _ => self.anchor = None,
                }
            } else {
                self.anchor = None;
            }
        }
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

    fn editor_shift(&self) -> bool {
        self.shift
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
            EditTarget::FreeText(_) if text_of(board, target).is_some_and(str::is_empty) => {
                self.finish_edit(board); // an empty free text is discarded
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
            EditTarget::NoteBody(_) | EditTarget::TodoBody(..) | EditTarget::FreeText(_) => {
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
    use crate::layout::{self, area_geom, Handle, Sel};

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
    fn middle_dragging_empty_space_pans_the_camera() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let mut c = cam();
        e.press(&mut b, &c, (400.0, 400.0), Button::Middle, 0, A);
        e.motion(&mut b, &mut c, (450.0, 430.0));
        e.release(&mut b, A);
        assert_eq!((c.x, c.y), (-50.0, -30.0), "content follows the pointer");
        assert!(e.take_view_changed() && !e.take_modified());
    }

    fn two_empty_areas() -> (Editor, Board, u64, u64) {
        let mut b = Board::new();
        let a1 = b.add_area(100.0, 100.0);
        let a2 = b.add_area(600.0, 100.0);
        (Editor::new(), b, a1, a2)
    }

    #[test]
    fn space_drag_pans_from_anywhere_without_touching_the_board() {
        let (mut e, mut b, ..) = two_empty_areas();
        let before: Vec<(f32, f32)> = b.areas.iter().map(|a| (a.x, a.y)).collect();
        let on_card = centre(area_geom(&b.areas[0], A).card);
        e.hand = true;
        for tool in [Tool::Select, Tool::Pen, Tool::Eraser, Tool::Note] {
            e.set_tool(&mut b, tool);
            let mut c = cam();
            e.press(&mut b, &c, on_card, Button::Left, 0, A);
            e.motion(&mut b, &mut c, (on_card.0 + 40.0, on_card.1 - 25.0));
            e.release(&mut b, A);
            assert_eq!((c.x, c.y), (-40.0, 25.0), "{tool:?}: content follows");
            assert_eq!(e.selected, None, "{tool:?}: nothing gets selected");
            assert!(e.editing.is_none());
        }
        let after: Vec<(f32, f32)> = b.areas.iter().map(|a| (a.x, a.y)).collect();
        assert_eq!(before, after);
        assert!(b.strokes.is_empty() && b.notes.is_empty());
        assert!(!e.take_modified(), "panning is not a board change");

        // Without Space the same drag on empty canvas selects and leaves the view alone.
        e.hand = false;
        e.set_tool(&mut b, Tool::Select);
        let mut c = cam();
        e.press(&mut b, &c, (20.0, 20.0), Button::Left, 5000, A);
        e.motion(&mut b, &mut c, (500.0, 140.0));
        e.release(&mut b, A);
        assert_eq!((c.x, c.y), (0.0, 0.0));
    }

    #[test]
    fn dragging_empty_space_selects_what_the_box_touches() {
        let (mut e, mut b, a1, a2) = two_empty_areas();
        let mut c = cam();
        e.press(&mut b, &c, (20.0, 20.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (500.0, 140.0));
        assert!(
            e.marquee_rect().is_some(),
            "rubber band is shown while dragging"
        );
        e.release(&mut b, A);
        assert!(e.marquee_rect().is_none());
        assert_eq!(e.selected, Some(Sel::Area(a1)));
        assert!(e.extra.is_empty(), "the second card is outside the box");
        assert_eq!((c.x, c.y), (0.0, 0.0), "selecting does not pan");

        e.press(&mut b, &c, (20.0, 20.0), Button::Left, 5000, A);
        e.motion(&mut b, &mut c, (1100.0, 140.0));
        e.release(&mut b, A);
        let mut all = vec![e.selected.unwrap()];
        all.extend(e.extra.iter().copied());
        assert_eq!(all.len(), 2);
        assert!(all.contains(&Sel::Area(a1)) && all.contains(&Sel::Area(a2)));
    }

    #[test]
    fn marquee_selection_can_be_grouped_and_deleted() {
        let (mut e, mut b, a1, a2) = two_empty_areas();
        let mut c = cam();
        e.press(&mut b, &c, (20.0, 20.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (1100.0, 140.0));
        e.release(&mut b, A);
        assert!(e.group_selected(&mut b));
        assert!(b.group_of(Sel::Area(a1)) != 0);
        assert_eq!(b.group_of(Sel::Area(a1)), b.group_of(Sel::Area(a2)));

        e.press(&mut b, &c, (20.0, 20.0), Button::Left, 1000, A);
        e.motion(&mut b, &mut c, (1100.0, 140.0));
        e.release(&mut b, A);
        assert!(e.delete_selected(&mut b));
        assert!(b.areas.is_empty() && e.selected.is_none());
        assert!(
            e.undo(&mut b) && b.areas.len() == 2,
            "one undo restores both"
        );
    }

    #[test]
    fn delete_keeps_cards_that_still_have_todos() {
        let (mut e, mut b) = setup();
        let n = b.areas.len();
        let mut c = cam();
        // a box over the whole board (starting off in the corner)
        e.press(&mut b, &c, (-500.0, -500.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (5000.0, 5000.0));
        e.release(&mut b, A);
        assert!(e.selected.is_some());
        e.delete_selected(&mut b);
        assert!(
            b.areas.iter().any(|a| !a.todos.is_empty()),
            "non-empty card survives"
        );
        assert!(b.areas.len() <= n);
    }

    #[test]
    fn a_plain_click_on_empty_space_just_deselects() {
        let (mut e, mut b, ..) = two_empty_areas();
        e.selected = Some(Sel::Area(1));
        click(&mut e, &mut b, (20.0, 20.0), 0);
        assert!(e.selected.is_none() && e.marquee_rect().is_none());
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
            due: None,
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
    fn selection_copy_replace_and_cut() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 0);
        e.shift = true;
        e.key(&mut b, Key::Right, A);
        e.key(&mut b, Key::Right, A);
        e.shift = false;
        assert_eq!(e.selection(), Some((0, 2)));
        assert_eq!(e.selected_text(&b).as_deref(), Some("al"));
        typed(&mut e, &mut b, "X"); // typing replaces the selection
        assert_eq!(texts(&b)[0], "Xpha");
        assert_eq!(e.selection(), None);
        e.key(&mut b, Key::SelectAll, A);
        assert_eq!(e.selected_text(&b).as_deref(), Some("Xpha"));
        e.key(&mut b, Key::Cut, A);
        assert_eq!(texts(&b)[0], "");
        typed(&mut e, &mut b, "pasted"); // paste is a Key::Text
        assert_eq!(texts(&b)[0], "pasted");
    }

    #[test]
    fn plain_arrow_collapses_selection_to_its_edge() {
        let (mut e, mut b) = setup();
        let id = b.areas[2].id;
        e.start_editing(EditTarget::TodoText(id, 0), 1);
        e.key(&mut b, Key::SelectAll, A);
        e.key(&mut b, Key::Left, A);
        assert_eq!((e.editing.unwrap().caret, e.selection()), (0, None));
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
                due: None,
            },
        ];
        let circle = area_geom(&b.areas[1], A).rows[3].circle;
        click(&mut e, &mut b, centre(circle), 0); // refused: Focus is full
        assert!(b.areas[1].todos[3].done);
        assert!(!e.take_modified(), "a refused action changes nothing");
        assert!(!e.undo(&mut b), "and records nothing to undo");
    }

    // ---- free text

    #[test]
    fn text_tool_places_free_text_and_it_is_multiline() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Text);
        click(&mut e, &mut b, (300.0, 200.0), 0);
        assert_eq!(b.texts.len(), 1);
        assert_eq!((b.texts[0].x, b.texts[0].y), (300.0, 200.0));
        assert_eq!(e.tool, Tool::Select, "back to Select after placing");
        assert!(matches!(e.editing.unwrap().target, EditTarget::FreeText(_)));
        typed(&mut e, &mut b, "hello");
        e.key(&mut b, Key::Enter, A);
        typed(&mut e, &mut b, "world");
        assert_eq!(
            b.texts[0].text, "hello\nworld",
            "Enter is a newline in free text"
        );
        e.key(&mut b, Key::Escape, A);
        assert!(e.editing.is_none() && b.texts.len() == 1);
    }

    #[test]
    fn empty_free_text_is_discarded() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Text);
        click(&mut e, &mut b, (10.0, 10.0), 0);
        e.key(&mut b, Key::Escape, A);
        assert!(b.texts.is_empty());
        // and Backspace in an empty free text discards it too
        e.set_tool(&mut b, Tool::Text);
        click(&mut e, &mut b, (10.0, 10.0), 5000);
        e.key(&mut b, Key::Backspace, A);
        assert!(b.texts.is_empty() && e.editing.is_none());
    }

    #[test]
    fn text_tool_on_existing_text_edits_it_and_a_click_on_a_card_does_nothing() {
        let (mut e, mut b) = setup();
        let id = b.add_text(-3000.0, -3000.0);
        b.text_mut(id).unwrap().text = "note to self".into();
        e.set_tool(&mut b, Tool::Text);
        click(&mut e, &mut b, (-2990.0, -2990.0), 0);
        assert_eq!(b.texts.len(), 1, "no second text on top of the first");
        assert_eq!(e.editing.unwrap().target, EditTarget::FreeText(id));
        assert_eq!(e.editing.unwrap().caret, 12);
        e.key(&mut b, Key::Escape, A);
        e.set_tool(&mut b, Tool::Text);
        let p = centre(work(&b).title);
        click(&mut e, &mut b, p, 5000);
        assert_eq!(b.texts.len(), 1);
        assert_eq!(
            e.tool,
            Tool::Text,
            "clicking a card keeps the Text tool armed"
        );
    }

    #[test]
    fn select_click_edits_free_text_drag_moves_it_and_right_click_removes_it() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_text(100.0, 100.0);
        b.text_mut(id).unwrap().text = "move me".into();
        let mut c = cam();
        e.press(&mut b, &c, (110.0, 110.0), Button::Left, 0, A);
        e.motion(&mut b, &mut c, (160.0, 150.0));
        e.release(&mut b, A);
        assert_eq!((b.texts[0].x, b.texts[0].y), (150.0, 140.0));
        assert!(e.editing.is_none(), "a drag does not start editing");
        assert!(e.undo(&mut b));
        assert_eq!((b.texts[0].x, b.texts[0].y), (100.0, 100.0));
        click(&mut e, &mut b, (110.0, 110.0), 9000);
        assert!(
            matches!(e.editing.unwrap().target, EditTarget::FreeText(_)),
            "a click edits"
        );
        e.key(&mut b, Key::Escape, A);
        e.press(&mut b, &cam(), (110.0, 110.0), Button::Right, 20000, A);
        assert!(b.texts.is_empty());
    }

    #[test]
    fn format_bar_styles_the_text_in_one_undo_step_each() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_text(100.0, 100.0);
        b.text_mut(id).unwrap().text = "hello".into();
        assert_eq!(e.text_focus(), None);
        e.selected = Some(Sel::Text(id));
        assert_eq!(e.text_focus(), Some(id));
        assert!(e.format_text(&mut b, id, FmtBtn::Bold));
        assert!(e.format_text(&mut b, id, FmtBtn::Size(crate::board::TextSize::Title)));
        assert!(e.format_text(&mut b, id, FmtBtn::Color(3)));
        assert!(
            !e.format_text(&mut b, id, FmtBtn::Color(3)),
            "no-op is not an undo step"
        );
        let t = b.text(id).unwrap();
        assert!(t.bold && t.size == crate::board::TextSize::Title && t.color == 3);
        assert_eq!(t.style(), Style::Free(crate::board::TextSize::Title, true));
        assert!(e.undo(&mut b));
        assert_eq!(b.text(id).unwrap().color, 0);
        assert!(e.step_text_size(&mut b, id, false));
        assert_eq!(b.text(id).unwrap().size, crate::board::TextSize::Heading);
        assert!(e.take_modified());
    }

    #[test]
    fn selected_text_in_notes_and_todos_gets_markdown_styles() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let n = b.add_note(0.0, 0.0);
        b.note_mut(n).unwrap().body = "buy milk today".into();
        e.editing = Some(Edit {
            target: EditTarget::NoteBody(n),
            caret: 8,
        });
        e.anchor = Some(4);
        assert!(e.apply_fmt(&mut b, FmtBtn::Bold));
        assert_eq!(b.note(n).unwrap().body, "buy **milk** today");
        assert_eq!(e.selection(), Some((6, 10)), "the word stays selected");
        assert!(e.apply_fmt(&mut b, FmtBtn::Bold), "again: unbold");
        assert_eq!(b.note(n).unwrap().body, "buy milk today");
        assert!(e.apply_fmt(&mut b, FmtBtn::Task));
        assert_eq!(b.note(n).unwrap().body, "- [ ] buy milk today");
        assert!(e.undo(&mut b));
        assert_eq!(b.note(n).unwrap().body, "buy milk today");
        // A single-line todo takes inline styles on the word at the caret, not line formats.
        let a = b.add_area(0.0, 400.0);
        b.area_mut(a).unwrap().todos = vec![Todo::new("call mom")];
        e.anchor = None;
        e.editing = Some(Edit {
            target: EditTarget::TodoText(a, 0),
            caret: 6,
        });
        assert!(e.apply_fmt(&mut b, FmtBtn::Italic));
        assert_eq!(b.area(a).unwrap().todos[0].text, "call *mom*");
        assert!(!e.apply_fmt(&mut b, FmtBtn::Bullet));
        // A free text box: bold with nothing selected styles the box; with a selection, the words.
        let t = b.add_text(500.0, 0.0);
        b.text_mut(t).unwrap().text = "one two".into();
        e.editing = Some(Edit {
            target: EditTarget::FreeText(t),
            caret: 7,
        });
        assert!(e.apply_fmt(&mut b, FmtBtn::Bold));
        assert!(b.text(t).unwrap().bold && b.text(t).unwrap().text == "one two");
        e.anchor = Some(4);
        assert!(e.apply_fmt(&mut b, FmtBtn::Strike));
        assert_eq!(b.text(t).unwrap().text, "one ~~two~~");
    }

    #[test]
    fn the_grip_moves_text_while_typing_and_keeps_the_caret() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_text(100.0, 100.0);
        b.text_mut(id).unwrap().text = "drag me".into();
        let mut c = cam();
        click(&mut e, &mut b, (110.0, 110.0), 0);
        e.key(&mut b, Key::Home, A);
        assert_eq!(e.editing.unwrap().caret, 0);
        // The grip sits left of the text; a press there drags the box.
        assert!(e.grab_text(&b, &c, (80.0, 110.0), id));
        e.motion(&mut b, &mut c, (130.0, 160.0));
        e.release(&mut b, A);
        assert_eq!((b.texts[0].x, b.texts[0].y), (150.0, 150.0));
        let ed = e.editing.expect("still typing");
        assert_eq!((ed.target, ed.caret), (EditTarget::FreeText(id), 0));
        assert!(e.undo(&mut b));
        assert_eq!((b.texts[0].x, b.texts[0].y), (100.0, 100.0));
        // A grip click on a merely selected text starts typing in it.
        e.key(&mut b, Key::Escape, A);
        e.selected = Some(Sel::Text(id));
        e.grab_text(&b, &c, (80.0, 110.0), id);
        e.release(&mut b, A);
        assert_eq!(e.editing.unwrap().target, EditTarget::FreeText(id));
    }

    #[test]
    fn pasting_copies_a_text_box_with_its_style_and_staggers_repeats() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_text(10.0, 10.0);
        {
            let t = b.text_mut(id).unwrap();
            t.text = "styled".into();
            (t.bold, t.color, t.group) = (true, 2, 7);
        }
        let tmpl = b.text(id).unwrap().clone();
        let a = e.paste_text_box(&mut b, Some(&tmpl), "", (300.0, 200.0));
        let p = b.text(a).unwrap();
        assert_eq!(
            (p.text.as_str(), p.bold, p.color, p.group),
            ("styled", true, 2, 0)
        );
        assert_eq!((p.x, p.y), (300.0, 200.0));
        assert_eq!(e.selected, Some(Sel::Text(a)));
        let c2 = e.paste_text_box(&mut b, None, "plain\ntext", (300.0, 200.0));
        let p2 = b.text(c2).unwrap();
        assert_eq!((p2.x, p2.y), (324.0, 224.0), "staggered, not stacked");
        assert_eq!((p2.text.as_str(), p2.bold), ("plain\ntext", false));
        assert!(e.undo(&mut b) && e.undo(&mut b));
        assert_eq!(b.texts.len(), 1);
    }

    #[test]
    fn styled_text_is_measured_at_its_size() {
        let mut b = Board::new();
        let id = b.add_text(0.0, 0.0);
        b.text_mut(id).unwrap().text = "a\nb".into();
        let adv = |s: Style, _: char| s.size() * 0.5;
        let g0 = text_geom(b.text(id).unwrap(), &adv);
        b.text_mut(id).unwrap().size = crate::board::TextSize::Title;
        let g1 = text_geom(b.text(id).unwrap(), &adv);
        assert!(g1.rect.h > g0.rect.h * 2.0 && g1.line_h > g0.line_h);
        // Markdown headings make the box taller than the raw text.
        b.text_mut(id).unwrap().size = crate::board::TextSize::Medium;
        b.text_mut(id).unwrap().text = "# a".into();
        let md = text_geom(b.text(id).unwrap(), &adv);
        b.text_mut(id).unwrap().plain = true;
        let raw = text_geom(b.text(id).unwrap(), &adv);
        assert!(md.rect.h > raw.rect.h);
    }

    // ---- shapes

    fn draw_shape(e: &mut Editor, b: &mut Board, tool: Tool, from: (f32, f32), to: (f32, f32)) {
        e.set_tool(b, tool);
        let mut c = cam();
        e.press(b, &c, from, Button::Left, 0, A);
        e.motion(b, &mut c, ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
        e.motion(b, &mut c, to);
        e.release(b, A);
    }

    #[test]
    fn connector_attaches_to_items_follows_them_and_free_ends_work() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        draw_shape(&mut e, &mut b, Tool::Rect, (0.0, 0.0), (100.0, 100.0));
        draw_shape(&mut e, &mut b, Tool::Rect, (400.0, 300.0), (500.0, 400.0));
        let (r1, r2) = (b.shapes[0].id, b.shapes[1].id);
        draw_shape(
            &mut e,
            &mut b,
            Tool::Connector,
            (50.0, 50.0),
            (450.0, 350.0),
        );
        assert_eq!(b.connectors.len(), 1);
        assert_eq!((b.connectors[0].from.id, b.connectors[0].to.id), (r1, r2));
        assert_eq!(e.tool, Tool::Select);
        // moving an attached item moves the arrow's end with it
        let before = crate::layout::connector_path(&b, &b.connectors[0], A);
        b.translate(Sel::Shape(r2), 0.0, 500.0);
        let after = crate::layout::connector_path(&b, &b.connectors[0], A);
        assert_ne!(before.last(), after.last());
        assert!(after.last().unwrap().1 > 700.0, "{after:?}");
        // a point-to-point arrow between empty spots keeps free ends
        draw_shape(
            &mut e,
            &mut b,
            Tool::Connector,
            (1000.0, 1000.0),
            (1200.0, 1100.0),
        );
        assert_eq!(b.connectors.len(), 2);
        assert_eq!((b.connectors[1].from.id, b.connectors[1].to.id), (0, 0));
        // a bare click on empty canvas makes nothing
        draw_shape(
            &mut e,
            &mut b,
            Tool::Connector,
            (2000.0, 2000.0),
            (2000.0, 2000.0),
        );
        assert_eq!(b.connectors.len(), 2);
        // deleting an attached item frees that end instead of leaving a dangling id
        e.selected = Some(Sel::Shape(r1));
        assert!(e.delete_selected(&mut b));
        assert_eq!(b.connectors[0].from.id, 0);
        // undo brings the item and the attachment back
        assert!(e.undo(&mut b));
        assert_eq!(b.connectors[0].from.id, r1);
    }

    #[test]
    fn connectors_can_be_selected_moved_reattached_recoloured_and_deleted() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        draw_shape(&mut e, &mut b, Tool::Rect, (0.0, 0.0), (100.0, 100.0));
        draw_shape(&mut e, &mut b, Tool::Rect, (400.0, 0.0), (500.0, 100.0));
        let (r1, r2) = (b.shapes[0].id, b.shapes[1].id);
        // free arrow far away from the rects
        draw_shape(
            &mut e,
            &mut b,
            Tool::Connector,
            (1000.0, 1000.0),
            (1200.0, 1000.0),
        );
        let cid = b.connectors[0].id;
        let mut c = cam();
        // click on the line selects it
        e.press(&mut b, &c, (1100.0, 1000.0), Button::Left, 0, A);
        e.release(&mut b, A);
        assert_eq!(e.selected, Some(Sel::Connector(cid)));
        // drag the body: free ends move
        e.press(&mut b, &c, (1100.0, 1000.0), Button::Left, 5000, A);
        e.motion(&mut b, &mut c, (1100.0, 1050.0));
        e.motion(&mut b, &mut c, (1100.0, 1100.0));
        e.release(&mut b, A);
        assert_eq!(
            (b.connectors[0].from.y, b.connectors[0].to.y),
            (1100.0, 1100.0)
        );
        // drag the end handle onto a rect: it attaches
        e.press(&mut b, &c, (1200.0, 1100.0), Button::Left, 9000, A);
        e.motion(&mut b, &mut c, (300.0, 50.0));
        e.motion(&mut b, &mut c, (450.0, 50.0));
        e.release(&mut b, A);
        assert_eq!(b.connectors[0].to.id, r2);
        // and the start end onto the other one
        let start = crate::layout::connector_path(&b, &b.connectors[0], A)[0];
        e.press(&mut b, &c, start, Button::Left, 14000, A);
        e.motion(&mut b, &mut c, (30.0, 30.0));
        e.motion(&mut b, &mut c, (50.0, 50.0));
        e.release(&mut b, A);
        assert_eq!((b.connectors[0].from.id, b.connectors[0].to.id), (r1, r2));
        // delete key removes it, undo brings it back
        e.selected = Some(Sel::Connector(cid));
        assert!(e.delete_selected(&mut b));
        assert!(b.connectors.is_empty());
        assert!(e.undo(&mut b));
        assert_eq!(b.connectors.len(), 1);
    }

    #[test]
    fn rectangle_ellipse_and_line_are_dragged_out_and_return_to_select() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        draw_shape(&mut e, &mut b, Tool::Rect, (100.0, 100.0), (300.0, 250.0));
        draw_shape(
            &mut e,
            &mut b,
            Tool::Ellipse,
            (400.0, 400.0),
            (350.0, 320.0),
        );
        draw_shape(&mut e, &mut b, Tool::Line, (0.0, 0.0), (80.0, 60.0));
        let kinds: Vec<ShapeKind> = b.shapes.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [ShapeKind::Rect, ShapeKind::Ellipse, ShapeKind::Line]
        );
        assert_eq!(b.shapes[0].bbox(), (100.0, 100.0, 200.0, 150.0));
        assert_eq!(
            b.shapes[1].bbox(),
            (350.0, 320.0, 50.0, 80.0),
            "dragging up-left is normalised"
        );
        assert_eq!((b.shapes[2].x2, b.shapes[2].y2), (80.0, 60.0));
        assert_eq!(e.tool, Tool::Select);
        assert_eq!(
            e.selected,
            Some(Sel::Shape(b.shapes[2].id)),
            "the new shape is selected"
        );
        assert!(
            e.undo(&mut b) && b.shapes.len() == 2,
            "one shape is one undo step"
        );
    }

    #[test]
    fn a_click_with_a_shape_tool_makes_a_default_sized_shape() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        e.set_tool(&mut b, Tool::Rect);
        click(&mut e, &mut b, (500.0, 500.0), 0);
        assert_eq!(
            b.shapes[0].bbox(),
            (500.0, 500.0, DEFAULT_SHAPE.0, DEFAULT_SHAPE.1)
        );
        e.set_tool(&mut b, Tool::Line);
        click(&mut e, &mut b, (0.0, 0.0), 5000);
        assert_eq!(
            (b.shapes[1].x2, b.shapes[1].y2),
            (DEFAULT_SHAPE.0, 0.0),
            "a default horizontal line"
        );
    }

    #[test]
    fn shapes_move_exactly_and_double_click_cycles_their_colour() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        draw_shape(&mut e, &mut b, Tool::Rect, (100.0, 100.0), (300.0, 250.0));
        e.deselect();
        let mut c = cam();
        e.press(&mut b, &c, (150.0, 150.0), Button::Left, 100_000, A);
        e.motion(&mut b, &mut c, (170.0, 160.0));
        e.motion(&mut b, &mut c, (250.0, 190.0));
        e.release(&mut b, A);
        assert_eq!(
            b.shapes[0].bbox(),
            (200.0, 140.0, 200.0, 150.0),
            "moved by exactly the pointer travel"
        );
        assert!(e.undo(&mut b));
        assert_eq!(b.shapes[0].bbox(), (100.0, 100.0, 200.0, 150.0));
        let c0 = b.shapes[0].color;
        click(&mut e, &mut b, (150.0, 150.0), 200_000);
        click(&mut e, &mut b, (151.0, 150.0), 200_100);
        assert_eq!(b.shapes[0].color, (c0 + 1) % crate::board::PALETTE_LEN);
    }

    #[test]
    fn cards_stay_clickable_on_top_of_a_shape() {
        let (mut e, mut b) = setup();
        let card = work(&b).card;
        let s = b.add_shape(ShapeKind::Rect, card.x - 50.0, card.y - 50.0);
        b.shape_mut(s).unwrap().x2 = card.right() + 50.0;
        b.shape_mut(s).unwrap().y2 = card.bottom() + 50.0;
        let p = centre(work(&b).rows[0].circle);
        click(&mut e, &mut b, p, 0);
        assert!(
            b.areas[2].todos[0].done,
            "the checkbox still works on top of a big shape"
        );
    }

    // ---- selection and deleting

    #[test]
    fn clicking_selects_and_clicking_empty_space_deselects() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).title);
        click(&mut e, &mut b, p, 0);
        assert_eq!(e.selected, Some(Sel::Area(b.areas[2].id)));
        click(&mut e, &mut b, (-9000.0, -9000.0), 5000);
        assert_eq!(e.selected, None);
        let id = b.add_note(2000.0, 2000.0);
        b.note_mut(id).unwrap().title = "n".into();
        let g = note_geom(b.note(id).unwrap(), A);
        click(&mut e, &mut b, centre(g.body.rect), 9000);
        assert_eq!(e.selected, Some(Sel::Note(id)));
        assert!(e.deselect() && e.selected.is_none() && !e.deselect());
    }

    #[test]
    fn delete_removes_the_selection_but_never_a_group_that_has_todos() {
        let (mut e, mut b) = setup();
        let p = centre(work(&b).title);
        click(&mut e, &mut b, p, 0);
        assert!(!e.delete_selected(&mut b), "a group with todos is kept");
        assert_eq!(b.areas.len(), 7);
        let s = b.add_shape(ShapeKind::Rect, -500.0, -500.0);
        b.shape_mut(s).unwrap().x2 = -400.0;
        b.shape_mut(s).unwrap().y2 = -400.0;
        e.selected = Some(Sel::Shape(s));
        assert!(e.delete_selected(&mut b) && b.shapes.is_empty() && e.selected.is_none());
        assert!(e.undo(&mut b) && b.shapes.len() == 1, "and it is undoable");
        assert!(
            !e.delete_selected(&mut b),
            "nothing selected, nothing deleted"
        );
    }

    // ---- resizing

    fn handle_pos(b: &Board, sel: Sel, h: Handle) -> (f32, f32) {
        layout::handles(b, sel, A)
            .into_iter()
            .find(|(k, _)| *k == h)
            .unwrap()
            .1
    }

    fn drag_from(e: &mut Editor, b: &mut Board, from: (f32, f32), to: (f32, f32), t: u32) {
        let mut c = cam();
        e.press(b, &c, from, Button::Left, t, A);
        e.motion(b, &mut c, ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0));
        e.motion(b, &mut c, to);
        e.release(b, A);
    }

    #[test]
    fn a_card_is_resized_by_dragging_its_side_handles() {
        let (mut e, mut b) = setup();
        let sel = Sel::Area(b.areas[2].id);
        e.selected = Some(sel);
        let w0 = b.areas[2].width();
        let right = handle_pos(&b, sel, Handle::BottomRight);
        drag_from(&mut e, &mut b, right, (right.0 + 100.0, right.1 + 500.0), 0);
        assert_eq!(
            b.areas[2].width(),
            w0 + 100.0,
            "wider; height is not touched by the pointer's y"
        );
        assert_eq!(b.areas[2].x, 100.0, "left edge fixed");
        assert!(e.undo(&mut b));
        assert_eq!(b.areas[2].width(), w0, "one drag is one undo step");
        // left handle keeps the right edge fixed
        let right_edge = b.areas[2].x + b.areas[2].width();
        let tl = handle_pos(&b, sel, Handle::TopLeft);
        drag_from(&mut e, &mut b, tl, (tl.0 - 60.0, tl.1), 9000);
        assert_eq!(b.areas[2].width(), w0 + 60.0);
        assert_eq!(
            b.areas[2].x + b.areas[2].width(),
            right_edge,
            "right edge fixed"
        );
    }

    #[test]
    fn card_and_note_widths_are_clamped() {
        let (mut e, mut b) = setup();
        let sel = Sel::Area(b.areas[2].id);
        e.selected = Some(sel);
        let br = handle_pos(&b, sel, Handle::BottomRight);
        drag_from(&mut e, &mut b, br, (br.0 - 5000.0, br.1), 0);
        assert_eq!(
            b.areas[2].width(),
            crate::board::MIN_AREA_W,
            "never narrower than the minimum"
        );
        let br = handle_pos(&b, sel, Handle::BottomRight);
        drag_from(&mut e, &mut b, br, (br.0 + 9000.0, br.1), 9000);
        assert_eq!(
            b.areas[2].width(),
            crate::board::MAX_W,
            "never wider than the maximum"
        );
        let id = b.add_note(2000.0, 2000.0);
        let nsel = Sel::Note(id);
        e.selected = Some(nsel);
        let nbr = handle_pos(&b, nsel, Handle::BottomRight);
        drag_from(&mut e, &mut b, nbr, (nbr.0 + 160.0, nbr.1), 30_000);
        assert_eq!(b.note(id).unwrap().width(), crate::board::NOTE_W + 160.0);
        let ntl = handle_pos(&b, nsel, Handle::TopLeft);
        drag_from(&mut e, &mut b, ntl, (ntl.0 + 5000.0, ntl.1), 60_000);
        assert_eq!(b.note(id).unwrap().width(), crate::board::MIN_NOTE_W);
    }

    #[test]
    fn a_handle_on_the_card_wins_over_dragging_the_card() {
        let (mut e, mut b) = setup();
        let sel = Sel::Area(b.areas[2].id);
        e.selected = Some(sel);
        let tr = handle_pos(&b, sel, Handle::TopRight);
        let (x0, y0) = (b.areas[2].x, b.areas[2].y);
        let w0 = b.areas[2].width();
        drag_from(&mut e, &mut b, tr, (tr.0 + 40.0, tr.1 + 40.0), 0);
        assert_eq!(
            (b.areas[2].x, b.areas[2].y),
            (x0, y0),
            "the card did not move"
        );
        assert_eq!(b.areas[2].width(), w0 + 40.0, "it was resized instead");
    }

    #[test]
    fn grabbing_a_handle_only_works_on_the_selected_item() {
        let (mut e, mut b) = setup();
        let tr = {
            let g = work(&b).card;
            (g.right(), g.y)
        };
        let w0 = b.areas[2].width();
        // nothing selected: pressing near the corner is just a normal click on the canvas/card
        drag_from(
            &mut e,
            &mut b,
            (tr.0 + 3.0, tr.1 - 3.0),
            (tr.0 + 80.0, tr.1 - 3.0),
            0,
        );
        assert_eq!(b.areas[2].width(), w0);
    }

    #[test]
    fn rectangle_corners_and_line_ends_are_resized() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        draw_shape(&mut e, &mut b, Tool::Rect, (100.0, 100.0), (300.0, 250.0));
        let sel = e.selected.unwrap();
        let br = handle_pos(&b, sel, Handle::BottomRight);
        drag_from(&mut e, &mut b, br, (400.0, 300.0), 0);
        assert_eq!(b.shapes[0].bbox(), (100.0, 100.0, 300.0, 200.0));
        let tl = handle_pos(&b, sel, Handle::TopLeft);
        drag_from(&mut e, &mut b, tl, (50.0, 20.0), 9000);
        assert_eq!(
            b.shapes[0].bbox(),
            (50.0, 20.0, 350.0, 280.0),
            "opposite corner stayed put"
        );
        // dragging a corner past the opposite one flips it cleanly
        let tl = handle_pos(&b, sel, Handle::TopLeft);
        drag_from(&mut e, &mut b, tl, (600.0, 500.0), 20_000);
        let (x, y, w, h) = b.shapes[0].bbox();
        assert!(
            w > 0.0 && h > 0.0 && x >= 400.0 - 1e-3 && y >= 300.0 - 1e-3,
            "{x} {y} {w} {h}"
        );
        // a line has two end handles
        draw_shape(&mut e, &mut b, Tool::Line, (0.0, 0.0), (100.0, 0.0));
        let lsel = e.selected.unwrap();
        let hs: Vec<Handle> = layout::handles(&b, lsel, A)
            .into_iter()
            .map(|(h, _)| h)
            .collect();
        assert_eq!(hs, [Handle::End1, Handle::End2]);
        let e2 = handle_pos(&b, lsel, Handle::End2);
        drag_from(&mut e, &mut b, e2, (100.0, 90.0), 50_000);
        assert_eq!((b.shapes[1].x2, b.shapes[1].y2), (100.0, 90.0));
        assert_eq!(
            (b.shapes[1].x1, b.shapes[1].y1),
            (0.0, 0.0),
            "the other end is fixed"
        );
    }

    #[test]
    fn free_text_has_no_resize_handles_and_a_removed_item_clears_the_selection() {
        let (mut e, mut b) = (Editor::new(), Board::new());
        let id = b.add_text(0.0, 0.0);
        b.text_mut(id).unwrap().text = "t".into();
        assert!(layout::handles(&b, Sel::Text(id), A).is_empty());
        let s = b.add_shape(ShapeKind::Rect, 500.0, 500.0);
        b.shape_mut(s).unwrap().x2 = 600.0;
        b.shape_mut(s).unwrap().y2 = 600.0;
        click(&mut e, &mut b, (550.0, 550.0), 0);
        assert_eq!(e.selected, Some(Sel::Shape(s)));
        e.press(&mut b, &cam(), (550.0, 550.0), Button::Right, 9000, A);
        assert_eq!(e.selected, None, "removing the selected item deselects it");
    }

    #[test]
    fn shift_click_then_ctrl_g_locks_items_so_one_drag_moves_all() {
        let mut b = Board::new();
        let n1 = b.add_note(0.0, 0.0);
        let n2 = b.add_note(600.0, 0.0);
        b.note_mut(n1).unwrap().title = "a".into();
        b.note_mut(n2).unwrap().title = "b".into();
        let mut e = Editor::new();
        let p1 = centre(crate::layout::note_geom(b.note(n1).unwrap(), A).title);
        let p2 = centre(crate::layout::note_geom(b.note(n2).unwrap(), A).title);
        click(&mut e, &mut b, p1, 0);
        e.shift = true;
        click(&mut e, &mut b, p2, 1000);
        e.shift = false;
        assert_eq!(e.extra.len(), 1);
        assert!(e.group_selected(&mut b));
        let mut c = cam();
        e.press(&mut b, &c, p1, Button::Left, 5000, A);
        e.motion(&mut b, &mut c, (p1.0 + 100.0, p1.1 + 50.0));
        e.release(&mut b, A);
        let (a, d) = (b.note(n1).unwrap(), b.note(n2).unwrap());
        assert_eq!((a.x, a.y), (100.0, 50.0));
        assert_eq!((d.x, d.y), (700.0, 50.0));
    }
}

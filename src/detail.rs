//! Todo detail page: a centred panel with the title, a multi-line description and a due date.
//!
//! Pure state and date helpers (no Wayland, no drawing); `render` draws it and `main` feeds it keys.
//! Changes are applied to the board only on save, so Esc cancels cleanly.

use crate::board::Board;
use crate::editor::Key;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Desc,
    Due,
}

impl Field {
    fn next(self) -> Field {
        match self {
            Field::Title => Field::Desc,
            Field::Desc => Field::Due,
            Field::Due => Field::Title,
        }
    }
    fn prev(self) -> Field {
        match self {
            Field::Title => Field::Due,
            Field::Desc => Field::Title,
            Field::Due => Field::Desc,
        }
    }
}

/// What the caller should do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Keep,
    /// Discard changes and close.
    Cancel,
    /// Apply to the board and close.
    Save,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub area: u64,
    pub row: usize,
    pub field: Field,
    pub title: String,
    pub desc: String,
    pub due: String,
    /// Caret, in chars, within the active field.
    pub caret: usize,
}

impl Detail {
    /// Open the page for todo `row` of area `area`. `None` if it does not exist.
    pub fn open(board: &Board, area: u64, row: usize) -> Option<Detail> {
        let t = board.area(area)?.todos.get(row)?;
        let title = t.text.clone();
        Some(Detail {
            area,
            row,
            field: Field::Title,
            caret: title.chars().count(),
            title,
            desc: t.body.clone().unwrap_or_default(),
            due: t.due.clone().unwrap_or_default(),
        })
    }

    pub fn text(&self, f: Field) -> &str {
        match f {
            Field::Title => &self.title,
            Field::Desc => &self.desc,
            Field::Due => &self.due,
        }
    }

    fn active_mut(&mut self) -> &mut String {
        match self.field {
            Field::Title => &mut self.title,
            Field::Desc => &mut self.desc,
            Field::Due => &mut self.due,
        }
    }

    pub fn focus(&mut self, f: Field) {
        self.field = f;
        self.caret = self.text(f).chars().count();
    }

    /// The due field is empty (no date) or a real `YYYY-MM-DD`.
    pub fn due_valid(&self) -> bool {
        self.due.trim().is_empty() || parse_date(self.due.trim()).is_some()
    }

    /// A todo needs a title and a valid (or empty) due date.
    pub fn can_save(&self) -> bool {
        !self.title.trim().is_empty() && self.due_valid()
    }

    fn byte(&self, chars: usize) -> usize {
        let s = self.text(self.field);
        s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        let len = self.text(self.field).chars().count();
        match key {
            Key::Escape => return Outcome::Cancel,
            // Ctrl+Enter always saves; plain Enter saves except in the description (new line).
            Key::ToggleDone => return self.save_or_keep(),
            Key::Enter if self.field == Field::Desc => self.insert("\n"),
            Key::Enter => return self.save_or_keep(),
            Key::Tab => self.focus(self.field.next()),
            Key::BackTab => self.focus(self.field.prev()),
            Key::Text(s) => {
                // Single-line fields never take a newline.
                if self.field == Field::Desc {
                    self.insert(&s);
                } else {
                    self.insert(&s.replace(['\n', '\r'], " "));
                }
            }
            Key::Backspace if self.caret > 0 => {
                let (a, b) = (self.byte(self.caret - 1), self.byte(self.caret));
                self.active_mut().replace_range(a..b, "");
                self.caret -= 1;
            }
            Key::Delete if self.caret < len => {
                let (a, b) = (self.byte(self.caret), self.byte(self.caret + 1));
                self.active_mut().replace_range(a..b, "");
            }
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(len),
            Key::Home => self.caret = 0,
            Key::End => self.caret = len,
            _ => {}
        }
        Outcome::Keep
    }

    /// Apply a Markdown edit to the active field at the caret (not the due date).
    pub fn format(&mut self, op: crate::md::MdOp) -> bool {
        if self.field == Field::Due {
            return false;
        }
        let multi = self.field == Field::Desc;
        let caret = self.caret;
        let Some((t, _, c)) = crate::md::apply(self.text(self.field), None, caret, op, multi) else {
            return false;
        };
        *self.active_mut() = t;
        self.caret = c;
        true
    }

    fn save_or_keep(&self) -> Outcome {
        if self.can_save() {
            Outcome::Save
        } else {
            Outcome::Keep
        }
    }

    fn insert(&mut self, s: &str) {
        let at = self.byte(self.caret);
        self.active_mut().insert_str(at, s);
        self.caret += s.chars().count();
    }

    /// Would saving change the todo?
    pub fn differs(&self, board: &Board) -> bool {
        let Some(t) = board.area(self.area).and_then(|a| a.todos.get(self.row)) else {
            return false;
        };
        let due = (!self.due.trim().is_empty()).then(|| self.due.trim().to_string());
        t.text != self.title.trim() || t.body.as_deref().unwrap_or("") != self.desc || t.due != due
    }

    /// Write the edited fields to the board. Returns whether anything changed.
    pub fn apply(&self, board: &mut Board) -> bool {
        if !self.can_save() {
            return false;
        }
        let Some(t) = board
            .area_mut(self.area)
            .and_then(|a| a.todos.get_mut(self.row))
        else {
            return false;
        };
        let body = (!self.desc.is_empty()).then(|| self.desc.clone());
        let due = (!self.due.trim().is_empty()).then(|| self.due.trim().to_string());
        let title = self.title.trim().to_string();
        let changed = t.text != title || t.body != body || t.due != due;
        t.text = title;
        t.body = body;
        t.due = due;
        changed
    }
}

// ---- naming a space (a one-field popup)

/// The popup that names a new space or renames one. A new space is only saved on Enter.
#[derive(Debug, Clone, PartialEq)]
pub struct NameBox {
    /// `Some(id)` renames that space; `None` saves the current view as a new one.
    pub space: Option<u64>,
    /// For a new space: the world point at the centre of the screen and the zoom.
    pub view: ((f32, f32), f32),
    pub text: String,
    pub caret: usize,
    /// The suggested name is selected: the first typed character replaces it.
    pub fresh: bool,
}

impl NameBox {
    pub fn new(space: Option<u64>, view: ((f32, f32), f32), text: &str) -> NameBox {
        NameBox {
            space,
            view,
            caret: text.chars().count(),
            text: text.to_string(),
            fresh: true,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.space.is_some() {
            "Rename space"
        } else {
            "Name this space"
        }
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        let len = self.text.chars().count();
        let at = |s: &str, n: usize| s.char_indices().nth(n).map_or(s.len(), |(i, _)| i);
        match key {
            Key::Escape => return Outcome::Cancel,
            Key::Enter | Key::ToggleDone => {
                return if self.text.trim().is_empty() {
                    Outcome::Keep
                } else {
                    Outcome::Save
                }
            }
            Key::Text(s) => {
                if self.fresh {
                    self.text.clear();
                    self.caret = 0;
                }
                let s = s.replace(['\n', '\r'], " ");
                let b = at(&self.text, self.caret);
                self.text.insert_str(b, &s);
                self.caret += s.chars().count();
            }
            Key::Backspace | Key::Delete if self.fresh => {
                self.text.clear();
                self.caret = 0;
            }
            Key::Backspace if self.caret > 0 => {
                let (a, b) = (at(&self.text, self.caret - 1), at(&self.text, self.caret));
                self.text.replace_range(a..b, "");
                self.caret -= 1;
            }
            Key::Delete if self.caret < len => {
                let (a, b) = (at(&self.text, self.caret), at(&self.text, self.caret + 1));
                self.text.replace_range(a..b, "");
            }
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(len),
            Key::Home => self.caret = 0,
            Key::End => self.caret = len,
            _ => return Outcome::Keep,
        }
        self.fresh = false;
        Outcome::Keep
    }
}

/// (card, field) of the naming popup, in screen px.
pub fn name_panel(vw: f32, vh: f32) -> (R, R) {
    let w = 400.0_f32.min(vw - 20.0).max(200.0);
    let h = 132.0;
    let (x, y) = ((vw - w) / 2.0, ((vh - h) / 2.0).max(10.0));
    (
        (x, y, w, h),
        (x + PANEL_PAD, y + 64.0, w - 2.0 * PANEL_PAD, FIELD_H),
    )
}

// ---- geometry (screen px, shared by drawing and hit-testing)

pub type R = (f32, f32, f32, f32); // x, y, w, h

#[derive(Debug, Clone, Copy)]
pub struct Panel {
    pub card: R,
    pub title: R,
    pub desc: R,
    pub due: R,
}

pub const PANEL_PAD: f32 = 24.0;
pub const LABEL_H: f32 = 22.0;
pub const FIELD_H: f32 = 36.0;
pub const DESC_H: f32 = 150.0;
pub const HEAD_H: f32 = 34.0;
pub const FOOT_H: f32 = 34.0;

pub fn panel(vw: f32, vh: f32) -> Panel {
    let w = 520.0_f32.min(vw - 20.0).max(200.0);
    let inner = w - 2.0 * PANEL_PAD;
    let h = HEAD_H + 3.0 * LABEL_H + 2.0 * FIELD_H + DESC_H + 2.0 * PANEL_PAD + FOOT_H - 8.0;
    let (x, y) = ((vw - w) / 2.0, ((vh - h) / 2.0).max(10.0));
    let fx = x + PANEL_PAD;
    let ty = y + PANEL_PAD + HEAD_H + LABEL_H;
    let dy = ty + FIELD_H + 6.0 + LABEL_H;
    let uy = dy + DESC_H + 6.0 + LABEL_H;
    Panel {
        card: (x, y, w, h),
        title: (fx, ty, inner, FIELD_H),
        desc: (fx, dy, inner, DESC_H),
        due: (fx, uy, inner, FIELD_H),
    }
}

pub fn contains(r: R, p: (f32, f32)) -> bool {
    p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3
}

// ---- dates (proleptic Gregorian, UTC; no time-zone database needed for a due *day*)

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`.
#[cfg(test)]
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400) as i32 + i32::from(m <= 2);
    (y, m, d)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Parse a strict `YYYY-MM-DD` into a real calendar date.
pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        if t.bytes().all(|c| c.is_ascii_digit()) {
            t.parse().ok()
        } else {
            None
        }
    };
    let (y, m, d) = (num(0..4)? as i32, num(5..7)?, num(8..10)?);
    ((1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)).then_some((y, m, d))
}

/// Today as days since the epoch (UTC).
pub fn today() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| (d.as_secs() / 86_400) as i64)
}

/// Whole days from `today` until the due date (negative = overdue). `None` if unparsable.
pub fn days_until(due: &str, today: i64) -> Option<i64> {
    let (y, m, d) = parse_date(due)?;
    Some(days_from_civil(y, m, d) - today)
}

/// Short badge text for a row: "Today", "Tomorrow", "Yesterday", or "Nov 25".
pub fn badge(due: &str, today: i64) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = parse_date(due)?;
    Some(match days_until(due, today)? {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => format!("{} {}", MONTHS[m as usize - 1], d),
    })
}

/// True if the due date is before `today`.
pub fn overdue(due: &str, today: i64) -> bool {
    days_until(due, today).is_some_and(|n| n < 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Board {
        let mut b = Board::new();
        b.add_area(0.0, 0.0);
        b.areas[0].todos.clear();
        b.areas[0]
            .todos
            .push(crate::board::Todo::new("write report"));
        b
    }

    #[test]
    fn panel_fields_are_inside_the_card_and_ordered() {
        let p = panel(1280.0, 720.0);
        for f in [p.title, p.desc, p.due] {
            assert!(contains(p.card, (f.0 + 1.0, f.1 + 1.0)));
            assert!(contains(p.card, (f.0 + f.2 - 1.0, f.1 + f.3 - 1.0)));
        }
        assert!(p.title.1 < p.desc.1 && p.desc.1 < p.due.1);
        // a tiny screen still gives a usable, on-screen panel
        let s = panel(300.0, 400.0);
        assert!(s.card.0 >= 0.0 && s.card.0 + s.card.2 <= 300.0);
    }

    #[test]
    fn name_box_suggestion_is_replaced_by_typing_and_empty_cannot_save() {
        let mut n = NameBox::new(None, ((0.0, 0.0), 1.0), "Space 1");
        n.key(Key::Text("Docs".into()));
        n.key(Key::Text("!".into()));
        assert_eq!(n.text, "Docs!");
        assert_eq!(n.key(Key::Enter), Outcome::Save);
        let mut n = NameBox::new(Some(3), ((0.0, 0.0), 1.0), "Space 1");
        n.key(Key::Backspace); // clears the selected suggestion at once
        assert_eq!(n.text, "");
        assert_eq!(n.key(Key::Enter), Outcome::Keep, "a name is required");
        assert_eq!(n.key(Key::Escape), Outcome::Cancel);
        let mut n = NameBox::new(Some(3), ((0.0, 0.0), 1.0), "ab");
        n.key(Key::Left);
        n.key(Key::Text("X".into()));
        assert_eq!(
            n.text, "aXb",
            "moving the caret keeps the name and edits in place"
        );
        let mut n = NameBox::new(Some(3), ((0.0, 0.0), 1.0), "ab");
        n.key(Key::End);
        n.key(Key::Text("c".into()));
        assert_eq!(n.text, "abc");
    }

    #[test]
    fn civil_round_trip_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for z in [-800_000, -1, 0, 19_000, 20_777, 900_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn parse_accepts_real_dates_only() {
        assert_eq!(parse_date("2026-11-21"), Some((2026, 11, 21)));
        assert_eq!(parse_date("2024-02-29"), Some((2024, 2, 29)));
        for bad in [
            "2025-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-11-31",
            "26-11-21",
            "2026/11/21",
            "2026-1-21",
            "abcd-ef-gh",
            "+026-11-21",
            "",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn badge_and_overdue() {
        let t = days_from_civil(2026, 11, 21);
        assert_eq!(badge("2026-11-21", t).as_deref(), Some("Today"));
        assert_eq!(badge("2026-11-22", t).as_deref(), Some("Tomorrow"));
        assert_eq!(badge("2026-11-20", t).as_deref(), Some("Yesterday"));
        assert_eq!(badge("2026-12-05", t).as_deref(), Some("Dec 5"));
        assert!(overdue("2026-11-20", t) && !overdue("2026-11-21", t));
        assert_eq!(badge("nonsense", t), None);
    }

    #[test]
    fn open_loads_the_todo_and_save_writes_it_back() {
        let mut b = board();
        let a = b.areas[0].id;
        let mut d = Detail::open(&b, a, 0).unwrap();
        assert_eq!(
            (d.title.as_str(), d.desc.as_str(), d.due.as_str()),
            ("write report", "", "")
        );
        d.key(Key::Tab);
        d.key(Key::Text("line1".into()));
        d.key(Key::Enter); // newline in the description
        d.key(Key::Text("line2".into()));
        d.key(Key::Tab);
        d.key(Key::Text("2026-12-01".into()));
        assert_eq!(d.key(Key::Enter), Outcome::Save);
        assert!(d.apply(&mut b));
        let t = &b.areas[0].todos[0];
        assert_eq!(t.body.as_deref(), Some("line1\nline2"));
        assert_eq!(t.due.as_deref(), Some("2026-12-01"));
        assert!(!d.apply(&mut b), "second apply changes nothing");
    }

    #[test]
    fn invalid_due_or_empty_title_blocks_saving_and_esc_cancels() {
        let mut b = board();
        let a = b.areas[0].id;
        let mut d = Detail::open(&b, a, 0).unwrap();
        d.focus(Field::Due);
        d.key(Key::Text("2026-02-30".into()));
        assert!(!d.due_valid());
        assert_eq!(d.key(Key::Enter), Outcome::Keep);
        assert!(!d.apply(&mut b));
        assert_eq!(b.areas[0].todos[0].due, None);
        assert_eq!(d.key(Key::Escape), Outcome::Cancel);
        d.due.clear();
        d.title.clear();
        assert!(!d.can_save());
    }

    #[test]
    fn caret_editing_is_char_based() {
        let b = board();
        let mut d = Detail::open(&b, b.areas[0].id, 0).unwrap();
        d.title = "héllo".into();
        d.caret = 2;
        d.key(Key::Backspace);
        assert_eq!(d.title, "hllo");
        d.key(Key::Text("é".into()));
        d.key(Key::End);
        d.key(Key::Text("!".into()));
        assert_eq!(d.title, "héllo!");
        d.key(Key::Home);
        d.key(Key::Delete);
        assert_eq!(d.title, "éllo!");
    }

    #[test]
    fn clearing_the_description_and_due_removes_them() {
        let mut b = board();
        b.areas[0].todos[0].body = Some("x".into());
        b.areas[0].todos[0].due = Some("2026-01-01".into());
        let mut d = Detail::open(&b, b.areas[0].id, 0).unwrap();
        d.desc.clear();
        d.due.clear();
        assert!(d.apply(&mut b));
        let t = &b.areas[0].todos[0];
        assert_eq!((t.body.clone(), t.due.clone()), (None, None));
    }
}

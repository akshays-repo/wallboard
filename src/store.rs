//! JSON load/save (format version 2).
//!
//! - Every field added after the first release has a serde default, so older files load.
//! - Writes are atomic: temp file in the same directory, fsync, rename.
//! - `board.json.bak` is a copy of the last file that loaded successfully.
//! - A file that cannot be used (corrupt, unknown version) is moved to `board.json.bad`
//!   (or `.bad.1`, `.bad.2`, ... if one exists) and never overwritten; the app starts fresh.

use crate::board::{
    Area, Background, Board, Connector, Note, Shape, Space, Stroke, TextBox, Theme,
};
use crate::camera::Camera;
use crate::layout::Margins;
use crate::sync::BackupSettings;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Save-file format version this build reads and writes.
pub const VERSION: u32 = 2;

/// Everything that is saved besides the board content.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub camera: Option<Camera>,
    pub theme: Theme,
    pub background: Background,
    pub margins: Margins,
    pub backup: BackupSettings,
}

#[derive(Deserialize)]
struct FileData {
    #[serde(default)]
    areas: Vec<Area>,
    #[serde(default)]
    notes: Vec<Note>,
    #[serde(default)]
    strokes: Vec<Stroke>,
    #[serde(default)]
    texts: Vec<TextBox>,
    #[serde(default)]
    shapes: Vec<Shape>,
    #[serde(default)]
    connectors: Vec<Connector>,
    #[serde(default)]
    spaces: Vec<Space>,
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    camera: Option<Camera>,
    #[serde(default)]
    theme: Theme,
    #[serde(default)]
    background: Background,
    #[serde(default)]
    ui: Margins,
    #[serde(default)]
    backup: BackupSettings,
}

#[derive(Serialize)]
struct FileRef<'a> {
    version: u32,
    areas: &'a [Area],
    notes: &'a [Note],
    strokes: &'a [Stroke],
    texts: &'a [TextBox],
    shapes: &'a [Shape],
    connectors: &'a [Connector],
    spaces: &'a [Space],
    next_id: u64,
    camera: Option<Camera>,
    theme: Theme,
    background: &'a Background,
    ui: Margins,
    backup: BackupSettings,
}

/// `$WALLBOARD_DATA_DIR`, else `$XDG_DATA_HOME/wallboard` (default `~/.local/share/wallboard`).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WALLBOARD_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wallboard")
}

pub fn board_path() -> PathBuf {
    data_dir().join("board.json")
}

#[derive(Debug)]
pub enum Loaded {
    /// No save file yet.
    Missing,
    Ok {
        board: Board,
        view: View,
    },
    /// The file could not be used. It was moved to `backup` (if that worked); start fresh.
    Rejected {
        reason: String,
        backup: Option<PathBuf>,
    },
}

pub fn load(path: &Path) -> Loaded {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return reject(path, format!("cannot read file: {e}")),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return reject(path, format!("not valid JSON: {e}")),
    };
    match value.get("version").and_then(|v| v.as_u64()) {
        None => return reject(path, "missing or invalid \"version\" field".to_string()),
        Some(v) if v != u64::from(VERSION) => {
            return reject(
                path,
                format!("unsupported version {v} (this build reads {VERSION})"),
            )
        }
        Some(_) => {}
    }
    match serde_json::from_value::<FileData>(value) {
        Ok(f) => {
            // This file is good: keep a copy as the last known-good version.
            if let Err(e) = fs::copy(path, path.with_extension("json.bak")) {
                eprintln!(
                    "wallboard: could not write {}: {e}",
                    path.with_extension("json.bak").display()
                );
            }
            Loaded::Ok {
                board: Board::from_parts(f.areas, f.notes, f.strokes, f.texts, f.shapes, f.next_id)
                    .with_connectors(f.connectors)
                    .with_spaces(f.spaces),
                view: View {
                    camera: f.camera.filter(Camera::is_valid),
                    theme: f.theme,
                    background: f.background,
                    margins: f.ui.sane(),
                    backup: f.backup.sane(),
                },
            }
        }
        Err(e) => reject(path, format!("unexpected structure: {e}")),
    }
}

/// Move a bad file aside to `.bad` (never overwriting an earlier one) and report why.
fn reject(path: &Path, reason: String) -> Loaded {
    let base = path.with_extension("json.bad");
    let mut backup = base.clone();
    let mut n = 1;
    while backup.exists() {
        let mut name = base
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        name.push(format!(".{n}"));
        backup = base.with_file_name(name);
        n += 1;
    }
    let backup = match fs::rename(path, &backup) {
        Ok(()) => Some(backup),
        Err(e) => {
            eprintln!("wallboard: could not move {} aside: {e}", path.display());
            None
        }
    };
    Loaded::Rejected { reason, backup }
}

/// Write the board atomically: temp file, fsync, rename over the target.
pub fn save(path: &Path, board: &Board, view: &View) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let data = FileRef {
        version: VERSION,
        areas: &board.areas,
        notes: &board.notes,
        strokes: &board.strokes,
        texts: &board.texts,
        shapes: &board.shapes,
        connectors: &board.connectors,
        spaces: &board.spaces,
        next_id: board.next_id(),
        camera: view.camera,
        theme: view.theme,
        background: &view.background,
        ui: view.margins,
        backup: view.backup,
    };
    let json = serde_json::to_vec_pretty(&data).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Hash of everything that counts as content (not camera, theme or background), used to decide
/// whether a git backup is worth making.
pub fn content_hash(board: &Board) -> u64 {
    #[derive(Serialize)]
    struct C<'a> {
        areas: &'a [Area],
        notes: &'a [Note],
        strokes: &'a [Stroke],
        texts: &'a [TextBox],
        shapes: &'a [Shape],
        connectors: &'a [Connector],
        spaces: &'a [Space],
    }
    let json = serde_json::to_string(&C {
        areas: &board.areas,
        notes: &board.notes,
        strokes: &board.strokes,
        texts: &board.texts,
        shapes: &board.shapes,
        connectors: &board.connectors,
        spaces: &board.spaces,
    })
    .unwrap_or_default();
    let mut h = DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Todo;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wallboard-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> Board {
        let mut b = Board::default_board();
        b.areas[1].todos.push(Todo {
            text: "ship \"it\"\nnow".into(),
            done: true,
            priority: 3,
            body: Some("details\nmore".into()),
            due: None,
        });
        let n = b.add_note(1.0, 2.0);
        b.note_mut(n).unwrap().title = "héllo".into();
        let s = b.add_stroke(0.0, 0.0, 2);
        b.stroke_mut(s).unwrap().pts.push((5.0, 6.0));
        b
    }

    fn view() -> View {
        View {
            camera: Some(Camera {
                x: -10.5,
                y: 20.0,
                zoom: 1.75,
            }),
            theme: Theme::Light,
            background: Background {
                color: Some("#14211A".into()),
                ..Background::default()
            },
            margins: Margins { top: 70.0 },
            backup: BackupSettings {
                interval_secs: 120,
                debounce_secs: 15,
            },
        }
    }

    #[test]
    fn roundtrip_everything() {
        let d = tmpdir("roundtrip");
        let p = d.join("board.json");
        let b = sample();
        save(&p, &b, &view()).unwrap();
        match load(&p) {
            Loaded::Ok { board, view: v } => {
                assert_eq!(board.areas, b.areas);
                assert_eq!(board.notes, b.notes);
                assert_eq!(board.strokes, b.strokes);
                assert_eq!(v, view());
                assert!(board.next_id() >= b.next_id());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn margins_round_trip_and_bad_values_are_repaired_on_load() {
        let d = tmpdir("margins");
        let p = d.join("board.json");
        fs::write(&p, br#"{"version":2,"areas":[],"ui":{"top":-50}}"#).unwrap();
        match load(&p) {
            Loaded::Ok { view, .. } => {
                assert_eq!(view.margins.top, 0.0);
            }
            other => panic!("{other:?}"),
        }
        fs::write(&p, br#"{"version":2,"areas":[],"ui":{"top":80}}"#).unwrap();
        match load(&p) {
            Loaded::Ok { view, .. } => assert_eq!(view.margins, Margins { top: 80.0 }),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn backup_interval_defaults_to_ten_minutes_and_bad_values_are_repaired() {
        let d = tmpdir("backupcfg");
        let p = d.join("board.json");
        fs::write(&p, br#"{"version":2,"areas":[]}"#).unwrap();
        match load(&p) {
            Loaded::Ok { view, .. } => assert_eq!(
                view.backup.interval_secs, 600,
                "every 10 minutes by default"
            ),
            other => panic!("{other:?}"),
        }
        fs::write(
            &p,
            br#"{"version":2,"areas":[],"backup":{"interval_secs":1,"debounce_secs":9999}}"#,
        )
        .unwrap();
        match load(&p) {
            Loaded::Ok { view, .. } => assert_eq!(
                (view.backup.interval_secs, view.backup.debounce_secs),
                (5, 5)
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_file_is_missing() {
        let d = tmpdir("missing");
        assert!(matches!(load(&d.join("board.json")), Loaded::Missing));
    }

    #[test]
    fn creates_parent_directory_and_leaves_no_tmp() {
        let d = tmpdir("mkdir");
        let p = d.join("a/b/board.json");
        save(&p, &Board::new(), &View::default()).unwrap();
        assert!(p.exists());
        assert!(!p.with_extension("json.tmp").exists());
    }

    #[test]
    fn bak_is_a_copy_of_the_last_good_load_not_of_every_save() {
        let d = tmpdir("bak");
        let p = d.join("board.json");
        save(&p, &sample(), &view()).unwrap();
        let first = fs::read(&p).unwrap();
        assert!(
            !d.join("board.json.bak").exists(),
            "saving alone does not create a .bak"
        );
        assert!(matches!(load(&p), Loaded::Ok { .. }));
        assert_eq!(fs::read(d.join("board.json.bak")).unwrap(), first);
        // saving again must not touch the .bak
        save(&p, &Board::new(), &View::default()).unwrap();
        assert_eq!(fs::read(d.join("board.json.bak")).unwrap(), first);
        assert_ne!(fs::read(&p).unwrap(), first);
        // the next good load refreshes it
        assert!(matches!(load(&p), Loaded::Ok { .. }));
        assert_eq!(
            fs::read(d.join("board.json.bak")).unwrap(),
            fs::read(&p).unwrap()
        );
    }

    fn assert_rejected_and_preserved(d: &Path, content: &[u8], expect: &str) -> PathBuf {
        let p = d.join("board.json");
        fs::write(&p, content).unwrap();
        match load(&p) {
            Loaded::Rejected { reason, backup } => {
                assert!(reason.contains(expect), "{reason}");
                let backup = backup.expect("backup created");
                assert_eq!(
                    fs::read(&backup).unwrap(),
                    content,
                    "original bytes preserved"
                );
                assert!(
                    !p.exists(),
                    "bad file moved away, path free for a fresh save"
                );
                assert!(backup
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".bad"));
                backup
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn garbage_is_moved_to_bad_not_overwritten() {
        let b = assert_rejected_and_preserved(
            &tmpdir("garbage"),
            b"{ this is not json",
            "not valid JSON",
        );
        assert!(b.ends_with("board.json.bad"));
    }

    #[test]
    fn empty_file_is_rejected() {
        assert_rejected_and_preserved(&tmpdir("empty"), b"", "not valid JSON");
    }

    #[test]
    fn unknown_and_old_versions_are_rejected() {
        assert_rejected_and_preserved(
            &tmpdir("ver99"),
            br#"{"version":99,"areas":[]}"#,
            "unsupported version 99",
        );
        assert_rejected_and_preserved(
            &tmpdir("ver1"),
            br#"{"version":1,"items":[]}"#,
            "unsupported version 1",
        );
        assert_rejected_and_preserved(&tmpdir("nover"), br#"{"areas":[]}"#, "version");
    }

    #[test]
    fn wrong_structure_is_rejected() {
        let c = br#"{"version":2,"areas":[{"nope":true}]}"#;
        assert_rejected_and_preserved(&tmpdir("shape"), c, "unexpected structure");
    }

    #[test]
    fn a_second_bad_file_never_overwrites_the_first() {
        let d = tmpdir("twobad");
        let first = assert_rejected_and_preserved(&d, b"first bad", "not valid JSON");
        let second = assert_rejected_and_preserved(&d, b"second bad", "not valid JSON");
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"first bad");
        assert!(second.to_string_lossy().ends_with(".bad.1"));
        let third = assert_rejected_and_preserved(&d, b"third bad", "not valid JSON");
        assert!(third.to_string_lossy().ends_with(".bad.2"));
    }

    #[test]
    fn older_files_load_through_serde_defaults() {
        let d = tmpdir("old");
        let p = d.join("board.json");
        // A minimal version-2 file from before notes, strokes, theme, background, hero and priority existed.
        fs::write(
            &p,
            br#"{"version":2,"areas":[{"id":3,"name":"Old","x":1,"y":2,"todos":[{"text":"keep"}]}]}"#,
        )
        .unwrap();
        match load(&p) {
            Loaded::Ok { board, view } => {
                assert_eq!(board.areas[0].todos[0].text, "keep");
                assert!(!board.areas[0].hero && board.areas[0].todos[0].priority == 0);
                assert!(board.notes.is_empty() && board.strokes.is_empty());
                assert_eq!(view, View::default());
                assert!(board.next_id() > 3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_camera_is_dropped_not_fatal() {
        let d = tmpdir("cam");
        let p = d.join("board.json");
        fs::write(
            &p,
            br#"{"version":2,"areas":[],"camera":{"x":0,"y":0,"zoom":0}}"#,
        )
        .unwrap();
        match load(&p) {
            Loaded::Ok { view, .. } => assert_eq!(view.camera, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn content_hash_ignores_view_changes_but_not_content() {
        let mut b = sample();
        let h0 = content_hash(&b);
        assert_eq!(h0, content_hash(&b), "stable");
        // Pan/zoom/theme/background are not part of the board, so they cannot change the hash.
        b.areas[2].todos.push(Todo::new("new"));
        assert_ne!(h0, content_hash(&b));
        let h1 = content_hash(&b);
        b.areas[2].todos[0].done = true;
        assert_ne!(h1, content_hash(&b));
    }

    #[test]
    fn files_from_before_the_dock_was_removed_still_load() {
        // Older saves have a "launchers" list and "ui.bottom"; both are simply ignored now.
        let d = tmpdir("olddock");
        let p = d.join("board.json");
        fs::write(
            &p,
            br#"{"version":2,"areas":[{"id":1,"name":"Inbox","x":0,"y":0}],
                 "launchers":[{"label":"Files","command":"xdg-open ."}],"ui":{"top":70,"bottom":52}}"#,
        )
        .unwrap();
        match load(&p) {
            Loaded::Ok { board, view } => {
                assert_eq!(board.areas[0].name, "Inbox");
                assert_eq!(view.margins.top, 70.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn texts_shapes_and_resized_widths_survive_a_save_and_load() {
        let d = tmpdir("freeitems");
        let p = d.join("board.json");
        let mut b = Board::default_board();
        b.areas[2].w = 480.0;
        let n = b.add_note(5.0, 6.0);
        b.note_mut(n).unwrap().w = 333.0;
        let t = b.add_text(7.0, 8.0);
        b.text_mut(t).unwrap().text = "free\ntext".into();
        {
            let tb = b.text_mut(t).unwrap();
            tb.size = crate::board::TextSize::Title;
            (
                tb.bold,
                tb.italic,
                tb.underline,
                tb.strike,
                tb.plain,
                tb.color,
            ) = (true, true, true, true, true, 4);
        }
        let sh = b.add_shape(crate::board::ShapeKind::Ellipse, 1.0, 2.0);
        b.shape_mut(sh).unwrap().x2 = 90.0;
        b.shape_mut(sh).unwrap().y2 = 70.0;
        save(&p, &b, &View::default()).unwrap();
        match load(&p) {
            Loaded::Ok { board, .. } => {
                assert_eq!(board.areas[2].width(), 480.0);
                assert_eq!(board.notes[0].width(), 333.0);
                assert_eq!(board.texts, b.texts);
                assert_eq!(board.shapes, b.shapes);
                assert!(board.next_id() > sh);
            }
            other => panic!("{other:?}"),
        }
        assert_ne!(content_hash(&b), {
            let mut c = Board::default_board();
            c.areas[2].w = 480.0;
            content_hash(&c)
        });
    }

    #[test]
    fn spaces_survive_a_save_and_load_and_old_files_have_none() {
        let dir = tmpdir("spaces");
        let path = dir.join("board.json");
        let mut b = Board::default_board();
        b.add_space("Docs", 12.5, -40.0, 1.25);
        save(&path, &b, &View::default()).unwrap();
        match load(&path) {
            Loaded::Ok { board, .. } => assert_eq!(board.spaces, b.spaces),
            _ => panic!("load failed"),
        }
        assert_ne!(content_hash(&b), content_hash(&Board::default_board()));
        fs::write(&path, r#"{"version":2,"areas":[]}"#).unwrap();
        match load(&path) {
            Loaded::Ok { board, .. } => assert!(board.spaces.is_empty()),
            _ => panic!("old file"),
        }
    }

    #[test]
    fn older_files_without_widths_texts_or_shapes_still_load() {
        let d = tmpdir("nowidth");
        let p = d.join("board.json");
        fs::write(
            &p,
            br#"{"version":2,"areas":[{"id":1,"name":"Old","x":0,"y":0,"hero":true}],"notes":[{"id":2,"x":0,"y":0}]}"#,
        )
        .unwrap();
        match load(&p) {
            Loaded::Ok { board, .. } => {
                assert_eq!(
                    board.areas[0].width(),
                    crate::board::HERO_W,
                    "default hero width"
                );
                assert_eq!(board.notes[0].width(), crate::board::NOTE_W);
                assert!(board.texts.is_empty() && board.shapes.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }
}

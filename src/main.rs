//! wallboard: a life-OS home screen on the niri background layer.
//!
//! Canvas: scroll pans, Ctrl+scroll zooms at the cursor, drag empty space or middle-drag pans.
//!
//! Keys (when not typing): `1`-`9` jump to an area, `C` capture a todo into Inbox, `0` fit
//! everything, `+`/`-` zoom, `L` dark/light, `B` cycle the background (colours, then your
//! wallpaper), `V` select, `G` group, `T` todo, `N` note, `P` pen, `E` eraser,
//! Ctrl+Z / Ctrl+Shift+Z undo / redo, Ctrl+Q quit.
//!
//! Typing: Enter adds a todo (an empty one is discarded), Tab opens a todo's details,
//! Ctrl+Enter toggles done, Ctrl+P cycles priority, Esc stops.
//!
//! The board autosaves about 1 s after a change to `$XDG_DATA_HOME/wallboard/board.json`.
//! If that folder is a git repository it is also backed up to its (private) remote, see `sync`.
//! Set `WALLBOARD_DEBUG=1` for an event/state log on stdout.

// Drawing helpers pass many small values (view, colours, geometry); bundling them into
// one-off structs would add noise without clarity, so this lint is off for the whole crate.
#![allow(clippy::too_many_arguments)]

mod board;
mod camera;
mod editor;
mod layout;
mod render;
mod store;
mod sync;
mod text;

use board::{Background, Board, Theme};
use camera::Camera;
use editor::{Button, Editor, Key};
use layout::{area_geom, note_geom, Margins, Overlay, Rect, Tool, ViewBtn};
use serde_json::json;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{AxisScroll, PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::time::{Duration, Instant};
use store::{Loaded, View};
use text::{Fonts, Style};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// One wheel notch is 15 px of `absolute` scroll in niri.
const PX_PER_NOTCH: f32 = 15.0;
/// A real mouse-wheel notch pans this many px (smooth/touchpad scroll is used as is).
const PAN_PER_NOTCH: f32 = 60.0;
const ZOOM_PER_NOTCH: f32 = 1.1;
/// Margin (screen px) around everything when fitting the view.
const FIT_MARGIN: f32 = 60.0;
/// Do not zoom in past this when jumping to an area.
const JUMP_MAX_ZOOM: f32 = 1.2;
/// Wait this long after the last change before writing the save file.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// After this long without drawing, drop the spare shm buffer and the wallpaper cache.
const IDLE_SHRINK: Duration = Duration::from_secs(2);

/// Resident set size in KiB, from /proc/self/status.
fn vmrss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn rect_json(r: Rect) -> serde_json::Value {
    json!([r.x, r.y, r.w, r.h])
}

const HELP: &str = "wallboard: a life-OS home screen for niri

USAGE:
    wallboard                       run the board (normally started by niri)
    wallboard --init-sync <URL>     back board.json up to a PRIVATE git repository
    wallboard --help                show this help

The remote repository MUST be private: board.json holds your todos and notes.
Create one with `gh repo create wallboard-data --private` (or on github.com), then run
    wallboard --init-sync git@github.com:YOU/wallboard-data.git
";

/// Command-line modes that run without a display. Returns an exit code, or None to run the app.
fn cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => None,
        Some("-h") | Some("--help") => {
            print!("{HELP}");
            Some(0)
        }
        Some("--init-sync") => {
            let Some(url) = args.get(1) else {
                eprintln!("wallboard: --init-sync needs the remote URL\n\n{HELP}");
                return Some(2);
            };
            let dir = store::data_dir();
            println!("Backing up {} to {url}", dir.display());
            println!(
                "IMPORTANT: this repository must be PRIVATE (it will hold your todos and notes)."
            );
            match sync::init_sync(&dir, url, &sync::Timing::default()) {
                Ok(steps) => {
                    for s in steps {
                        println!("  - {s}");
                    }
                    println!("Done. Changes are now committed and pushed automatically while wallboard runs.");
                    Some(0)
                }
                Err(e) => {
                    eprintln!("wallboard: {e}");
                    Some(1)
                }
            }
        }
        Some(other) => {
            eprintln!("wallboard: unknown option {other:?}\n\n{HELP}");
            Some(2)
        }
    }
}

fn main() {
    if let Some(code) = cli() {
        std::process::exit(code);
    }
    let debug = std::env::var_os("WALLBOARD_DEBUG").is_some();
    let conn = Connection::connect_to_env().expect("cannot connect to Wayland");
    let (globals, event_queue) = registry_queue_init(&conn).expect("registry init");
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<App> = EventLoop::try_new().expect("event loop");
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .expect("insert wayland source");

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor missing");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell missing");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm missing");

    let surface = compositor.create_surface(&qh);
    // The bottom layer sits above every background-layer surface, so a shell that (re)creates its
    // own wallpaper a few seconds after login (DankMaterialShell does) cannot cover the board.
    // WALLBOARD_LAYER=background opts back into the background layer.
    let layer_kind = match std::env::var("WALLBOARD_LAYER").as_deref() {
        Ok("background") => Layer::Background,
        _ => Layer::Bottom,
    };
    let layer = layer_shell.create_layer_surface(&qh, surface, layer_kind, Some("wallboard"), None);
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, 0);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
    layer.commit(); // no buffer: triggers the first configure

    // Starts tiny; SlotPool grows when the first buffer is created.
    let pool = SlotPool::new(4096, &shm).expect("shm pool");

    let (fonts, font_label) = match Fonts::load() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("wallboard: {e}");
            std::process::exit(1);
        }
    };

    // Load the saved board. A bad file is moved aside (never overwritten) and we start fresh.
    let data_path = store::board_path();
    let (board, view) = match store::load(&data_path) {
        Loaded::Missing => (Board::default_board(), View::default()),
        Loaded::Ok { board, view } => (board, view),
        Loaded::Rejected { reason, backup } => {
            eprintln!("wallboard: ignoring {}: {reason}", data_path.display());
            match backup {
                Some(b) => eprintln!("wallboard: the original was kept as {}", b.display()),
                None => eprintln!("wallboard: the original file was left in place"),
            }
            eprintln!("wallboard: starting with a fresh board");
            (Board::default_board(), View::default())
        }
    };

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        qh,
        loop_handle: event_loop.handle(),
        keyboard: None,
        pointer: None,
        width: 0,
        height: 0,
        scale: 1,
        camera: view.camera.unwrap_or(Camera {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }),
        camera_loaded: view.camera.is_some(),
        theme: view.theme,
        background: view.background,
        margins: view.margins,
        wallpaper: None,
        wallpaper_failed: false,
        last_hash: store::content_hash(&board),
        backup: sync::Backup::start(
            store::data_dir(),
            sync::Timing::from_settings(view.backup),
            debug,
        ),
        backup_settings: view.backup,
        board,
        editor: Editor::new(),
        fonts,
        data_path,
        save_due: None,
        save_armed: false,
        cursor: (0.0, 0.0),
        mods: Modifiers::default(),
        show_help: false,
        dirty: false,
        frame_pending: false,
        last_draw: Instant::now(),
        shrink_armed: false,
        configured: false,
        draws: 0,
        debug,
        exit: false,
    };

    app.log(format_args!("[font] {font_label}"));
    app.log(format_args!("[data] {}", app.data_path.display()));
    app.log(format_args!("[start] VmRSS = {} KiB", vmrss_kib()));

    // Block until an event arrives: idle CPU stays at zero.
    while !app.exit {
        event_loop.dispatch(None, &mut app).expect("dispatch");
    }
    if app.save_due.is_some() {
        app.save(); // flush any pending change
    }
    app.log(format_args!(
        "[exit] draws={} VmRSS = {} KiB",
        app.draws,
        vmrss_kib()
    ));
    // Dropping `app` destroys the layer surface and closes the connection.
}

struct App {
    /// The keyboard shortcuts popup is open.
    show_help: bool,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Logical size from configure.
    width: u32,
    height: u32,
    scale: i32,
    camera: Camera,
    /// The camera came from the save file (do not re-centre on first configure).
    camera_loaded: bool,
    theme: Theme,
    background: Background,
    /// Gap kept clear above the chips (see `layout::Margins`).
    margins: Margins,
    /// Wallpaper pixels for the current buffer size (dropped when idle).
    wallpaper: Option<Vec<u8>>,
    /// Loading the wallpaper failed once; do not retry every frame.
    wallpaper_failed: bool,
    /// Hash of the board content at the last backup notification (view-only changes keep it).
    last_hash: u64,
    /// Git backup thread, if the data folder is a repository.
    backup: Option<sync::Backup>,
    backup_settings: sync::BackupSettings,
    board: Board,
    editor: Editor,
    fonts: Fonts,
    data_path: std::path::PathBuf,
    /// When a debounced save is due.
    save_due: Option<Instant>,
    save_armed: bool,
    /// Pointer position, logical px.
    cursor: (f32, f32),
    mods: Modifiers,
    /// State changed since the last draw.
    dirty: bool,
    /// A frame callback is outstanding; do not draw again until it fires.
    frame_pending: bool,
    last_draw: Instant,
    /// An idle-shrink timer is registered.
    shrink_armed: bool,
    configured: bool,
    draws: u64,
    debug: bool,
    exit: bool,
}

impl App {
    fn log(&self, args: std::fmt::Arguments) {
        if self.debug {
            println!("{args}");
        }
    }

    fn view_size(&self) -> (f32, f32) {
        (self.width as f32, self.height as f32)
    }

    fn overlay(&self) -> Overlay {
        let f = &self.fonts;
        let (vw, vh) = self.view_size();
        layout::overlay(&self.board, vw, vh, self.margins, &|s, t| {
            f.text_width(s.face(), t, s.size())
        })
    }

    /// Mark state changed and draw now unless a frame is already in flight.
    fn request_redraw(&mut self) {
        self.dirty = true;
        if !self.frame_pending {
            self.draw();
        }
    }

    /// The wallpaper path: the configured one, else `wallpaper.png` in the data directory.
    fn wallpaper_path(&self) -> Option<String> {
        if let Some(p) = &self.background.image {
            return Some(p.clone());
        }
        let default = store::data_dir().join("wallpaper.png");
        default
            .exists()
            .then(|| default.to_string_lossy().into_owned())
    }

    /// Build (or drop) the wallpaper cache so it matches the current buffer size and settings.
    fn ensure_wallpaper(&mut self, w: u32, h: u32) {
        if !self.background.image_on {
            self.wallpaper = None;
            return;
        }
        let want = (w * h * 4) as usize;
        if self.wallpaper.as_ref().is_some_and(|v| v.len() == want) || self.wallpaper_failed {
            return;
        }
        let Some(path) = self.wallpaper_path() else {
            self.wallpaper_failed = true;
            self.background.image_on = false;
            eprintln!(
                "wallboard: no wallpaper image: set \"background.image\" in board.json or add wallpaper.png to {}",
                store::data_dir().display()
            );
            return;
        };
        let started = Instant::now();
        match render::build_wallpaper(&path, w, h, self.background.dim) {
            Ok(data) => {
                self.log(format_args!(
                    "[wallpaper] {path} -> {w}x{h} in {} ms",
                    started.elapsed().as_millis()
                ));
                self.wallpaper = Some(data);
            }
            Err(e) => {
                eprintln!("wallboard: wallpaper: {e}");
                self.wallpaper_failed = true;
                self.background.image_on = false;
            }
        }
    }

    fn draw(&mut self) {
        if !self.configured || self.width == 0 || self.height == 0 {
            return;
        }
        let scale = self.scale.max(1);
        let w = self.width as i32 * scale;
        let h = self.height as i32 * scale;
        self.ensure_wallpaper(w as u32, h as u32);
        let overlay = self.overlay();
        let current = {
            let f = &self.fonts;
            let (vw, vh) = self.view_size();
            layout::current_area(&self.board, &self.camera, vw, vh, &|s, c| {
                f.advance(s.face(), c, s.size())
            })
        };
        let (buffer, canvas) = self
            .pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Xrgb8888)
            .expect("create buffer");

        let scene = render::Scene {
            board: &self.board,
            editor: &self.editor,
            theme: self.theme,
            background: &self.background,
            wallpaper: self.wallpaper.as_deref(),
            overlay: &overlay,
            current_area: current,
            show_help: self.show_help,
        };
        render::draw(
            canvas,
            w as u32,
            h as u32,
            &self.camera,
            scale as u32,
            &scene,
            &mut self.fonts,
        );

        let surface = self.layer.wl_surface();
        let _ = self.layer.set_buffer_scale(scale as u32);
        surface.damage_buffer(0, 0, w, h);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).expect("attach");
        self.layer.commit();

        self.frame_pending = true;
        self.dirty = false;
        self.draws += 1;
        self.last_draw = Instant::now();
        self.arm_shrink();
        if self.debug {
            println!("[draw] #{} {w}x{h} VmRSS = {} KiB", self.draws, vmrss_kib());
            println!("[state] {}", self.state_json(&overlay, current));
        }
    }

    /// Everything a test needs to find controls on screen, as one JSON line.
    fn state_json(&self, ov: &Overlay, current: Option<usize>) -> serde_json::Value {
        let f = &self.fonts;
        let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
        let areas: Vec<_> = self
            .board
            .areas
            .iter()
            .map(|a| {
                let g = area_geom(a, &adv);
                json!({
                    "id": a.id, "name": a.name, "color": a.color, "hero": a.hero, "x": a.x, "y": a.y,
                    "todos": a.todos,
                    "card": rect_json(g.card), "title": rect_json(g.title), "dot": rect_json(g.dot),
                    "footer": rect_json(g.footer),
                    "rows": g.rows.iter().map(|r| json!({
                        "circle": rect_json(r.circle), "text": rect_json(r.text), "prio": rect_json(r.prio),
                        "body": r.body.as_ref().map(|b| rect_json(b.rect)),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let notes: Vec<_> = self
            .board
            .notes
            .iter()
            .map(|n| {
                let g = note_geom(n, &adv);
                json!({
                    "id": n.id, "title": n.title, "body": n.body, "color": n.color, "x": n.x, "y": n.y,
                    "card": rect_json(g.card), "title_rect": rect_json(g.title),
                    "body_rect": rect_json(g.body.rect), "dot": rect_json(g.dot),
                })
            })
            .collect();
        let texts: Vec<_> = self
            .board
            .texts
            .iter()
            .map(|t| {
                let g = layout::text_geom(t, &adv);
                json!({"id": t.id, "text": t.text, "x": t.x, "y": t.y, "rect": rect_json(g.rect)})
            })
            .collect();
        let shapes: Vec<_> = self
            .board
            .shapes
            .iter()
            .map(|s| json!({"id": s.id, "kind": format!("{:?}", s.kind), "x1": s.x1, "y1": s.y1, "x2": s.x2, "y2": s.y2, "color": s.color}))
            .collect();
        let selected = self.editor.selected.map(|sel| {
            let hs: Vec<_> = layout::handles(&self.board, sel, &adv)
                .into_iter()
                .map(|(h, (x, y))| json!({"handle": format!("{h:?}"), "x": x, "y": y}))
                .collect();
            json!({"sel": format!("{sel:?}"), "handles": hs})
        });
        let strokes: Vec<_> = self
            .board
            .strokes
            .iter()
            .map(
                |s| json!({"id": s.id, "n": s.pts.len(), "first": s.pts.first(), "color": s.color}),
            )
            .collect();
        let editing = self
            .editor
            .editing
            .map(|e| json!({"target": format!("{:?}", e.target), "caret": e.caret}));
        json!({
            "areas": areas, "notes": notes, "strokes": strokes, "texts": texts, "shapes": shapes, "selected": selected,
            "tool": format!("{:?}", self.editor.tool),
            "editing": editing,
            "camera": {"x": self.camera.x, "y": self.camera.y, "zoom": self.camera.zoom},
            "theme": format!("{:?}", self.theme),
            "background": {"color": self.background.color, "image_on": self.background.image_on},
            "wallpaper_loaded": self.wallpaper.is_some(),
            "current_area": current,
            "tools": ov.tools.iter().map(|(t, r)| json!({"tool": format!("{t:?}"), "rect": rect_json(*r)})).collect::<Vec<_>>(),
            "view_buttons": ov.view_btns.iter().map(|(b, r)| json!({"button": format!("{b:?}"), "rect": rect_json(*r)})).collect::<Vec<_>>(),
            "chips": ov.chips.iter().map(|(i, r)| json!({"area": i, "rect": rect_json(*r)})).collect::<Vec<_>>(),
            "view": [self.width, self.height],
            "glyphs": self.fonts.cached_glyphs(),
        })
    }

    /// Register the idle timer if it is not already running.
    fn arm_shrink(&mut self) {
        if self.shrink_armed {
            return;
        }
        self.shrink_armed = true;
        self.loop_handle
            .insert_source(Timer::from_duration(IDLE_SHRINK), |_, _, app: &mut App| {
                let idle = app.last_draw.elapsed();
                if idle < IDLE_SHRINK {
                    return TimeoutAction::ToDuration(IDLE_SHRINK - idle);
                }
                app.shrink();
                app.shrink_armed = false;
                TimeoutAction::Drop
            })
            .expect("insert shrink timer");
    }

    /// Replace the pool by one sized for a single buffer, drop the wallpaper cache, and redraw
    /// into the new pool. This frees the spare buffer the compositor no longer needs.
    fn shrink(&mut self) {
        if !self.configured || self.width == 0 || self.height == 0 {
            return;
        }
        let scale = self.scale.max(1) as usize;
        let bytes = self.width as usize * self.height as usize * scale * scale * 4;
        let Ok(pool) = SlotPool::new(bytes, &self.shm) else {
            return;
        };
        self.pool = pool;
        self.frame_pending = false;
        self.draw();
        // The redraw above used the wallpaper cache; do not hold it while idle (rebuilt on demand).
        self.wallpaper = None;
        self.log(format_args!("[shrink] VmRSS = {} KiB", vmrss_kib()));
    }

    // ---- persistence

    /// Schedule a debounced save (content or view changed).
    fn mark_dirty_for_save(&mut self) {
        self.save_due = Some(Instant::now() + SAVE_DEBOUNCE);
        self.arm_save();
    }

    fn arm_save(&mut self) {
        if self.save_armed {
            return;
        }
        self.save_armed = true;
        self.loop_handle
            .insert_source(
                Timer::from_duration(SAVE_DEBOUNCE),
                |_, _, app: &mut App| loop {
                    match app.save_due {
                        Some(due) if Instant::now() < due => {
                            return TimeoutAction::ToDuration(due - Instant::now());
                        }
                        Some(_) => {
                            app.save();
                            // A failed save sets a retry time; otherwise we are done.
                            if app.save_due.is_none() {
                                app.save_armed = false;
                                return TimeoutAction::Drop;
                            }
                        }
                        None => {
                            app.save_armed = false;
                            return TimeoutAction::Drop;
                        }
                    }
                },
            )
            .expect("insert save timer");
    }

    fn save(&mut self) {
        self.save_due = None;
        let view = View {
            // Only persist the camera once it is real (after the first configure).
            camera: self.configured.then_some(self.camera),
            theme: self.theme,
            background: self.background.clone(),
            margins: self.margins,
            backup: self.backup_settings,
        };
        match store::save(&self.data_path, &self.board, &view) {
            Ok(()) => {
                self.log(format_args!(
                    "[save] ok areas={} VmRSS = {} KiB",
                    self.board.areas.len(),
                    vmrss_kib()
                ));
                // Only content changes (not pan/zoom/theme) are worth a git commit.
                let hash = store::content_hash(&self.board);
                if hash != self.last_hash {
                    self.last_hash = hash;
                    if let Some(b) = &self.backup {
                        b.notify();
                    }
                }
            }
            Err(e) => {
                eprintln!("wallboard: save failed: {e} (retrying in 5 s)");
                self.save_due = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    /// The editor changed something: schedule a save if content changed, then redraw.
    fn after_edit(&mut self) {
        let content = self.editor.take_modified();
        let view = self.editor.take_view_changed();
        if content || view {
            self.mark_dirty_for_save();
        }
        self.request_redraw();
    }

    // ---- navigation

    fn jump_to_area(&mut self, i: usize) {
        let Some(a) = self.board.areas.get(i) else {
            return;
        };
        let f = &self.fonts;
        let g = area_geom(a, &|s, c| f.advance(s.face(), c, s.size()));
        let (vw, vh) = self.view_size();
        self.camera.focus(
            (g.card.x, g.card.y, g.card.w, g.card.h),
            vw,
            vh,
            JUMP_MAX_ZOOM,
        );
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    fn fit_all(&mut self) {
        let f = &self.fonts;
        let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
        let (vw, vh) = self.view_size();
        if let Some(b) = layout::bounds(&self.board, &adv) {
            self.camera
                .fit((b.x, b.y, b.w, b.h), vw, vh, FIT_MARGIN, JUMP_MAX_ZOOM);
        } else {
            self.camera = Camera::centered(vw, vh);
        }
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    fn zoom_center(&mut self, factor: f32) {
        let (vw, vh) = self.view_size();
        self.camera.zoom_at(vw / 2.0, vh / 2.0, factor);
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    /// `C`: a new todo in Inbox, from anywhere.
    fn capture(&mut self) {
        if self.board.areas.is_empty() {
            let id = self.board.add_area(0.0, 0.0);
            if let Some(a) = self.board.area_mut(id) {
                a.name = "Inbox".into();
            }
        }
        let Some(ai) = self.board.inbox_index() else {
            return;
        };
        if self.editor.add_todo(&mut self.board, ai) {
            self.jump_to_area(ai);
            self.after_edit();
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        self.editor.set_tool(&mut self.board, tool);
        self.after_edit();
    }

    fn toggle_theme(&mut self) {
        self.theme = self.theme.toggled();
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    fn cycle_background(&mut self) {
        let have = self.wallpaper_path().is_some();
        self.background.cycle(have);
        self.wallpaper_failed = false;
        self.wallpaper = None;
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    // ---- keyboard

    fn to_key(&self, ev: &KeyEvent) -> Option<Key> {
        let ctrl = self.mods.ctrl;
        Some(match ev.keysym {
            Keysym::BackSpace => Key::Backspace,
            Keysym::Delete => Key::Delete,
            Keysym::Return | Keysym::KP_Enter if ctrl => Key::ToggleDone,
            Keysym::Return | Keysym::KP_Enter => Key::Enter,
            Keysym::Tab if !self.mods.shift => Key::Tab,
            Keysym::Tab | Keysym::ISO_Left_Tab => Key::BackTab,
            Keysym::Left => Key::Left,
            Keysym::Right => Key::Right,
            Keysym::Up => Key::Up,
            Keysym::Down => Key::Down,
            Keysym::Home => Key::Home,
            Keysym::End => Key::End,
            Keysym::Escape => Key::Escape,
            Keysym::p | Keysym::P if ctrl => Key::CyclePriority,
            _ => {
                if ctrl || self.mods.alt || self.mods.logo {
                    return None;
                }
                let s = ev.utf8.as_ref()?;
                if s.is_empty() || s.chars().any(|c| c.is_control()) {
                    return None;
                }
                Key::Text(s.clone())
            }
        })
    }

    /// Handle a key press or repeat. Typing always wins while a field is being edited.
    fn handle_key(&mut self, ev: KeyEvent, repeat: bool) {
        let ctrl = self.mods.ctrl;
        if ctrl {
            match ev.keysym {
                Keysym::q | Keysym::Q => {
                    if !repeat {
                        self.exit = true;
                    }
                    return;
                }
                Keysym::z | Keysym::Z | Keysym::y | Keysym::Y => {
                    let redo = matches!(ev.keysym, Keysym::y | Keysym::Y) || self.mods.shift;
                    let changed = if redo {
                        self.editor.redo(&mut self.board)
                    } else {
                        self.editor.undo(&mut self.board)
                    };
                    if changed {
                        self.after_edit();
                    }
                    return;
                }
                Keysym::g | Keysym::G if self.editor.editing.is_none() => {
                    if !repeat {
                        let changed = if self.mods.shift {
                            self.editor.ungroup_selected(&mut self.board)
                        } else {
                            self.editor.group_selected(&mut self.board)
                        };
                        if changed {
                            self.after_edit();
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        if self.editor.editing.is_some() {
            if let Some(k) = self.to_key(&ev) {
                let f = &self.fonts;
                let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
                if self.editor.key(&mut self.board, k, &adv) {
                    self.after_edit();
                }
            }
            return;
        }
        if repeat || ctrl || self.mods.alt || self.mods.logo {
            return;
        }
        match ev.keysym {
            Keysym::_1
            | Keysym::_2
            | Keysym::_3
            | Keysym::_4
            | Keysym::_5
            | Keysym::_6
            | Keysym::_7
            | Keysym::_8
            | Keysym::_9 => {
                let n = ev.keysym.raw() - Keysym::_1.raw();
                self.jump_to_area(n as usize);
            }
            Keysym::_0 | Keysym::KP_0 => self.fit_all(),
            Keysym::plus | Keysym::equal | Keysym::KP_Add => self.zoom_center(1.2),
            Keysym::minus | Keysym::KP_Subtract => self.zoom_center(1.0 / 1.2),
            Keysym::c | Keysym::C => self.capture(),
            Keysym::l | Keysym::L => self.toggle_theme(),
            Keysym::b | Keysym::B => self.cycle_background(),
            Keysym::question => self.show_help = !self.show_help,
            Keysym::Escape if self.show_help => self.show_help = false,
            Keysym::Escape => {
                // Esc always goes back to plain selecting: drop the selection and any armed tool.
                self.editor.deselect();
                self.set_tool(Tool::Select);
            }
            Keysym::Delete | Keysym::BackSpace => {
                if self.editor.delete_selected(&mut self.board) {
                    self.after_edit();
                }
            }
            Keysym::v | Keysym::V => self.set_tool(Tool::Select),
            Keysym::x | Keysym::X => self.set_tool(Tool::Text),
            Keysym::r | Keysym::R => self.set_tool(Tool::Rect),
            Keysym::o | Keysym::O => self.set_tool(Tool::Ellipse),
            Keysym::i | Keysym::I => self.set_tool(Tool::Line),
            Keysym::g | Keysym::G => self.set_tool(Tool::Group),
            Keysym::t | Keysym::T => self.set_tool(Tool::Todo),
            Keysym::n | Keysym::N => self.set_tool(Tool::Note),
            Keysym::p | Keysym::P => self.set_tool(Tool::Pen),
            Keysym::e | Keysym::E => self.set_tool(Tool::Eraser),
            _ => {}
        }
    }

    // ---- pointer

    /// A left click on the screen-space overlay. Returns true if it was handled there.
    fn overlay_click(&mut self, p: (f32, f32)) -> bool {
        if self.show_help {
            // Any click closes the popup and is swallowed.
            self.show_help = false;
            return true;
        }
        let ov = self.overlay();
        if let Some(btn) = ov.view_at(p.0, p.1) {
            match btn {
                ViewBtn::Help => self.show_help = true,
                ViewBtn::ZoomIn => self.zoom_center(1.2),
                ViewBtn::ZoomOut => self.zoom_center(1.0 / 1.2),
                ViewBtn::Background => self.cycle_background(),
            }
            return true;
        }
        if let Some(tool) = ov.tool_at(p.0, p.1) {
            self.set_tool(tool);
            return true;
        }
        if let Some(i) = ov.chip_at(p.0, p.1) {
            self.jump_to_area(i);
            return true;
        }
        ov.covers(p.0, p.1) // clicks on a pill's padding go nowhere
    }

    fn handle_axis(&mut self, h: AxisScroll, v: AxisScroll) {
        let px = |a: AxisScroll, notch: f32| {
            if a.value120 != 0 {
                a.value120 as f32 / 120.0 * notch
            } else {
                a.absolute as f32
            }
        };
        let (cx, cy) = self.cursor;
        let (dx, dy) = (px(h, PAN_PER_NOTCH), px(v, PAN_PER_NOTCH));
        if self.mods.ctrl {
            // Ctrl+scroll zooms around the cursor (scroll up = zoom in).
            let notches = -px(v, PX_PER_NOTCH) / PX_PER_NOTCH;
            if notches != 0.0 {
                self.camera.zoom_at(cx, cy, ZOOM_PER_NOTCH.powf(notches));
            }
        } else if self.mods.shift {
            self.camera.pan_by_screen(-(dy + dx), 0.0);
        } else {
            self.camera.pan_by_screen(-dx, -dy);
        }
        self.mark_dirty_for_save();
        self.request_redraw();
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.log(format_args!(
            "[scale] preferred buffer scale = {new_factor}"
        ));
        if new_factor != self.scale {
            self.scale = new_factor;
            self.wallpaper = None;
            self.request_redraw();
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.dirty {
            self.draw();
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, o: wl_output::WlOutput) {
        if let Some(info) = self.output_state.info(&o) {
            self.log(format_args!(
                "[output] new {:?} logical_size={:?} scale={}",
                info.name, info.logical_size, info.scale_factor
            ));
        }
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        self.log(format_args!("[configure] new_size = ({w}, {h})"));
        let first = !self.configured;
        let resized = (self.width, self.height) != (w, h);
        self.width = w;
        self.height = h;
        self.configured = true;
        if resized {
            self.wallpaper = None;
        }
        if first && !self.camera_loaded {
            // First run: start on the hero (Focus) card, else show everything.
            match self.board.areas.iter().position(|a| a.hero) {
                Some(i) => self.jump_to_area(i),
                None => self.fit_all(),
            }
        }
        self.request_redraw();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            // Key repeat is driven by the calloop loop (needed for held Backspace/arrows).
            let kb = self
                .seat_state
                .get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.loop_handle.clone(),
                    Box::new(|app: &mut App, _kb, ev| app.handle_key(ev, true)),
                )
                .expect("keyboard");
            self.keyboard = Some(kb);
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = Some(self.seat_state.get_pointer(qh, &seat).expect("pointer"));
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if self.layer.wl_surface() == surface {
            self.log(format_args!("[focus] keyboard ENTER"));
        }
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self.layer.wl_surface() == surface {
            self.log(format_args!("[focus] keyboard LEAVE"));
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        ev: KeyEvent,
    ) {
        self.handle_key(ev, false);
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        m: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.mods = m;
        self.editor.shift = m.shift;
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut redraw = false;
        let mut edited = false;
        for e in events {
            if &e.surface != self.layer.wl_surface() {
                continue;
            }
            let pos = (e.position.0 as f32, e.position.1 as f32);
            match e.kind {
                PointerEventKind::Enter { .. } => self.cursor = pos,
                PointerEventKind::Leave { .. } => {}
                PointerEventKind::Motion { .. } => {
                    self.cursor = pos;
                    if self.editor.is_dragging() {
                        edited |= self.editor.motion(&mut self.board, &mut self.camera, pos);
                    }
                }
                PointerEventKind::Press { button, time, .. } => {
                    self.cursor = pos;
                    self.log(format_args!(
                        "[press] button=0x{button:x} at ({:.0},{:.0})",
                        pos.0, pos.1
                    ));
                    let b = match button {
                        BTN_LEFT => Some(Button::Left),
                        BTN_RIGHT => Some(Button::Right),
                        BTN_MIDDLE => Some(Button::Middle),
                        _ => None,
                    };
                    if let Some(b) = b {
                        if b == Button::Left && self.overlay_click(pos) {
                            redraw = true;
                        } else if !self.overlay().covers(pos.0, pos.1) {
                            let f = &self.fonts;
                            let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
                            edited |= self.editor.press(
                                &mut self.board,
                                &self.camera,
                                pos,
                                b,
                                time,
                                &adv,
                            );
                        }
                    }
                }
                PointerEventKind::Release { .. } => {
                    let f = &self.fonts;
                    let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
                    edited |= self.editor.release(&mut self.board, &adv);
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    self.cursor = pos;
                    self.handle_axis(horizontal, vertical);
                }
            }
        }
        if edited {
            self.after_edit();
        } else if redraw {
            self.request_redraw();
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);

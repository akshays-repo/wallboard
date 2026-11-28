//! wallboard: a life-OS home screen on the niri background layer.
//!
//! Canvas: scroll, middle-drag, Space+drag or the arrow keys pan; Ctrl+scroll zooms at the
//! cursor; dragging empty space draws a selection box.
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
mod detail;
mod editor;
mod layout;
mod md;
mod pics;
mod render;
mod store;
mod sync;
mod text;

use board::{Background, Board, Theme};
use camera::Camera;
use editor::{Button, Editor, Key};
use layout::{area_geom, note_geom, FmtBtn, Hover, Margins, Overlay, Rect, Tool, ViewBtn};
use serde_json::json;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    data_device_manager::{
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        data_source::{CopyPasteSource, DataSourceHandler},
        DataDeviceManagerState, WritePipe,
    },
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

/// How long a theme switch (or hover preview) takes to glide.
const THEME_ANIM: Duration = Duration::from_millis(220);
use store::{Loaded, View};
use text::{Fonts, Style};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{
        wl_data_device::WlDataDevice, wl_data_device_manager::DndAction,
        wl_data_source::WlDataSource, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm,
        wl_surface,
    },
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
/// Screen px an arrow key pans per press or repeat (Shift: this many times further).
const PAN_PER_KEY: f32 = 80.0;
const PAN_KEY_FAST: f32 = 4.0;
/// Margin (screen px) around everything when fitting the view.
const FIT_MARGIN: f32 = 60.0;
/// Do not zoom in past this when jumping to an area.
const JUMP_MAX_ZOOM: f32 = 1.2;
/// Wait this long after the last change before writing the save file.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// After this long without drawing, drop the spare shm buffer and the wallpaper cache.
const IDLE_SHRINK: Duration = Duration::from_secs(2);

/// Resident set size in KiB, from /proc/self/status.
/// Open a folder with the desktop's file manager, detached.
fn open_in_file_manager(dir: &std::path::Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

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

/// Decode %XX escapes in a file:// URI path.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    // Without a data device manager the board still works, just without copy / paste.
    let data_mgr = DataDeviceManagerState::bind(&globals, &qh).ok();

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

    // Messages from helper threads (the folder picker) back into the event loop.
    let (pick_tx, pick_rx) =
        smithay_client_toolkit::reexports::calloop::channel::channel::<String>();
    event_loop
        .handle()
        .insert_source(pick_rx, |ev, _, app: &mut App| {
            if let smithay_client_toolkit::reexports::calloop::channel::Event::Msg(dir) = ev {
                app.set_wallpaper_folder(dir);
            }
        })
        .expect("insert picker channel");

    let mut app = App {
        pick_tx,
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        conn: conn.clone(),
        data_mgr,
        data_device: None,
        copy_source: None,
        clip_text: String::new(),
        clip_box: None,
        key_serial: 0,
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
        anim_from: render::colors(view.theme),
        anim_to: view.theme,
        anim_t0: Instant::now() - THEME_ANIM,
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
        detail: None,
        chip_first: 0,
        name_box: None,
        last_chip_click: None,
        theme_open: false,
        hover: Hover::None,
        dirty: false,
        frame_pending: false,
        last_draw: Instant::now(),
        shrink_armed: false,
        rotate_armed: false,
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
    /// The todo detail page (title, description, due date), when open.
    detail: Option<detail::Detail>,
    /// The popup that names a new space or renames one, when open.
    name_box: Option<detail::NameBox>,
    /// First space shown in the top menu (it scrolls when there are too many to fit).
    chip_first: usize,
    /// (time, area index) of the last click on a space in the menu, to spot a double-click.
    last_chip_click: Option<(u32, usize)>,
    /// The theme panel (right sidebar) is open.
    theme_open: bool,
    /// What the pointer is over in the overlay.
    hover: Hover,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    /// Clipboard: the manager, the per-seat device, our current copy source and its text.
    conn: Connection,
    data_mgr: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    copy_source: Option<CopyPasteSource>,
    clip_text: String,
    /// The text box copied with Ctrl+C (pasted with its style while we still own the clipboard).
    clip_box: Option<board::TextBox>,
    /// Serial of the latest key press (the compositor wants it when we set the selection).
    key_serial: u32,
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
    /// Theme switch animation: blend from `anim_from` to `colors(anim_to)` since `anim_t0`.
    anim_from: render::Colors,
    anim_to: Theme,
    anim_t0: Instant,
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
    /// A wallpaper-rotation timer is registered.
    rotate_armed: bool,
    /// Folder chosen in the picker dialog arrives here.
    pick_tx: smithay_client_toolkit::reexports::calloop::channel::Sender<String>,
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
        layout::overlay_from(
            &self.board,
            vw,
            vh,
            self.margins,
            &|s, t| f.text_width(s.face(), t, s.size()),
            self.chip_first,
        )
    }

    /// The theme on screen: the one under the pointer in the open picker (a live preview), else
    /// the chosen one.
    fn shown_theme(&self) -> Theme {
        match self.hover {
            Hover::Theme(t) if self.theme_open => t,
            _ => self.theme,
        }
    }

    /// The palette for this frame, eased toward `shown_theme`, and whether it is still moving.
    fn palette_now(&mut self) -> (render::Colors, bool) {
        let now = Instant::now();
        let target = self.shown_theme();
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        let progress = |t0: Instant| {
            (now.saturating_duration_since(t0).as_secs_f32() / THEME_ANIM.as_secs_f32()).min(1.0)
        };
        if target != self.anim_to {
            // Retarget from wherever the blend currently is, so reversing mid-glide is smooth.
            let cur = render::lerp_colors(
                &self.anim_from,
                &render::colors(self.anim_to),
                ease(progress(self.anim_t0)),
            );
            self.anim_from = cur;
            self.anim_to = target;
            self.anim_t0 = now;
        }
        let t = progress(self.anim_t0);
        let c = render::lerp_colors(&self.anim_from, &render::colors(self.anim_to), ease(t));
        (c, t < 1.0)
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
        let (palette, animating) = self.palette_now();
        let overlay = self.overlay();
        let text_bar = self.text_bar();
        let current = self.active_space();
        let (buffer, canvas) = self
            .pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Xrgb8888)
            .expect("create buffer");

        let scene = render::Scene {
            text_bar: text_bar.as_ref(),
            board: &self.board,
            editor: &self.editor,
            theme: self.theme,
            palette: Some(palette),
            background: &self.background,
            wallpaper: self.wallpaper.as_deref(),
            overlay: &overlay,
            current_area: current,
            show_help: self.show_help,
            detail: self.detail.as_ref(),
            name_box: self.name_box.as_ref(),
            theme_open: self.theme_open,
            hover: self.hover,
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
        // While a theme glide is running, ask for the next frame; idle again once it ends.
        self.dirty = animating;
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
                json!({"id": t.id, "text": t.text, "x": t.x, "y": t.y, "rect": rect_json(g.rect),
                    "size": format!("{:?}", t.size), "bold": t.bold, "italic": t.italic,
                    "underline": t.underline, "strike": t.strike, "plain": t.plain, "color": t.color})
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
            "background": {"color": self.background.color, "image_on": self.background.image_on, "image": self.background.image, "rotate_mins": self.background.rotate_mins, "random": self.background.random},
            "wallpaper_loaded": self.wallpaper.is_some(),
            "current_space": current, "spaces": self.board.spaces.iter().map(|s| json!({"id": s.id, "name": s.name, "x": s.x, "y": s.y, "zoom": s.zoom})).collect::<Vec<_>>(),
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
        self.fonts.pics.clear();
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

    /// Scroll the space menu so area `i` is visible.
    fn reveal_chip(&mut self, i: usize) {
        if i < self.chip_first {
            self.chip_first = i;
        }
        for _ in 0..self.board.spaces.len() {
            if self.overlay().chips.iter().any(|(k, _)| *k == i) || self.chip_first >= i {
                break;
            }
            self.chip_first += 1;
        }
    }

    /// Go to a saved space: point the camera at it.
    fn jump_to_space(&mut self, i: usize) {
        self.reveal_chip(i);
        let Some(sp) = self.board.spaces.get(i) else {
            return;
        };
        let (vw, vh) = self.view_size();
        layout::show_space(&mut self.camera, sp, vw, vh);
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    /// The space being shown right now, if the view matches one.
    fn active_space(&self) -> Option<usize> {
        let (vw, vh) = self.view_size();
        layout::active_space(&self.board, &self.camera, vw, vh)
    }

    /// Ctrl+Left / Ctrl+Right: go to the previous / next space.
    fn step_space(&mut self, delta: i32) {
        let n = self.board.spaces.len() as i32;
        if n == 0 {
            return;
        }
        let target = match self.active_space() {
            Some(i) => (i as i32 + delta).clamp(0, n - 1),
            None if delta > 0 => 0,
            None => n - 1,
        };
        self.jump_to_space(target as usize);
    }

    /// The "+" button: ask for a name, then save the current view as a space.
    fn add_space(&mut self) {
        let (vw, vh) = self.view_size();
        let c = self.camera.screen_to_world(vw / 2.0, vh / 2.0);
        let name = format!("Space {}", self.board.spaces.len() + 1);
        self.name_box = Some(detail::NameBox::new(None, (c, self.camera.zoom), &name));
        self.request_redraw();
    }

    /// Double-click on a space in the menu: rename it.
    fn rename_space(&mut self, i: usize) {
        if let Some(sp) = self.board.spaces.get(i) {
            self.name_box = Some(detail::NameBox::new(
                Some(sp.id),
                ((sp.x, sp.y), sp.zoom),
                &sp.name.clone(),
            ));
            self.request_redraw();
        }
    }

    fn close_name_box(&mut self, save: bool) {
        if let Some(n) = self.name_box.take() {
            let name = n.text.trim().to_string();
            if save && !name.is_empty() {
                match n.space {
                    Some(id) => {
                        self.editor.rename_space(&mut self.board, id, &name);
                    }
                    None => {
                        let id = self
                            .editor
                            .add_space(&mut self.board, &name, n.view.0, n.view.1);
                        if let Some(i) = self.board.spaces.iter().position(|s| s.id == id) {
                            self.reveal_chip(i);
                        }
                    }
                }
                self.after_edit();
            }
        }
        self.request_redraw();
    }

    fn name_box_key(&mut self, ev: &KeyEvent) {
        let key = if self.mods.ctrl && matches!(ev.keysym, Keysym::v | Keysym::V) {
            self.clipboard_text()
                .map(|t| Key::Text(t.replace("\r\n", " ").replace(['\r', '\n'], " ")))
        } else {
            self.to_key(ev)
        };
        let Some(key) = key else { return };
        let Some(n) = self.name_box.as_mut() else {
            return;
        };
        match n.key(key) {
            detail::Outcome::Keep => self.request_redraw(),
            detail::Outcome::Cancel => self.close_name_box(false),
            detail::Outcome::Save => self.close_name_box(true),
        }
    }

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

    /// Move the content by a screen-space distance.
    fn pan_view(&mut self, dx: f32, dy: f32) {
        self.camera.pan_by_screen(dx, dy);
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
        if tool == Tool::Image {
            // A button, not a mode: paste the clipboard image now.
            self.editor.set_tool(&mut self.board, Tool::Select);
            self.paste_image();
            self.after_edit();
            return;
        }
        self.editor.set_tool(&mut self.board, tool);
        self.after_edit();
    }

    fn toggle_theme(&mut self) {
        self.set_theme(self.theme.toggled());
    }

    fn set_theme(&mut self, t: Theme) {
        self.theme = t;
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    fn toggle_theme_panel(&mut self) {
        self.theme_open = !self.theme_open;
        self.request_redraw();
    }

    /// The wallpaper folder: the configured one, else `wallpapers/` in the data directory.
    fn wallpaper_dir(&self) -> std::path::PathBuf {
        match &self.background.folder {
            Some(f) if !f.trim().is_empty() => std::path::PathBuf::from(f),
            _ => store::data_dir().join("wallpapers"),
        }
    }

    /// The `F` key: pick the wallpaper folder in a dialog (zenity); without zenity, open the
    /// current folder in the file manager instead. Runs off-thread so the board stays live.
    fn pick_wallpaper_folder(&self) {
        let dir = self.wallpaper_dir();
        let _ = std::fs::create_dir_all(&dir);
        let tx = self.pick_tx.clone();
        std::thread::spawn(move || {
            let start = format!("{}/", dir.display());
            let picked = std::process::Command::new("zenity")
                .args([
                    "--file-selection",
                    "--directory",
                    "--title=Wallpaper folder",
                ])
                .arg(format!("--filename={start}"))
                .output();
            match picked {
                Ok(o) if o.status.success() => {
                    let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !p.is_empty() {
                        let _ = tx.send(p);
                    }
                }
                Ok(_) => {} // dialog cancelled
                Err(_) => open_in_file_manager(&dir),
            }
        });
    }

    /// The `Shift+F` key: open the wallpaper folder in the file manager.
    fn open_wallpaper_folder(&self) {
        let dir = self.wallpaper_dir();
        let _ = std::fs::create_dir_all(&dir);
        open_in_file_manager(&dir);
    }

    /// A folder was chosen: use it and show its first wallpaper.
    fn set_wallpaper_folder(&mut self, dir: String) {
        self.background.folder = Some(dir);
        self.background.image = None;
        let images = self.wallpaper_list();
        self.log(format_args!(
            "[wallpaper] folder {:?}: {} images",
            self.background.folder,
            images.len()
        ));
        if let Some(first) = images.first() {
            self.background.image_on = true;
            self.background.color = None;
            self.background.image = Some(first.clone());
        } else {
            eprintln!("wallboard: no PNG files in the chosen wallpaper folder");
        }
        self.wallpaper_changed();
    }

    /// Every wallpaper on offer, in cycle order: the configured image or `wallpaper.png` (when
    /// it is not already in the folder), then the PNGs in `wallpapers/` sorted by name.
    /// Scanned on demand; nothing is kept between calls.
    fn wallpaper_list(&self) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(self.wallpaper_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        found.sort();
        if let Some(extra) = self.wallpaper_path() {
            if !found.contains(&extra) {
                found.insert(0, extra);
            }
        }
        found
    }

    fn cycle_background(&mut self) {
        let images = self.wallpaper_list();
        self.background.cycle(&images);
        self.wallpaper_changed();
    }

    /// The `W` key: wallpaper rotation off -> in order -> random -> off.
    fn cycle_rotation(&mut self) {
        self.background.cycle_rotation();
        if self.background.rotate_mins > 0 && !self.background.image_on {
            // Turning rotation on should show a wallpaper straight away.
            let images = self.wallpaper_list();
            if let Some(first) = images.first() {
                self.background.image_on = true;
                self.background.color = None;
                self.background.image.get_or_insert_with(|| first.clone());
            }
        }
        self.log(format_args!(
            "[rotate] every {} min, random={}",
            self.background.rotate_mins, self.background.random
        ));
        self.wallpaper_changed();
    }

    /// The wallpaper choice changed: drop the cache, save, redraw, (re)arm rotation.
    fn wallpaper_changed(&mut self) {
        self.wallpaper_failed = false;
        self.wallpaper = None;
        self.arm_rotate();
        self.mark_dirty_for_save();
        self.request_redraw();
    }

    /// Register the rotation timer if rotation is on and it is not already running. The timer
    /// fires once per interval, so it costs nothing in between.
    fn arm_rotate(&mut self) {
        if self.rotate_armed || self.background.rotate_mins == 0 || !self.background.image_on {
            return;
        }
        self.rotate_armed = true;
        let first = Duration::from_secs(self.background.rotate_mins as u64 * 60);
        self.loop_handle
            .insert_source(Timer::from_duration(first), |_, _, app: &mut App| {
                if app.background.rotate_mins == 0 || !app.background.image_on {
                    app.rotate_armed = false;
                    return TimeoutAction::Drop;
                }
                let images = app.wallpaper_list();
                let rnd = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as u64);
                if app.background.advance(&images, rnd) {
                    app.wallpaper_failed = false;
                    app.wallpaper = None;
                    app.mark_dirty_for_save();
                    app.request_redraw();
                }
                TimeoutAction::ToDuration(Duration::from_secs(
                    app.background.rotate_mins as u64 * 60,
                ))
            })
            .expect("insert rotate timer");
    }

    // ---- clipboard

    /// Copy the selected text to the clipboard (and remove it when `cut`).
    fn copy_selection(&mut self, cut: bool) {
        let Some(text) = self.editor.selected_text(&self.board) else {
            return;
        };
        self.claim_clipboard(text);
        self.clip_box = None;
        if cut {
            let f = &self.fonts;
            let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
            if self.editor.key(&mut self.board, Key::Cut, &adv) {
                self.after_edit();
            }
        }
    }

    /// Ctrl+C / Ctrl+X on a selected text box: copy the whole box (its text goes to the system
    /// clipboard too, so other apps can paste it).
    fn copy_text_box(&mut self, cut: bool) {
        let Some(t) = self
            .editor
            .text_focus()
            .and_then(|id| self.board.text(id))
            .cloned()
        else {
            return;
        };
        self.claim_clipboard(t.text.clone());
        self.clip_box = Some(t);
        if cut && self.editor.delete_selected(&mut self.board) {
            self.after_edit();
        }
    }

    /// Ctrl+D on a selected text box: a copy right next to it.
    fn duplicate_text_box(&mut self) {
        let Some(t) = self
            .editor
            .text_focus()
            .and_then(|id| self.board.text(id))
            .cloned()
        else {
            return;
        };
        let at = (t.x + 24.0, t.y + 24.0);
        self.editor
            .paste_text_box(&mut self.board, Some(&t), "", at);
        self.after_edit();
    }

    /// Own the system clipboard with `text`.
    fn claim_clipboard(&mut self, text: String) {
        if let (Some(mgr), Some(dev)) = (&self.data_mgr, &self.data_device) {
            let src = mgr.create_copy_paste_source(
                &self.qh,
                [
                    "text/plain;charset=utf-8",
                    "text/plain",
                    "UTF8_STRING",
                    "STRING",
                ],
            );
            src.set_selection(dev, self.key_serial);
            self.copy_source = Some(src);
        }
        self.clip_text = text;
    }

    /// Where a paste lands: under the pointer when it is on the canvas, else the view centre.
    fn paste_point(&self) -> (f32, f32) {
        let (vw, vh) = self.view_size();
        let (cx, cy) = self.cursor;
        let on_canvas =
            cx > 0.0 && cy > 0.0 && cx < vw && cy < vh && !self.overlay().covers(cx, cy);
        let p = if on_canvas {
            (cx, cy)
        } else {
            (vw / 2.0, vh / 2.0)
        };
        self.camera.screen_to_world(p.0, p.1)
    }

    /// Ctrl+V with nothing being edited: a copied text box (with its style), an image, or plain
    /// text as a new text box.
    fn paste_board(&mut self) {
        let at = self.paste_point();
        if self.copy_source.is_some() {
            if let Some(t) = self.clip_box.clone().filter(|t| t.text == self.clip_text) {
                self.editor
                    .paste_text_box(&mut self.board, Some(&t), "", at);
                self.after_edit();
                return;
            }
        }
        if self.paste_image() {
            return;
        }
        let text = self
            .clipboard_text()
            .map(|t| t.replace("\r\n", "\n").replace('\r', "\n"))
            .filter(|t| !t.trim().is_empty());
        match text {
            Some(text) => {
                let text = text.trim_end_matches('\n').to_string();
                self.editor.paste_text_box(&mut self.board, None, &text, at);
                self.after_edit();
            }
            None => self.log(format_args!("[paste] nothing to paste")),
        }
    }

    /// Read the clipboard owner's data in the first of `wanted` it offers: (mime, bytes).
    fn read_offer(&self, wanted: &[&str]) -> Option<(String, Vec<u8>)> {
        let offer = self.data_device.as_ref()?.data().selection_offer()?;
        let mime = offer.with_mime_types(|m| {
            wanted
                .iter()
                .find(|w| m.iter().any(|x| x == *w))
                .map(|s| s.to_string())
        })?;
        let mut pipe = offer.receive(mime.clone()).ok()?;
        // The request must reach the compositor before we wait for the owner's reply.
        let _ = self.conn.flush();
        // Read off-thread so a stuck clipboard owner cannot freeze the board.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        let buf = rx.recv_timeout(Duration::from_millis(1500)).ok()?;
        Some((mime, buf))
    }

    /// The clipboard's text: ours if we own the selection, otherwise read from the owner.
    fn clipboard_text(&self) -> Option<String> {
        if self.copy_source.is_some() {
            return Some(self.clip_text.clone());
        }
        let (_, buf) = self.read_offer(&[
            "text/plain;charset=utf-8",
            "UTF8_STRING",
            "text/plain",
            "STRING",
        ])?;
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Ctrl+V with nothing being edited: paste a copied image or an image file onto the board.
    fn paste_image(&mut self) -> bool {
        let png = if self.copy_source.is_some() {
            None
        } else {
            self.read_offer(&["image/png"]).map(|(_, b)| b)
        };
        let bytes = png.or_else(|| {
            // A copied file (file manager) or a typed-out path.
            let (_, raw) = if self.copy_source.is_some() {
                ("".to_string(), self.clip_text.clone().into_bytes())
            } else {
                self.read_offer(&["text/uri-list", "text/plain;charset=utf-8", "text/plain"])?
            };
            let text = String::from_utf8_lossy(&raw).into_owned();
            let line = text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))?;
            let path = line
                .strip_prefix("file://")
                .map_or(line.to_string(), percent_decode);
            std::fs::read(path).ok()
        });
        let Some(bytes) = bytes else {
            self.log(format_args!("[paste] no image on the clipboard"));
            return false;
        };
        match pics::import_png(&bytes) {
            Ok((name, w, h)) => {
                let c = self
                    .camera
                    .screen_to_world(self.width as f32 / 2.0, self.height as f32 / 2.0);
                self.editor.add_image(&mut self.board, &name, (w, h), c);
                self.after_edit();
                true
            }
            Err(e) => {
                eprintln!("wallboard: paste: {e} (only PNG images are supported)");
                false
            }
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clipboard_text().filter(|t| !t.is_empty()) else {
            return;
        };
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let f = &self.fonts;
        let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
        if self.editor.key(&mut self.board, Key::Text(text), &adv) {
            self.after_edit();
        }
    }

    // ---- todo detail page

    /// Open the detail page for the todo being edited (Ctrl+D). Does nothing otherwise.
    fn open_detail(&mut self) {
        use editor::EditTarget::{TodoBody, TodoText};
        let Some((area, row)) = self.editor.editing.and_then(|e| match e.target {
            TodoText(a, r) | TodoBody(a, r) => Some((a, r)),
            _ => None,
        }) else {
            return;
        };
        self.detail = detail::Detail::open(&self.board, area, row);
        if self.detail.is_some() {
            self.editor.editing = None;
        }
        self.request_redraw();
    }

    /// Close the detail page, saving the edits when `save`.
    fn close_detail(&mut self, save: bool) {
        if let Some(d) = self.detail.take() {
            if save && d.can_save() && d.differs(&self.board) {
                self.editor.board_changed_externally(&self.board);
                d.apply(&mut self.board);
                self.after_edit();
            }
        }
        self.request_redraw();
    }

    fn detail_key(&mut self, ev: &KeyEvent) {
        let pasting = self.mods.ctrl && matches!(ev.keysym, Keysym::v | Keysym::V);
        let key = if pasting {
            self.clipboard_text()
                .map(|t| Key::Text(t.replace("\r\n", "\n").replace('\r', "\n")))
        } else {
            self.to_key(ev)
        };
        // Ctrl+B / I / E / Shift+S: Markdown on the word at the caret.
        if self.mods.ctrl && !pasting {
            let op = match ev.keysym {
                Keysym::b | Keysym::B => Some(md::MdOp::Wrap("**")),
                Keysym::i | Keysym::I => Some(md::MdOp::Wrap("*")),
                Keysym::e | Keysym::E => Some(md::MdOp::Wrap("`")),
                Keysym::S if self.mods.shift => Some(md::MdOp::Wrap("~~")),
                _ => None,
            };
            if let Some(op) = op {
                if let Some(d) = self.detail.as_mut() {
                    if d.format(op) {
                        self.request_redraw();
                    }
                }
                return;
            }
        }
        let Some(key) = key else { return };
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        match d.key(key) {
            detail::Outcome::Keep => self.request_redraw(),
            detail::Outcome::Cancel => self.close_detail(false),
            detail::Outcome::Save => self.close_detail(true),
        }
    }

    /// A left click while the detail page is open: focus a field, or cancel when outside.
    fn detail_click(&mut self, p: (f32, f32)) {
        let (vw, vh) = self.view_size();
        let pn = detail::panel(vw, vh);
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        if detail::contains(pn.title, p) {
            d.focus(detail::Field::Title);
        } else if detail::contains(pn.desc, p) {
            d.focus(detail::Field::Desc);
        } else if detail::contains(pn.due, p) {
            d.focus(detail::Field::Due);
        } else if !detail::contains(pn.card, p) {
            self.close_detail(false);
        }
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
            Keysym::a | Keysym::A if ctrl => Key::SelectAll,
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
        if self.name_box.is_some() {
            self.name_box_key(&ev);
            return;
        }
        if self.detail.is_some() {
            self.detail_key(&ev);
            return;
        }
        if ctrl {
            match ev.keysym {
                Keysym::q | Keysym::Q => {
                    if !repeat {
                        self.exit = true;
                    }
                    return;
                }
                Keysym::Left | Keysym::Right if self.editor.editing.is_none() => {
                    self.step_space(if ev.keysym == Keysym::Left { -1 } else { 1 });
                    return;
                }
                // Ctrl+D while editing a todo: open its detail page; on a selected text box:
                // duplicate it.
                Keysym::d | Keysym::D => {
                    if !repeat {
                        if self.editor.editing.is_none() && self.editor.text_focus().is_some() {
                            self.duplicate_text_box();
                        } else {
                            self.open_detail();
                        }
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
                Keysym::c | Keysym::C | Keysym::x | Keysym::X if self.editor.editing.is_some() => {
                    if !repeat {
                        let cut = matches!(ev.keysym, Keysym::x | Keysym::X);
                        self.copy_selection(cut);
                    }
                    return;
                }
                Keysym::c | Keysym::C | Keysym::x | Keysym::X
                    if self.editor.text_focus().is_some() =>
                {
                    if !repeat {
                        self.copy_text_box(matches!(ev.keysym, Keysym::x | Keysym::X));
                    }
                    return;
                }
                // Text formatting on the text being typed or selected.
                Keysym::b
                | Keysym::B
                | Keysym::i
                | Keysym::I
                | Keysym::u
                | Keysym::U
                | Keysym::s
                | Keysym::S
                | Keysym::e
                | Keysym::E
                | Keysym::bracketleft
                | Keysym::bracketright
                    if (self.editor.text_focus().is_some() || self.editor.editing.is_some())
                        && !(matches!(ev.keysym, Keysym::s | Keysym::S) && !self.mods.shift) =>
                {
                    let focus = self.editor.text_focus();
                    let changed = match ev.keysym {
                        Keysym::bracketleft | Keysym::bracketright => focus.is_some_and(|id| {
                            let up = ev.keysym == Keysym::bracketright;
                            self.editor.step_text_size(&mut self.board, id, up)
                        }),
                        k => {
                            let btn = match k {
                                Keysym::b | Keysym::B => FmtBtn::Bold,
                                Keysym::i | Keysym::I => FmtBtn::Italic,
                                Keysym::u | Keysym::U => FmtBtn::Underline,
                                Keysym::e | Keysym::E => FmtBtn::Code,
                                _ => FmtBtn::Strike,
                            };
                            !repeat && self.editor.apply_fmt(&mut self.board, btn)
                        }
                    };
                    if changed {
                        self.after_edit();
                    }
                    return;
                }
                Keysym::v | Keysym::V => {
                    if !repeat {
                        if self.editor.editing.is_some() {
                            self.paste();
                        } else {
                            self.paste_board();
                        }
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
        if ctrl || self.mods.alt || self.mods.logo {
            return;
        }
        // Panning comes before the repeat check so that a held arrow keeps moving.
        let arrow = match ev.keysym {
            Keysym::Left => Some((1.0, 0.0)),
            Keysym::Right => Some((-1.0, 0.0)),
            Keysym::Up => Some((0.0, 1.0)),
            Keysym::Down => Some((0.0, -1.0)),
            _ => None,
        };
        if let Some((x, y)) = arrow {
            let fast = if self.mods.shift { PAN_KEY_FAST } else { 1.0 };
            self.pan_view(x * PAN_PER_KEY * fast, y * PAN_PER_KEY * fast);
            return;
        }
        if repeat {
            return;
        }
        match ev.keysym {
            Keysym::space => self.editor.hand = true,
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
            Keysym::l => self.toggle_theme(),
            Keysym::L => self.toggle_theme_panel(),
            Keysym::b | Keysym::B => self.cycle_background(),
            Keysym::w | Keysym::W => self.cycle_rotation(),
            Keysym::f => self.pick_wallpaper_folder(),
            Keysym::F => self.open_wallpaper_folder(),
            Keysym::question => self.show_help = !self.show_help,
            Keysym::Escape if self.show_help => self.show_help = false,
            Keysym::Escape if self.theme_open => {
                self.theme_open = false;
                self.request_redraw();
            }
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
            Keysym::a | Keysym::A => self.set_tool(Tool::Connector),
            Keysym::g | Keysym::G => self.set_tool(Tool::Group),
            Keysym::t | Keysym::T => self.set_tool(Tool::Todo),
            Keysym::n | Keysym::N => self.set_tool(Tool::Note),
            Keysym::s | Keysym::S => self.set_tool(Tool::Sticker),
            Keysym::m | Keysym::M => self.set_tool(Tool::Image),
            Keysym::p | Keysym::P => self.set_tool(Tool::Pen),
            Keysym::e | Keysym::E => self.set_tool(Tool::Eraser),
            _ => {}
        }
    }

    // ---- pointer

    /// The format bar and grip of the text being typed or selected, if any.
    fn text_bar(&self) -> Option<layout::TextBar> {
        if self.detail.is_some() || self.name_box.is_some() {
            return None;
        }
        let f = &self.fonts;
        let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
        if let Some(id) = self.editor.text_focus() {
            return layout::text_bar(&self.board, id, &self.camera, self.view_size(), &adv);
        }
        // Any other field being typed in gets the Markdown buttons.
        let t = self.editor.editing?.target;
        let r = editor::field_rect(&self.board, t, &adv)?;
        let kind = if t.multiline() {
            layout::BarKind::Block
        } else {
            layout::BarKind::Inline
        };
        Some(layout::format_bar(
            kind,
            r,
            0.0,
            &self.camera,
            self.view_size(),
        ))
    }

    /// A left press on the text format bar or grip: `Some(edited)` if it was there.
    fn text_bar_click(&mut self, p: (f32, f32)) -> Option<bool> {
        let bar = self.text_bar()?;
        if let (true, Some(id)) = (bar.on_grip(p.0, p.1), bar.text_id()) {
            self.editor.grab_text(&self.board, &self.camera, p, id);
            return Some(false);
        }
        if let Some(btn) = bar.btn_at(p.0, p.1) {
            return Some(self.editor.apply_fmt(&mut self.board, btn));
        }
        bar.covers(p.0, p.1).then_some(false)
    }

    /// A left click on the screen-space overlay. Returns true if it was handled there.
    fn overlay_click(&mut self, p: (f32, f32), time: u32) -> bool {
        if self.show_help {
            // Any click closes the popup and is swallowed.
            self.show_help = false;
            return true;
        }
        let ov = self.overlay();
        let was_open = self.theme_open;
        if self.theme_open {
            if let Some(t) = ov.theme_at(p.0, p.1) {
                self.set_theme(t);
                return true;
            }
            if ov.theme_panel.contains(p.0, p.1) {
                return true;
            }
            // Anywhere else closes the panel; the Theme button itself just toggles it below.
            self.theme_open = false;
        }
        if let Some(btn) = ov.view_at(p.0, p.1) {
            match btn {
                ViewBtn::Theme => self.theme_open = !was_open,
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
        if ov.plus_at(p.0, p.1) {
            self.add_space();
            return true;
        }
        if let Some(i) = ov.chip_at(p.0, p.1) {
            let double = self
                .last_chip_click
                .is_some_and(|(t, k)| k == i && time.wrapping_sub(t) < 400);
            if double {
                self.last_chip_click = None;
                self.rename_space(i);
            } else {
                self.last_chip_click = Some((time, i));
                self.jump_to_space(i);
            }
            return true;
        }
        ov.covers(p.0, p.1) // clicks on a pill's padding go nowhere
    }

    /// Recompute what the pointer is over. True if it changed (then a redraw is due).
    fn update_hover(&mut self, p: (f32, f32)) -> bool {
        let bar = self.text_bar();
        let h = match bar.as_ref() {
            Some(b) if b.on_grip(p.0, p.1) => Hover::Grip,
            Some(b) if b.btn_at(p.0, p.1).is_some() => Hover::Fmt(b.btn_at(p.0, p.1).unwrap()),
            _ => self.overlay().hover_at(p.0, p.1, self.theme_open),
        };
        let changed = h != self.hover;
        self.hover = h;
        changed
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
        // Over the space menu the wheel scrolls it instead of the canvas.
        if self
            .overlay()
            .chips_pill
            .is_some_and(|r| r.contains(cx, cy))
        {
            let step = (dx + dy).signum() as i32;
            let max = self.board.spaces.len().saturating_sub(1) as i32;
            self.chip_first = (self.chip_first as i32 + step).clamp(0, max) as usize;
            self.request_redraw();
            return;
        }
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
        self.arm_rotate();
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
        if capability == Capability::Keyboard && self.data_device.is_none() {
            if let Some(m) = &self.data_mgr {
                self.data_device = Some(m.get_data_device(qh, &seat));
            }
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
            // The Space release will go elsewhere.
            self.editor.hand = false;
            self.log(format_args!("[focus] keyboard LEAVE"));
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        serial: u32,
        ev: KeyEvent,
    ) {
        self.key_serial = serial;
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
        ev: KeyEvent,
    ) {
        if ev.keysym == Keysym::space {
            self.editor.hand = false;
        }
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
                PointerEventKind::Enter { .. } => {
                    self.cursor = pos;
                    redraw |= self.update_hover(pos);
                }
                PointerEventKind::Leave { .. } => {
                    if self.hover != Hover::None {
                        self.hover = Hover::None;
                        redraw = true;
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.cursor = pos;
                    redraw |= self.update_hover(pos);
                    if self.editor.is_dragging() {
                        let f = &self.fonts;
                        let adv = |s: Style, c: char| f.advance(s.face(), c, s.size());
                        edited |= self.editor.drag_text(&self.board, &self.camera, pos, &adv)
                            || self.editor.motion(&mut self.board, &mut self.camera, pos);
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
                    if self.name_box.is_some() {
                        if b == Some(Button::Left) {
                            let (vw, vh) = self.view_size();
                            let (card, _) = detail::name_panel(vw, vh);
                            if !detail::contains(card, pos) {
                                self.close_name_box(false);
                            }
                        }
                    } else if self.detail.is_some() {
                        if b == Some(Button::Left) {
                            self.detail_click(pos);
                        }
                    } else if let Some(b) = b {
                        let panning = self.editor.hand && self.editor.editing.is_none();
                        let on_bar = if b == Button::Left && !panning {
                            self.text_bar_click(pos)
                        } else {
                            None
                        };
                        if let Some(e) = on_bar {
                            edited |= e;
                            redraw = true;
                        } else if b == Button::Left && self.overlay_click(pos, time) {
                            redraw = true;
                        } else if b == Button::Right
                            && self.overlay().chip_at(pos.0, pos.1).is_some()
                        {
                            // Right-click a space in the menu: delete it (Ctrl+Z brings it back).
                            let id = self
                                .overlay()
                                .chip_at(pos.0, pos.1)
                                .and_then(|i| self.board.spaces.get(i).map(|s| s.id));
                            if let Some(id) = id {
                                if self.editor.delete_space(&mut self.board, id) {
                                    self.after_edit();
                                }
                            }
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

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: String,
        mut fd: WritePipe,
    ) {
        use std::io::Write;
        let _ = fd.write_all(self.clip_text.as_bytes());
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        // Someone else copied: stop claiming the clipboard and paste from them instead.
        if self
            .copy_source
            .as_ref()
            .is_some_and(|s| s.inner() == source)
        {
            self.copy_source = None;
        }
        source.destroy();
    }
    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

smithay_client_toolkit::delegate_dispatch2!(App);

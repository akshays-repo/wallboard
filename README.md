# wallboard

A life-OS home screen for the [niri](https://github.com/YaLTeR/niri) compositor. It is a
whiteboard that lives on niri's **background layer**: on an empty workspace you see (and use)
the board, open an app and it tiles over it, close it and the board is back.

- Areas of todos: **Inbox, Focus** (the hero card, at most 3 open todos), Work, Startups,
  Health, Learning, Life. Add, rename and move groups freely.
- Free sticky **notes** (title + body) and a **pen / eraser**.
- Todos have **priority** (`!`, `!!`, `!!!`), a done circle, and optional multi-line **details**.
- Quick reach: area chips, number keys, capture-to-Inbox, a launcher dock.
- Apple Reminders / Freeform look, dark and light. Solid-colour or **wallpaper** background.
- One JSON file, autosaved, optionally **backed up to a private git repository**.
- CPU rendering (tiny-skia), no GPU, no GUI toolkit. About **15 MiB** of RAM idle at 1080p.

It is not a normal window and not a compositor.

## Build

One script does the whole setup (dependencies, build, install, optional extras):

```sh
./install.sh                      # dnf deps (asks for sudo), build, install to ~/.local/bin
./install.sh --help               # all options; add --dry-run to see what it would do
```

Useful extras (combine as needed; run with `--no-deps --no-build` once built):

```sh
./install.sh --no-deps --no-build --niri                 # start with niri (backs up + validates your config)
./install.sh --no-deps --no-build --wallpaper ~/wall.jpg # PNG as is, JPEG converted with ImageMagick
./install.sh --no-deps --no-build --backup               # auto-create a PRIVATE GitHub repo and back up to it
./install.sh --uninstall                                 # remove binary + niri block (keeps your data)
```

Or by hand on Fedora:

```sh
sudo dnf install gcc pkgconf-pkg-config libxkbcommon-devel rust cargo
cargo build --release
install -Dm755 target/release/wallboard ~/.local/bin/wallboard
```

Needs a compositor with `wlr-layer-shell` (niri has it). Fonts are embedded (Inter, subset to
Latin, SIL OFL 1.1), so nothing needs to be installed for text.

## Use

Run `wallboard` on an **empty workspace** (windows cover the board). Click the board once so it
has keyboard focus. Quit with `Ctrl+Q`.

### Mouse

| Action | Result |
|---|---|
| Scroll | pan (Shift+scroll: sideways) |
| Ctrl+scroll | zoom around the cursor |
| Middle-drag, or **Space** + drag | pan (Space + drag works over cards too and leaves them alone) |
| Drag the empty part of a group | pan |
| Drag empty space | draw a selection box |
| Drag a group by its title | move it |
| Double-click empty space | new group |
| Double-click a title | rename it |
| Click a circle | complete / re-open |
| Click a todo | edit its text |
| Click the right edge of a row | cycle priority |
| Click **+ New todo** | add a todo |
| Click the colour dot | cycle the colour |
| Right-click a todo | remove it (right-click an *empty* group or a note removes that) |

### Keys (when not typing)

| Key | Action |
|---|---|
| `1`-`9` | jump to the Nth area |
| `C` | capture a new todo into Inbox, from anywhere |
| `0` | fit everything on screen |
| `+` / `-` | zoom |
| Arrow keys | pan (hold to keep moving; with Shift, four times as far) |
| `Space` + drag | pan with the left button |
| `L` / `Shift+L` | dark / light; open the **theme picker** (also the half-moon button on the right menu): Dark, Light, Aurora, Neon, Blueprint, Paper, Brutal. Hovering any toolbar, menu or chip button highlights it |
| `B` | cycle the background: theme colour, six colours, then each wallpaper |
| `F` / `Shift+F` | pick the wallpaper folder in a dialog (needs `zenity`; else it opens the folder) / open it in the file manager |
| `W` | wallpaper rotation: off, in order, random (every 30 min, or `rotate_mins`) |
| `S` `M` | sticker tool (click to drop an emoji; double-click it for the next one) and paste image (also `Ctrl+V` with nothing being edited: a copied PNG or image file); both are on the toolbar |
| `V` `G` `T` `N` `A` `P` `E` | tools: select, group, todo, note, arrow, pen, eraser (also the toolbar on the left) |
| `Ctrl+Z`, `Ctrl+Shift+Z` / `Ctrl+Y` | undo, redo |
| `Ctrl+Q` | quit |

### Text boxes

Press `X` (or the **T** on the toolbar) and click to type free text. Select a text box, or click
into it, and a **format bar** appears above it:

- **B / I / U / S**: bold, italic, underline, strike-through (`Ctrl+B`, `Ctrl+I`, `Ctrl+U`,
  `Ctrl+Shift+S`)
- four sizes: small, medium, heading, title (`Ctrl+[` / `Ctrl+]` step down / up)
- eight colours (the first is the theme's text colour)
- **MD**: render the text as Markdown (on by default)

The style applies to the whole box. To move a text box, drag the **grip** (the dots to its left).
This works while you are typing too. A plain drag on a box you are not editing also moves it.

**Styling any text (notes, todos, details):** while typing in a sticky note, a todo or a todo's
details, the format bar shows **B / I / S / `<>`** (and, in multi-line fields, heading, bullet
and checkbox). Select words and press a button, or `Ctrl+B` / `Ctrl+I` / `Ctrl+Shift+S` /
`Ctrl+E` (code). With nothing selected they apply to the word under the caret. Press again to
remove the style. Styles are stored as Markdown markers in the text (`**bold**`), shown raw
while you type and rendered once you click away. On the todo detail page use the shortcuts.
In a free text box with nothing selected, B / I / S style the whole box.

**Copy and paste:** with a text box selected (not typing), `Ctrl+C` copies it and `Ctrl+V`
pastes a copy, with its style, under the pointer. `Ctrl+X` cuts it and `Ctrl+D` duplicates it.
The text also goes to the system clipboard. Text copied in another app and pasted with
`Ctrl+V` (when nothing is being edited) becomes a new text box. A PNG still becomes an image.

**Markdown** is rendered while you are not typing in the box. While you type you see the raw
text. One line is one block: `#`, `##`, `###` headings, `-` / `*` bullets (indent two spaces to
nest), `1.` numbered items, `- [ ]` / `- [x]` tasks, `> ` quotes, `---` rules, and inline
`**bold**`, `*italic*`, `~~strike~~`, `` `code` `` (`\*` for a literal star). Long lines do not
wrap.

Typing: **Enter** adds a todo below (on an empty todo it discards it), **Tab** opens a todo's
details (Shift+Tab goes back), **Ctrl+Enter** toggles done, **Ctrl+P** cycles priority,
arrows / Home / End move, **Esc** stops. No IME, Latin text only (no per-character styling:
a text box has one style plus its Markdown).

The **Focus** card holds at most **3 open todos**: finish one to add another.

## Files and settings

Everything is in one file, `~/.local/share/wallboard/board.json` (`$XDG_DATA_HOME`, or set
`WALLBOARD_DATA_DIR`). It is written about a second after a change, atomically. You can edit
it while wallboard is **not** running:

```jsonc
{
  "version": 2,
  "launchers": [                       // the dock at the bottom; run with `sh -c`, detached
    { "label": "Files",    "command": "xdg-open \"$HOME\"" },
    { "label": "Terminal", "command": "alacritty" }
  ],
  "ui": { "top": 64, "bottom": 52 },   // keep the chips / dock clear of a shell bar and dock
  "background": { "color": "#14211A", "image": "/home/me/wall.png", "image_on": false, "dim": 0.35,
                  "rotate_mins": 30, "random": true, "folder": "/home/me/Pictures/walls" },
  "backup": { "interval_secs": 600, "debounce_secs": 30 },   // git backup cadence (see below)
  "theme": "dark"
}
```

- **Wallpaper:** press `B` until it shows, or set `"background": {"image_on": true}`. The image
  is `background.image`, or `wallpaper.png` in the data folder if that is not set. **PNG only**
  (a JPEG gives a clear message and falls back to a colour; convert it with
  `magick wall.jpg wall.png`). It is darkened by `dim` so text stays readable, scaled to cover
  the screen, and its cache (about one screen buffer) is freed while idle.
- **Many wallpapers + rotation:** drop PNGs into `wallpapers/` in the data folder. `B` steps
  through them after the colours. `W` (or `"rotate_mins"` / `"random"` in `background`) changes
  the wallpaper on a timer, in name order or at random. One timer wakeup per interval, and only
  the current wallpaper is ever decoded.
- **`ui` margins:** a shell bar or dock on niri's *top* layer sits above wallboard and swallows
  clicks under it. The defaults clear a typical 50 px bar and a dock; raise them if the chips or
  dock do not react to clicks.
- **Safety:** a corrupt or unknown-version file is moved to `board.json.bad` (never
  overwritten, `.bad.1`, `.bad.2` ... if there are several) and you start fresh.
  `board.json.bak` is a copy of the last file that loaded successfully.

## Back up to a private GitHub repository

> **The repository MUST be private.** `board.json` contains your todos and notes. Anyone who can
> read the repository can read all of it.

**Automatic (needs the GitHub CLI, `sudo dnf install gh && gh auth login`):**

```sh
./install.sh --no-deps --no-build --backup
```

This creates the private repository for you (`gh repo create wallboard-data --private`, or
`--repo-name NAME`), re-uses it if it already exists, **checks that it really is private and
refuses otherwise**, and then runs `wallboard --init-sync` on it.

**Manual**, either way:

1. Create a **private** repository: `gh repo create wallboard-data --private`, or on github.com
   (*New repository*, choose **Private**).
2. Point wallboard at it (once). With `--backup-url` the installer also checks it is private:

   ```sh
   wallboard --init-sync git@github.com:YOU/wallboard-data.git
   ```

`--init-sync` runs `git init -b main` in the data folder, writes a `.gitignore`
(`*.bak`, `*.bad`, `*.tmp`, `wallpaper.*`), sets `origin`, commits `board.json` and pushes.
Only `board.json` and `.gitignore` are tracked.

From then on, while wallboard runs, it backs up **at most once every 10 minutes**: a change is
committed and pushed 30 s after the last edit, and later changes wait until 10 minutes after the
previous backup. Change the cadence with `"backup": {"interval_secs": 600, "debounce_secs": 30}`
in `board.json`. Panning, zooming and theme changes never create a commit. If you are offline, or
git needs credentials, it retries (every 2 minutes at the default); it never prompts
(`GIT_TERMINAL_PROMPT=0`, ssh `BatchMode`) and every git command has a timeout. Quitting commits
pending work locally and never waits for the network; the next run pushes it after about a minute.

For SSH, make sure `ssh -T git@github.com` works without a passphrase prompt (an ssh-agent, or
a key without a passphrase), or use an `https://` URL with a git credential helper.

## niri integration

None of this edits your config; add what you want to `~/.config/niri/config.kdl`.

```kdl
// Start the board with niri.
spawn-at-startup "wallboard"

// Jump to the home workspace (keep workspace 1 free of windows).
binds {
    Mod+Home { focus-workspace 1; }
}

layer-rule {
    match namespace="^wallboard$"
    // In the overview, background layers zoom out with the workspaces. place-within-backdrop only
    // applies to the *background* layer, so it needs WALLBOARD_LAYER=background (see below).
    // Untested here: check how the overview looks.
    // place-within-backdrop true
}
```

- **Restart cleanly:** `pkill wallboard; wallboard &` (or `systemctl --user restart wallboard`).
  It saves before exiting on Ctrl+Q; a plain `pkill` loses at most the last second.
- **Layer:** wallboard runs on niri's **bottom** layer, which is above every background-layer
  surface. That matters because a shell such as DankMaterialShell re-creates its own wallpaper
  a few seconds after login and would otherwise cover a background-layer board. Set
  `WALLBOARD_LAYER=background` to use the background layer instead (needed for
  `place-within-backdrop`).
- **Another wallpaper tool** (swaybg, DankMaterialShell, ...) can keep running: wallboard sits
  above it while it runs and the wallpaper reappears when it exits. A shell **bar** on the top
  layer stays above the board; see the `ui` margin above.
- **Optional systemd user unit** instead of `spawn-at-startup`
  (`contrib/wallboard.service`):

  ```sh
  install -Dm644 contrib/wallboard.service ~/.config/systemd/user/wallboard.service
  systemctl --user enable --now wallboard
  ```

  Use one or the other, not both.

## Tests

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The end-to-end suites drive the **real** app on your desktop through a virtual keyboard and
mouse (`/dev/uinput`), switch to workspace 2 (it must be empty), and use throw-away data
folders only:

```sh
python3 -I tests/e2e_lifeos.py     # ~3 min: everything above
python3 -I tests/e2e_backup.py     # ~3 min: git backup (shortened intervals) against a local bare repository
```

## Limits

Latin text only (no IME, no shaping); one surface (the output that niri gives it); PNG
wallpapers only; `SIGTERM` is not caught (the board is saved about a second after each change,
so at most that second is lost).

Licences: wallboard's own code is yours to license; the embedded Inter font is SIL OFL 1.1
(`assets/LICENSE-Inter.txt`).

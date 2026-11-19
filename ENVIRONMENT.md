# ENVIRONMENT (Phase 0, detected 2026-11-19)

| Item | Value |
|---|---|
| OS / kernel | Fedora 43 Workstation, 7.3.3-100.fc43.x86_64 |
| Session | Wayland (`wayland-1`) |
| RAM / CPU | 14 GiB total (~9.8 GiB available), 8 GiB swap, 12 threads |
| Compositor | niri 26.04 |
| Output | 1 monitor: eDP-1, 1920x1080 @ 165 Hz, **scale 1**, no transform |
| Workspaces | 2 on eDP-1; workspace 1 active with 2 windows (browser, Alacritty) |
| Existing layer surfaces | Background: `dms:desktop-input`, `quickshell`. Top: `dms:bar`, `dms:dock`. Overlay: `dms:tooltip`. All keyboard-interactivity none |
| Shell / wallpaper | DankMaterialShell (`dms run`, quickshell). Wallpaper is a jpg driven by `dms matugen` |
| Wayland protocols | **Unknown**: `wayland-utils` not installed, `wayland-info` gave nothing |
| Rust toolchain | **Not installed** (`rustc`, `cargo`, `rustup` missing) |
| Build deps present | gcc 15.3.1, pkgconf-pkg-config, libxkbcommon-devel, fontconfig-devel |
| Fonts | sans: `/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf`; mono: `.../NotoSansMono[wght].ttf`; 543 fonts |
| niri config | `~/.config/niri/config.kdl` (506 lines) + `include "dms/{cursor,binds,outputs}.kdl"`. No `spawn-at-startup` active, no `layer-rule`, no `workspace` blocks |

## Actions needed from you

```sh
# 1. toolchain + protocol tool
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # or: sudo dnf install rust cargo clippy rustfmt
sudo dnf install wayland-utils
# 2. then re-run and paste the output
wayland-info 2>/dev/null | grep -iE -A2 "layer_shell|wl_shm|fractional|viewporter"
```

Needed to confirm layer-shell version >= 4 (on-demand keyboard focus). Niri 26.04 should expose v4+, but this is unverified until you paste the output.

## Memory budget (1920x1080, scale 1)

- One buffer: 1920 x 1080 x 4 = 8,294,400 B = **7.9 MiB**. Two: **15.8 MiB**.
- Process baseline (Rust binary, wayland, fontdue, one font file, glyph cache): roughly 4-6 MiB.
- Expected minimum idle RSS: **~12-14 MiB** with one live buffer, **~20-22 MiB** double-buffered. These are estimates; Phase 1 measures the real number.
- Plan: two `SlotPool` buffers while drawing, release the spare when idle if the measurement justifies it.
- Font: the Noto Sans variable file is a few MB on disk; read it once, rasterise via fontdue, and measure. A smaller static font may be chosen if RSS suffers.

## Conflicts and notes

1. **DankMaterialShell already owns the background layer** (`quickshell` surface draws the wallpaper). Wallboard would be a second background surface; stacking among same-layer surfaces follows map order, so wallboard could be hidden under or cover the DMS wallpaper unpredictably. Options for Phase 1 to test: (a) start wallboard after DMS so it maps on top, (b) turn the DMS wallpaper off in DMS settings. Not touching your config; Phase 6 will give exact lines.
2. DMS has `dms:desktop-input` on the background layer too, which may grab pointer input on the empty desktop. Phase 1 logging will show whether clicks reach wallboard.
3. Workspace 1 currently has 2 windows, so the board is only visible on an empty workspace (workspace 2, or after closing windows). Windows also hide it. Keep one workspace window-free as "home".
4. Single output, integer scale 1: fractional scale not needed for now; design is still one surface per output.
5. No `layer-rule` exists yet. `place-within-backdrop` is optional (Phase 6).

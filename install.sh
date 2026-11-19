#!/bin/sh
# wallboard setup: dependencies, build, install, and optional niri / systemd / wallpaper / backup.
# POSIX sh. Run `./install.sh --help` for the options. Nothing outside your home directory is
# touched except `dnf install` (asks for sudo), and your niri config is only edited with --niri.
set -eu

PREFIX_BIN="${PREFIX_BIN:-$HOME/.local/bin}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
NIRI_CONFIG="${NIRI_CONFIG:-$CONFIG_HOME/niri/config.kdl}"
DATA_DIR="${WALLBOARD_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/wallboard}"
UNIT_DIR="$CONFIG_HOME/systemd/user"
REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
MARK_BEGIN="// >>> wallboard (added by install.sh) >>>"
MARK_END="// <<< wallboard <<<"

DEPS=1; BUILD=1; NIRI=0; SYSTEMD=0; DRY=0; UNINSTALL=0
WALLPAPER=""; BACKUP_URL=""; BACKUP=0; REPO_NAME="wallboard-data"

usage() {
    cat <<'EOF'
Usage: ./install.sh [options]

  (no options)         install build dependencies (dnf), build, install the binary
  --no-deps            skip `sudo dnf install` (you already have gcc, pkgconf, libxkbcommon-devel, rust)
  --no-build           skip building (use an existing target/release/wallboard)
  --niri               add `spawn-at-startup` and a layer-rule to your niri config
                       (backs it up, validates it, restores it if niri rejects it)
  --systemd            install and enable a systemd user unit instead of spawn-at-startup
  --wallpaper FILE     use FILE as the wallpaper (PNG; JPEG is converted with ImageMagick)
  --backup             back board.json up to git: automatically creates a PRIVATE GitHub
                       repository with `gh` (re-uses it if it exists and is private), then
                       sets the backup up. It syncs about every 10 minutes while wallboard runs.
  --repo-name NAME     name for that repository (default: wallboard-data)
  --backup-url URL     use an existing repository instead (it must be PRIVATE; checked with `gh`
                       when possible, otherwise you are asked to confirm)
  --uninstall          remove the binary, the niri block and the systemd unit (keeps your data)
  --dry-run            print what would be done and change nothing
  -h, --help           this help

Use --niri OR --systemd, not both. Your board data is never deleted.

Environment overrides: PREFIX_BIN, NIRI_CONFIG, WALLBOARD_DATA_DIR, XDG_CONFIG_HOME.
EOF
}

say() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Run a command, or just print it with --dry-run.
run() {
    if [ "$DRY" = 1 ]; then
        printf '[dry-run] %s\n' "$*"
    else
        "$@"
    fi
}

while [ $# -gt 0 ]; do
    case "$1" in
        --no-deps) DEPS=0 ;;
        --no-build) BUILD=0 ;;
        --niri) NIRI=1 ;;
        --systemd) SYSTEMD=1 ;;
        --uninstall) UNINSTALL=1 ;;
        --dry-run) DRY=1 ;;
        --wallpaper) shift; [ $# -gt 0 ] || die "--wallpaper needs a file"; WALLPAPER="$1" ;;
        --backup) BACKUP=1 ;;
        --repo-name) shift; [ $# -gt 0 ] || die "--repo-name needs a name"; REPO_NAME="$1" ;;
        --backup-url) shift; [ $# -gt 0 ] || die "--backup-url needs a URL"; BACKUP_URL="$1" ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

[ "$NIRI" = 1 ] && [ "$SYSTEMD" = 1 ] && die "use --niri or --systemd, not both (the board would start twice)"
case "$BACKUP_URL" in -*) die "that URL looks like an option: $BACKUP_URL" ;; esac
case "$REPO_NAME" in
    ""|-*|*[!A-Za-z0-9._-]*) die "bad repository name: $REPO_NAME (letters, digits, . _ - only)" ;;
esac
[ "$BACKUP" = 1 ] && [ -n "$BACKUP_URL" ] && die "use --backup (create a repo) or --backup-url (existing repo), not both"

# ---- niri config block ------------------------------------------------------------------

niri_block() {
    cat <<EOF
$MARK_BEGIN
spawn-at-startup "wallboard"
layer-rule {
    match namespace="^wallboard\$"
    // place-within-backdrop true   // optional: show the board in the overview backdrop
}
$MARK_END
EOF
}

remove_niri_block() {
    [ -f "$NIRI_CONFIG" ] || return 0
    grep -qF "$MARK_BEGIN" "$NIRI_CONFIG" || return 0
    say "removing the wallboard block from $NIRI_CONFIG"
    if [ "$DRY" = 1 ]; then printf '[dry-run] strip the marked block from %s\n' "$NIRI_CONFIG"; return 0; fi
    cp "$NIRI_CONFIG" "$NIRI_CONFIG.wallboard-backup"
    # Drop the marked block and the blank line the installer put in front of it.
    awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
        index($0, b) { held = 0; skip = 1; next }
        index($0, e) { skip = 0; next }
        skip { next }
        $0 == "" { if (held) print ""; held = 1; next }
        { if (held) print ""; held = 0; print }
        END { if (held) print "" }' "$NIRI_CONFIG.wallboard-backup" > "$NIRI_CONFIG"
}

add_niri_block() {
    [ -f "$NIRI_CONFIG" ] || die "niri config not found: $NIRI_CONFIG"
    if grep -qF "$MARK_BEGIN" "$NIRI_CONFIG"; then
        say "niri config already has the wallboard block (nothing to do)"
        return 0
    fi
    if grep -Eq '^[[:space:]]*spawn-at-startup[[:space:]]+"wallboard"' "$NIRI_CONFIG"; then
        warn "your niri config already starts wallboard; leaving it alone"
        return 0
    fi
    say "adding the wallboard block to $NIRI_CONFIG (backup: $NIRI_CONFIG.wallboard-backup)"
    if [ "$DRY" = 1 ]; then
        printf '[dry-run] append to %s:\n' "$NIRI_CONFIG"; niri_block | sed 's/^/    /'
        return 0
    fi
    cp "$NIRI_CONFIG" "$NIRI_CONFIG.wallboard-backup"
    { printf '\n'; niri_block; } >> "$NIRI_CONFIG"
    if command -v niri >/dev/null 2>&1; then
        if niri validate -c "$NIRI_CONFIG" >/dev/null 2>&1; then
            say "niri accepts the new config"
        else
            warn "niri rejected the config; restoring your original"
            cp "$NIRI_CONFIG.wallboard-backup" "$NIRI_CONFIG"
            die "the niri block was NOT added (run \`niri validate\` to see why); add it by hand from the README"
        fi
    else
        warn "niri not found, so the config was not validated"
    fi
}

# ---- uninstall --------------------------------------------------------------------------

if [ "$UNINSTALL" = 1 ]; then
    if [ -f "$UNIT_DIR/wallboard.service" ] || [ "$DRY" = 1 ]; then
        command -v systemctl >/dev/null 2>&1 && run systemctl --user disable --now wallboard 2>/dev/null || true
        run rm -f "$UNIT_DIR/wallboard.service"
    fi
    remove_niri_block
    run rm -f "$PREFIX_BIN/wallboard"
    say "removed. Your board data in $DATA_DIR was kept."
    exit 0
fi

# ---- 1. dependencies --------------------------------------------------------------------

if [ "$BUILD" = 1 ] && [ "$DEPS" = 1 ]; then
    if command -v dnf >/dev/null 2>&1; then
        say "installing build dependencies with dnf (sudo will ask for your password)"
        run sudo dnf install -y gcc pkgconf-pkg-config libxkbcommon-devel rust cargo
    else
        warn "dnf not found: install gcc, pkg-config, libxkbcommon dev files and a Rust toolchain yourself"
    fi
fi

# ---- 2. build ---------------------------------------------------------------------------

if [ "$BUILD" = 1 ]; then
    command -v cargo >/dev/null 2>&1 || [ "$DRY" = 1 ] || die "cargo not found (install Rust, or drop --no-deps)"
    say "building (release)"
    run sh -c "cd '$REPO_DIR' && cargo build --release"
fi

BIN="${WALLBOARD_SRC_BIN:-$REPO_DIR/target/release/wallboard}"
[ "$DRY" = 1 ] || [ -x "$BIN" ] || die "$BIN not found; run without --no-build"

# ---- 3. install the binary --------------------------------------------------------------

say "installing to $PREFIX_BIN/wallboard"
run install -Dm755 "$BIN" "$PREFIX_BIN/wallboard"
case ":$PATH:" in
    *":$PREFIX_BIN:"*) ;;
    *) warn "$PREFIX_BIN is not on your PATH; niri's spawn-at-startup needs it there (or use an absolute path)" ;;
esac

# ---- 4. optional: wallpaper -------------------------------------------------------------

if [ -n "$WALLPAPER" ]; then
    [ -f "$WALLPAPER" ] || die "wallpaper not found: $WALLPAPER"
    say "setting the wallpaper"
    run mkdir -p "$DATA_DIR"
    case "$(printf '%s' "$WALLPAPER" | tr '[:upper:]' '[:lower:]')" in
        *.png) run cp "$WALLPAPER" "$DATA_DIR/wallpaper.png" ;;
        *)
            if command -v magick >/dev/null 2>&1; then
                run magick "$WALLPAPER" "$DATA_DIR/wallpaper.png"
            elif command -v convert >/dev/null 2>&1; then
                run convert "$WALLPAPER" "$DATA_DIR/wallpaper.png"
            else
                die "only PNG wallpapers are supported and ImageMagick is not installed (sudo dnf install ImageMagick)"
            fi
            ;;
    esac
    say "press B in wallboard until the wallpaper shows"
fi

# ---- 5. optional: autostart -------------------------------------------------------------

if [ "$NIRI" = 1 ]; then
    add_niri_block
elif [ "$SYSTEMD" = 1 ]; then
    say "installing the systemd user unit"
    run mkdir -p "$UNIT_DIR"
    run install -Dm644 "$REPO_DIR/contrib/wallboard.service" "$UNIT_DIR/wallboard.service"
    if [ "$PREFIX_BIN" != "$HOME/.local/bin" ]; then
        run sed -i "s|^ExecStart=.*|ExecStart=$PREFIX_BIN/wallboard|" "$UNIT_DIR/wallboard.service"
    fi
    run systemctl --user daemon-reload
    run systemctl --user enable --now wallboard
fi

# ---- 6. optional: git backup -------------------------------------------------------------

# Ask a yes/no question on stdin. $1 = question, $2 = default (y or n).
# End of input (no terminal) counts as "no": the safe answer for a privacy question.
ask() {
    if [ "$DRY" = 1 ]; then printf '[dry-run] would ask: %s -> assuming yes\n' "$1"; return 0; fi
    if [ "$2" = y ]; then hint="[Y/n]"; else hint="[y/N]"; fi
    printf '%s %s ' "$1" "$hint" >&2
    if ! read -r reply; then printf '\n' >&2; return 1; fi
    case "$reply" in
        [Yy]*) return 0 ;;
        [Nn]*) return 1 ;;
        "") [ "$2" = y ] ;;
        *) return 1 ;;
    esac
}

have_gh() { command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; }

# owner/repo from a GitHub URL (git@github.com:o/r.git, https://github.com/o/r, ssh://git@github.com/o/r)
gh_slug() {
    printf '%s' "$1" | sed -n -e 's#^git@github\.com:\([^/]*/[^/]*\)$#\1#p' \
        -e 's#^\(https://\|ssh://git@\)github\.com/\([^/]*/[^/]*\)$#\2#p' | sed 's#\.git$##'
}

# Refuse unless the repository is private (verified with gh, or confirmed by you).
verify_private() {
    slug="$(gh_slug "$1")"
    if [ -n "$slug" ] && have_gh; then
        priv="$(gh repo view "$slug" --json isPrivate -q .isPrivate 2>/dev/null || true)"
        case "$priv" in
            true) say "$slug is private"; return 0 ;;
            false) die "$slug is PUBLIC. Your todos and notes would be readable by anyone. Make it private (gh repo edit $slug --visibility private --accept-visibility-change-consequences) and re-run." ;;
        esac
        warn "could not check $slug with gh"
    fi
    ask "I cannot verify that $1 is PRIVATE. Is it a private repository?" n \
        || die "not continuing: the backup repository must be private"
}

create_private_repo() {
    have_gh || {
        warn "the GitHub CLI is not installed or not logged in"
        cat >&2 <<'EOT'
Either: sudo dnf install gh && gh auth login   (then run this again with --backup)
Or:     create a PRIVATE repository on github.com yourself, then run
        ./install.sh --no-deps --no-build --backup-url git@github.com:YOU/REPO.git
EOT
        return 1
    }
    login="$(gh api user -q .login 2>/dev/null)" || die "could not read your GitHub login with gh"
    [ -n "$login" ] || die "could not read your GitHub login with gh"
    if gh repo view "$login/$REPO_NAME" >/dev/null 2>&1; then
        say "$login/$REPO_NAME already exists; checking that it is private"
    else
        say "creating the PRIVATE GitHub repository $login/$REPO_NAME"
        run gh repo create "$REPO_NAME" --private --description "wallboard backup (private)"
    fi
    if [ "$DRY" = 0 ]; then
        [ "$(gh repo view "$login/$REPO_NAME" --json isPrivate -q .isPrivate 2>/dev/null)" = true ] \
            || die "$login/$REPO_NAME is not private; refusing to back up to it"
    fi
    BACKUP_URL="git@github.com:$login/$REPO_NAME.git"
    return 0
}

if [ "$BACKUP" = 1 ]; then
    create_private_repo || BACKUP_URL=""
elif [ -n "$BACKUP_URL" ]; then
    verify_private "$BACKUP_URL"
fi

if [ -n "$BACKUP_URL" ]; then
    say "setting up the git backup to $BACKUP_URL"
    if [ "$DRY" = 1 ]; then
        printf '[dry-run] %s --init-sync %s\n' "$PREFIX_BIN/wallboard" "$BACKUP_URL"
    else
        WALLBOARD_DATA_DIR="$DATA_DIR" "$PREFIX_BIN/wallboard" --init-sync "$BACKUP_URL"
    fi
fi

# ---- done -------------------------------------------------------------------------------

say "done."
if [ "$NIRI" = 0 ] && [ "$SYSTEMD" = 0 ]; then
    cat <<EOF

Next steps:
  * try it:           niri msg action focus-workspace 2 && wallboard
  * start with niri:  ./install.sh --no-deps --no-build --niri
                      (or add the lines from the README to $NIRI_CONFIG yourself)
  * back it up:       ./install.sh --no-deps --no-build --backup
                      (auto-creates a PRIVATE GitHub repository with gh)
EOF
fi

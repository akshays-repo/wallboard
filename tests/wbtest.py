"""Shared helpers for wallboard end-to-end tests. Stdlib only.

Injects REAL input through /dev/uinput (an absolute pointer and a keyboard) and drives
niri through `niri msg`. Requires write access to /dev/uinput and an empty workspace 2.
"""
import fcntl, json, os, re, shutil, struct, subprocess, tempfile, time

EV_SYN, EV_KEY, EV_REL, EV_ABS = 0, 1, 2, 3
REL_HWHEEL, REL_WHEEL = 6, 8
ABS_X, ABS_Y = 0, 1
ABS_MAX = 32767
SCREEN = (1920, 1080)
BTN_LEFT, BTN_RIGHT, BTN_MIDDLE = 0x110, 0x111, 0x112

KEYS = {
    "esc": 1, "tab": 15, "minus": 12, "equal": 13, "backspace": 14, "enter": 28, "ctrl": 29, "shift": 42, "space": 57,
    "home": 102, "up": 103, "left": 105, "right": 106, "end": 107, "down": 108, "delete": 111,
}
for i, c in enumerate("1234567890"):
    KEYS[c] = 2 + i
for row, start in (("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)):
    for i, c in enumerate(row):
        KEYS[c] = start + i

UI_SET_EVBIT, UI_SET_KEYBIT, UI_SET_RELBIT, UI_SET_ABSBIT = 0x40045564, 0x40045565, 0x40045566, 0x40045567
UI_DEV_SETUP, UI_ABS_SETUP, UI_DEV_CREATE, UI_DEV_DESTROY = 0x405C5503, 0x401C5504, 0x5501, 0x5502


class Dev:
    def __init__(self, name, keys=(), rels=(), abs_axes=()):
        self.fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_KEY)
        for k in keys:
            fcntl.ioctl(self.fd, UI_SET_KEYBIT, k)
        if rels:
            fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_REL)
            for r in rels:
                fcntl.ioctl(self.fd, UI_SET_RELBIT, r)
        if abs_axes:
            fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_ABS)
            for a in abs_axes:
                fcntl.ioctl(self.fd, UI_SET_ABSBIT, a)
                fcntl.ioctl(self.fd, UI_ABS_SETUP, struct.pack("HHiiiiii", a, 0, 0, 0, ABS_MAX, 0, 0, 0))
        fcntl.ioctl(self.fd, UI_DEV_SETUP, struct.pack("HHHH80sI", 3, 0x1234, 0x5678, 1, name.encode(), 0))
        fcntl.ioctl(self.fd, UI_DEV_CREATE)
        time.sleep(0.8)

    def emit(self, typ, code, val):
        os.write(self.fd, struct.pack("llHHi", 0, 0, typ, code, val))

    def sync(self):
        self.emit(EV_SYN, 0, 0)

    def close(self):
        try:
            fcntl.ioctl(self.fd, UI_DEV_DESTROY)
        finally:
            os.close(self.fd)


def niri(*args):
    return subprocess.run(["niri", "msg", *args], capture_output=True, text=True)


def rss_kib(pid):
    for l in open(f"/proc/{pid}/status"):
        if l.startswith("VmRSS:"):
            return int(l.split()[1])


def cpu_ticks(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])


STATE = re.compile(r"^\[state\] (\{.*\})\s*$")


class Session:
    """Runs wallboard on workspace 2 with real injected input."""

    def __init__(self, binary, shots_dir, data_dir=None):
        self.binary, self.shots_dir = binary, shots_dir
        # Never touch the real ~/.local/share/wallboard from a test.
        self._own_dir = data_dir is None
        self.data_dir = data_dir or tempfile.mkdtemp(prefix="wallboard-e2e-")
        self.lines, self.pos = [], [0.0, 0.0]
        self.kb = self.ptr = self.proc = None
        self.home = None

    # -- lifecycle
    def start(self):
        wins = json.loads(niri("--json", "windows").stdout or "[]")
        ws = json.loads(niri("--json", "workspaces").stdout or "[]")
        ws2 = next((w for w in ws if w.get("idx") == 2), None)
        if not ws2 or any(w.get("workspace_id") == ws2["id"] for w in wins):
            raise SystemExit("ABORT: workspace 2 missing or not empty")
        self.home = next(w["idx"] for w in ws if w.get("is_focused"))
        self.kb = Dev("wb-test-kbd", keys=KEYS.values())
        self.ptr = Dev("wb-test-pointer", keys=[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE], rels=[REL_WHEEL, REL_HWHEEL], abs_axes=[ABS_X, ABS_Y])
        niri("action", "focus-workspace", "2")
        time.sleep(0.6)
        env = dict(os.environ, WALLBOARD_DEBUG="1", WALLBOARD_DATA_DIR=self.data_dir)
        self.proc = subprocess.Popen([self.binary], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1, env=env)
        os.set_blocking(self.proc.stdout.fileno(), False)
        self.pump(1.5)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.kill()
        for d in (self.kb, self.ptr):
            if d:
                d.close()
        if self.home is not None:
            niri("action", "focus-workspace", str(self.home))
        if self._own_dir:
            shutil.rmtree(self.data_dir, ignore_errors=True)

    # -- log
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            try:
                l = self.proc.stdout.readline()
            except Exception:
                l = ""
            if l:
                self.lines.append(l.rstrip())
            else:
                time.sleep(0.01)

    def state(self):
        """The most recent [state] line from the app, parsed."""
        for l in reversed(self.lines):
            m = STATE.match(l)
            if m:
                return json.loads(m.group(1))
        return None

    def area(self, name):
        st = self.state()
        return next((a for a in st["areas"] if a["name"] == name), None) if st else None

    def scr(self, wx, wy):
        """World point -> screen point using the latest camera."""
        c = self.state()["camera"]
        return ((wx - c["x"]) * c["zoom"], (wy - c["y"]) * c["zoom"])

    def scr_c(self, rect):
        """Centre of a world-space [x, y, w, h] rect, in screen coordinates."""
        return self.scr(rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)

    def scr_at(self, rect, fx, fy):
        """A point at fractions (fx, fy) inside a world-space rect, in screen coordinates."""
        return self.scr(rect[0] + rect[2] * fx, rect[1] + rect[3] * fy)

    # -- pointer
    def goto(self, x, y):
        self.pos[:] = [x, y]
        self.ptr.emit(EV_ABS, ABS_X, int(x / SCREEN[0] * ABS_MAX))
        self.ptr.emit(EV_ABS, ABS_Y, int(y / SCREEN[1] * ABS_MAX))
        self.ptr.sync(); time.sleep(0.03)

    def drag(self, x0, y0, x1, y1, steps=12, button=BTN_LEFT):
        self.goto(x0, y0); self.pump(0.1)
        self.button(button, True)
        for i in range(1, steps + 1):
            self.goto(x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps)
        self.pump(0.1)
        self.button(button, False); self.pump(0.2)

    def button(self, code, down):
        self.ptr.emit(EV_KEY, code, 1 if down else 0); self.ptr.sync(); time.sleep(0.05)

    def click(self, x, y):
        self.goto(x, y); self.pump(0.1)
        self.button(BTN_LEFT, True); self.button(BTN_LEFT, False); self.pump(0.2)

    def double_click(self, x, y):
        self.goto(x, y); self.pump(0.1)
        for _ in range(2):
            self.button(BTN_LEFT, True); self.button(BTN_LEFT, False)
        self.pump(0.3)

    def rclick(self, x, y):
        self.goto(x, y); self.pump(0.1)
        self.button(BTN_RIGHT, True); self.button(BTN_RIGHT, False); self.pump(0.25)

    def wheel(self, n, ctrl=False, shift=False, horizontal=False):
        for mod, on in (("ctrl", ctrl), ("shift", shift)):
            if on:
                self.kb.emit(EV_KEY, KEYS[mod], 1); self.kb.sync()
        if ctrl or shift:
            time.sleep(0.08)
        axis = REL_HWHEEL if horizontal else REL_WHEEL
        for _ in range(abs(n)):
            self.ptr.emit(EV_REL, axis, 1 if n > 0 else -1); self.ptr.sync(); time.sleep(0.04)
        for mod, on in (("ctrl", ctrl), ("shift", shift)):
            if on:
                self.kb.emit(EV_KEY, KEYS[mod], 0); self.kb.sync()
        self.pump(0.3)

    # -- keyboard
    def key(self, name, hold=0.04, shift=False):
        if shift:
            self.kb.emit(EV_KEY, KEYS["shift"], 1); self.kb.sync(); time.sleep(0.03)
        self.kb.emit(EV_KEY, KEYS[name], 1); self.kb.sync(); time.sleep(hold)
        self.kb.emit(EV_KEY, KEYS[name], 0); self.kb.sync()
        if shift:
            self.kb.emit(EV_KEY, KEYS["shift"], 0); self.kb.sync()
        self.pump(0.1)  # let the app answer so the next read of the log is current

    def type(self, s):
        for ch in s:
            if ch == " ":
                self.key("space")
            elif ch == "\n":
                self.key("enter")
            elif ch == "!":
                self.key("1", shift=True)
            elif ch.isupper():
                self.key(ch.lower(), shift=True)
            else:
                self.key(ch)
        self.pump(0.2)

    def ctrl(self, name, shift=False):
        self.kb.emit(EV_KEY, KEYS["ctrl"], 1); self.kb.sync(); time.sleep(0.05)
        self.key(name, shift=shift)
        self.kb.emit(EV_KEY, KEYS["ctrl"], 0); self.kb.sync()
        self.pump(0.15)

    # -- screen
    def shot(self, name):
        path = os.path.join(self.shots_dir, name)
        niri("action", "screenshot-screen", "--path", path, "--show-pointer", "false")
        time.sleep(0.8)
        return os.path.exists(path)

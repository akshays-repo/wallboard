#!/usr/bin/env python3
"""End-to-end test for the life-OS board. Real input via /dev/uinput, real niri, real screen.
Uses throw-away data directories only.

Usage: python3 -I tests/e2e_lifeos.py [binary] [screenshot_dir]
"""
import glob, json, os, shutil, struct, sys, tempfile, time, zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wbtest import Session, rss_kib, cpu_ticks  # noqa: E402

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/wallboard"
SHOTS = sys.argv[2] if len(sys.argv) > 2 else "/tmp"
NAMES = ["Inbox", "Focus", "Work", "Startups", "Health", "Learning", "Life"]
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""))


def near(a, b, tol=1.5):
    return abs(a - b) <= tol


def texts(area):
    return [t["text"] for t in area["todos"]]


def quit_and_wait(s):
    s.ctrl("q")
    try:
        s.proc.wait(timeout=3)
        ok = True
    except Exception:
        ok = False
    s.pump(0.3)
    return ok


def read_json(d):
    with open(os.path.join(d, "board.json")) as f:
        return json.load(f)


def make_png(path, w=96, h=54):
    """A small RGB gradient PNG, stdlib only."""
    rows = b""
    for y in range(h):
        rows += b"\x00" + b"".join(bytes((int(255 * x / w), int(255 * y / h), 160)) for x in range(w))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def current(s):
    return s.state()["current_area"]


def click_footer(s, name):
    a = s.area(name)
    x, y = s.scr_c(a["footer"])
    s.click(x + 60, y)  # right of the "New todo" label, still the footer row


# ------------------------------------------------------------------------------------------------
def main_session(data):
    s = Session(BIN, SHOTS, data)
    s.start()
    out = {}
    try:
        st = s.state()
        # ---------- startup layout
        check("seven default areas, left to right", [a["name"] for a in st["areas"]] == NAMES, str([a["name"] for a in st["areas"]]))
        xs = [a["card"][0] for a in st["areas"]]
        check("areas are ordered left to right without overlap", all(xs[i] + st["areas"][i]["card"][2] <= xs[i + 1] for i in range(6)))
        focus = s.area("Focus")
        check("Focus is the hero: wider than the others", focus["hero"] and focus["card"][2] > s.area("Work")["card"][2],
              f"{focus['card'][2]} vs {s.area('Work')['card'][2]}")
        check("first run opens on Focus", st["current_area"] == 1 and st["camera"]["zoom"] <= 1.2 + 1e-6, str((st["current_area"], st["camera"]["zoom"])))
        check("toolbar (6 tools), chips (7) and dock (3) are present",
              [t["tool"] for t in st["tools"]] == ["Select", "Group", "Todo", "Note", "Pen", "Eraser"] and len(st["chips"]) == 7 and len(st["dock"]) == 3)
        check("starts in dark theme with the Select tool", st["theme"] == "Dark" and st["tool"] == "Select")
        s.shot("l1_start.png")

        # ---------- quick reach
        s.key("3")
        cx, cy = s.scr_c(s.area("Work")["card"])
        check("key 3 jumps to the 3rd area and centres it", current(s) == 2 and near(cx, 960, 3) and near(cy, 540, 3), f"{cx:.1f},{cy:.1f}")
        chip = next(c for c in s.state()["chips"] if c["area"] == 4)["rect"]
        s.click(chip[0] + chip[2] / 2, chip[1] + chip[3] / 2)
        check("clicking a chip jumps to that area", current(s) == 4)
        s.key("1")
        check("key 1 jumps to Inbox", current(s) == 0)
        s.key("9")
        check("a key with no matching area does nothing", current(s) == 0)

        # ---------- capture
        s.key("c")
        st = s.state()
        inbox = s.area("Inbox")
        check("C captures into Inbox and starts editing the new todo",
              len(inbox["todos"]) == 1 and st["editing"] and st["editing"]["target"].startswith("TodoText") and st["current_area"] == 0,
              str(st["editing"]))
        s.type("buy milk")
        check("typed text lands in the todo", texts(s.area("Inbox")) == ["buy milk"])
        s.key("enter")
        check("Enter adds a new todo below", texts(s.area("Inbox")) == ["buy milk", ""] and s.state()["editing"])
        s.key("enter")
        check("Enter on an empty todo discards it and stops editing", texts(s.area("Inbox")) == ["buy milk"] and s.state()["editing"] is None,
              str(texts(s.area("Inbox"))))

        # ---------- circle, priority
        row = s.area("Inbox")["rows"][0]
        c = s.scr_c(row["circle"])
        s.click(*c)
        check("clicking the circle completes the todo", s.area("Inbox")["todos"][0]["done"] is True)
        check("completing does not start editing", s.state()["editing"] is None)
        s.click(*c)
        check("clicking again re-opens it", s.area("Inbox")["todos"][0]["done"] is False)
        p = s.scr_c(s.area("Inbox")["rows"][0]["prio"])
        got = []
        for _ in range(4):
            s.click(*p)
            got.append(s.area("Inbox")["todos"][0]["priority"])
        check("clicking the right edge cycles priority 1, 2, 3, then 0", got == [1, 2, 3, 0], str(got))
        s.click(*p); s.click(*p)  # leave it at priority 2

        # ---------- more todos, right-click removal
        for word in ("alpha", "beta"):
            s.key("c"); s.type(word); s.key("esc")
        check("three todos in Inbox", texts(s.area("Inbox")) == ["buy milk", "alpha", "beta"], str(texts(s.area("Inbox"))))
        r = s.area("Inbox")["rows"][1]
        s.rclick(*s.scr_c(r["text"]))
        check("right-click removes a todo", texts(s.area("Inbox")) == ["buy milk", "beta"], str(texts(s.area("Inbox"))))
        s.rclick(*s.scr_c(s.area("Inbox")["title"]))
        check("right-click on a group with todos keeps it", s.area("Inbox") is not None)
        s.shot("l2_inbox.png")

        # ---------- Focus holds at most three open todos
        s.key("2")
        for word in ("one", "two", "three"):
            click_footer(s, "Focus"); s.type(word)
        s.key("esc")
        check("Focus holds three open todos", texts(s.area("Focus")) == ["one", "two", "three"], str(texts(s.area("Focus"))))
        click_footer(s, "Focus"); s.type("four"); s.key("esc")
        check("a 4th open todo in Focus is refused", len(s.area("Focus")["todos"]) == 3, str(texts(s.area("Focus"))))
        s.click(*s.scr_c(s.area("Focus")["rows"][0]["circle"]))
        check("finishing one makes room", s.area("Focus")["todos"][0]["done"] is True)
        click_footer(s, "Focus"); s.type("four"); s.key("esc")
        check("so a new open todo is accepted again", len(s.area("Focus")["todos"]) == 4, str(texts(s.area("Focus"))))
        s.click(*s.scr_c(s.area("Focus")["rows"][0]["circle"]))
        check("re-opening a done todo in a full Focus is refused", s.area("Focus")["todos"][0]["done"] is True)
        s.shot("l3_focus.png")

        # ---------- details (body)
        s.key("3")
        click_footer(s, "Work"); s.type("Write report")
        h0 = s.area("Work")["card"][3]
        s.key("tab")
        check("Tab opens the details field", s.state()["editing"]["target"].startswith("TodoBody"), str(s.state()["editing"]))
        s.type("Q3 numbers"); s.key("enter"); s.type("and charts")
        body = s.area("Work")["todos"][0].get("body")
        check("details are multi-line (Enter is a newline)", body == "Q3 numbers\nand charts", repr(body))
        check("the row grows to show its details", s.area("Work")["card"][3] > h0 and s.area("Work")["rows"][0]["body"] is not None,
              f"{h0} -> {s.area('Work')['card'][3]}")
        s.key("tab", shift=True)
        check("Shift+Tab goes back to the title", s.state()["editing"]["target"].startswith("TodoText"))
        s.key("esc")
        bx = s.scr_c(s.area("Work")["rows"][0]["body"])
        s.click(*bx)
        check("clicking the details edits them", s.state()["editing"] and s.state()["editing"]["target"].startswith("TodoBody"))
        s.key("esc")
        s.shot("l4_body.png")

        # ---------- Ctrl+Enter / Ctrl+P while editing
        s.click(*s.scr_at(s.area("Work")["rows"][0]["text"], 0.2, 0.5))
        s.ctrl("enter")
        check("Ctrl+Enter toggles done while editing", s.area("Work")["todos"][0]["done"] is True)
        s.ctrl("p")
        check("Ctrl+P cycles priority while editing", s.area("Work")["todos"][0]["priority"] == 1)
        s.key("esc")

        # ---------- groups
        s.key("0")
        st = s.state()
        sw, sh = st["view"]
        free = (sw * 0.5, sh * 0.78)  # empty canvas well below the cards
        s.double_click(*free)
        st = s.state()
        check("double-click on empty space creates a group and edits its title",
              len(st["areas"]) == 8 and st["editing"] and st["editing"]["target"].startswith("AreaTitle"), str(len(st["areas"])))
        for _ in range(9):
            s.key("backspace")
        s.type("Reading"); s.key("enter")
        g = s.area("Reading")
        check("the group can be named", g is not None and s.state()["editing"] is None)
        x0, y0 = g["x"], g["y"]
        zoom = s.state()["camera"]["zoom"]
        tx, ty = s.scr_c(g["title"])
        s.drag(tx, ty, tx + 120, ty + 40)
        g = s.area("Reading")
        check("a group moves when dragged by its title", near(g["x"] - x0, 120 / zoom, 2) and near(g["y"] - y0, 40 / zoom, 2),
              f"d=({g['x']-x0:.1f},{g['y']-y0:.1f}) expected ({120/zoom:.1f},{40/zoom:.1f})")
        c0 = g["color"]
        s.click(*s.scr_c(g["dot"]))
        check("the colour dot cycles the group colour", s.area("Reading")["color"] == (c0 + 1) % 10)
        s.double_click(*s.scr_c(s.area("Reading")["title"]))
        check("double-clicking a title renames it", s.state()["editing"] and s.state()["editing"]["target"].startswith("AreaTitle"))
        s.key("esc")

        # ---------- notes via the toolbar
        tool = next(t for t in s.state()["tools"] if t["tool"] == "Note")["rect"]
        s.click(tool[0] + tool[2] / 2, tool[1] + tool[3] / 2)
        check("toolbar button arms the Note tool", s.state()["tool"] == "Note")
        s.click(sw * 0.78, sh * 0.62)
        st = s.state()
        check("clicking empty canvas places a note (title edit) and returns to Select",
              len(st["notes"]) == 1 and st["tool"] == "Select" and st["editing"]["target"].startswith("NoteTitle"), str((len(st["notes"]), st["tool"])))
        s.type("Idea"); s.key("enter"); s.type("line one"); s.key("enter"); s.type("line two")
        n = s.state()["notes"][0]
        check("note title and body are set separately", n["title"] == "Idea" and n["body"] == "line one\nline two", str((n["title"], n["body"])))
        s.key("esc")
        n = s.state()["notes"][0]
        zoom = s.state()["camera"]["zoom"]
        tx, ty = s.scr_c(n["title_rect"])
        s.drag(tx, ty, tx - 90, ty + 30)
        n2 = s.state()["notes"][0]
        check("a note moves when dragged by its title", near(n2["x"] - n["x"], -90 / zoom, 2) and near(n2["y"] - n["y"], 30 / zoom, 2),
              f"({n2['x']-n['x']:.1f},{n2['y']-n['y']:.1f})")
        s.shot("l5_note.png")

        # ---------- pen and eraser
        for nm in ("Pen",):
            t = next(t for t in s.state()["tools"] if t["tool"] == nm)["rect"]
            s.click(t[0] + t[2] / 2, t[1] + t[3] / 2)
        check("toolbar button arms the Pen", s.state()["tool"] == "Pen")
        s.drag(sw * 0.2, sh * 0.82, sw * 0.4, sh * 0.9, steps=24)
        sk = s.state()["strokes"]
        check("pen draws one smoothed stroke", len(sk) == 1 and sk[0]["n"] > 12, str(sk))
        check("pen stays armed after a stroke", s.state()["tool"] == "Pen")
        t = next(t for t in s.state()["tools"] if t["tool"] == "Eraser")["rect"]
        s.click(t[0] + t[2] / 2, t[1] + t[3] / 2)
        s.drag(sw * 0.3 - 40, sh * 0.76, sw * 0.3 + 40, sh * 0.96, steps=20)
        check("eraser removes the stroke it crosses", len(s.state()["strokes"]) == 0)
        s.ctrl("z")
        check("erasing is undoable", len(s.state()["strokes"]) == 1)
        s.key("v")
        check("V returns to Select", s.state()["tool"] == "Select")
        s.shot("l6_pen.png")

        # ---------- navigation
        s.key("0"); s.pump(0.2)
        c = s.state()["camera"]
        s.goto(sw * 0.5, sh * 0.6); s.pump(0.1)
        s.wheel(+3)
        c2 = s.state()["camera"]
        check("scroll pans vertically", c2["y"] < c["y"] - 5 and near(c2["x"], c["x"], 0.01) and near(c2["zoom"], c["zoom"], 1e-6),
              f"dy={c2['y']-c['y']:.1f}")
        s.wheel(-3)
        c2 = s.state()["camera"]  # baseline after undoing the vertical scroll
        s.wheel(+2, shift=True)
        c3 = s.state()["camera"]
        check("Shift+scroll pans horizontally only", abs(c3["x"] - c2["x"]) > 5 and near(c3["y"], c2["y"], 0.01), f"dx={c3['x']-c2['x']:.1f} dy={c3['y']-c2['y']:.2f}")
        s.wheel(+2, horizontal=True)
        c4 = s.state()["camera"]
        check("horizontal scroll pans horizontally", abs(c4["x"] - c3["x"]) > 5, f"dx={c4['x']-c3['x']:.1f}")
        px, py = sw * 0.45, sh * 0.55
        s.goto(px, py); s.pump(0.1)
        before = s.state()["camera"]
        w0 = (before["x"] + px / before["zoom"], before["y"] + py / before["zoom"])
        s.wheel(+3, ctrl=True)
        after = s.state()["camera"]
        w1 = (after["x"] + px / after["zoom"], after["y"] + py / after["zoom"])
        check("Ctrl+scroll zooms in around the cursor", after["zoom"] > before["zoom"] * 1.2 and near(w0[0], w1[0], 1.0) and near(w0[1], w1[1], 1.0),
              f"zoom {before['zoom']:.3f}->{after['zoom']:.3f}, fixed point moved ({w1[0]-w0[0]:.2f},{w1[1]-w0[1]:.2f})")
        s.wheel(-3, ctrl=True)
        check("Ctrl+scroll the other way zooms out", s.state()["camera"]["zoom"] < after["zoom"])
        z0 = s.state()["camera"]["zoom"]
        s.key("equal", shift=True)
        check("+ zooms in", s.state()["camera"]["zoom"] > z0 * 1.1)
        s.key("minus")
        check("- zooms out", near(s.state()["camera"]["zoom"], z0, 0.01), f"{s.state()['camera']['zoom']:.4f} vs {z0:.4f}")
        cam = s.state()["camera"]
        # drag on empty canvas (free area) to pan
        s.drag(sw * 0.7, sh * 0.9, sw * 0.7 - 80, sh * 0.9 - 30, steps=10)
        cam2 = s.state()["camera"]
        check("dragging empty canvas pans (content follows the pointer)", near(cam2["x"] - cam["x"], 80 / cam["zoom"], 2) and near(cam2["y"] - cam["y"], 30 / cam["zoom"], 2),
              f"d=({cam2['x']-cam['x']:.1f},{cam2['y']-cam['y']:.1f})")
        s.key("0")
        st = s.state()
        inside = all(
            0 <= s.scr(a["card"][0], a["card"][1])[0] and s.scr(a["card"][0] + a["card"][2], a["card"][1] + a["card"][3])[0] <= sw
            and 0 <= s.scr(a["card"][0], a["card"][1])[1] and s.scr(a["card"][0], a["card"][1] + a["card"][3])[1] <= sh
            for a in st["areas"]
        )
        check("0 fits everything on screen", inside and st["camera"]["zoom"] < 1.0, f"zoom={st['camera']['zoom']:.3f}")

        # ---------- theme and background
        s.key("l")
        check("L switches to the light theme", s.state()["theme"] == "Light")
        s.shot("l7_light.png")
        s.key("l")
        check("L switches back to dark", s.state()["theme"] == "Dark")
        seen = []
        for _ in range(7):
            s.key("b")
            seen.append(s.state()["background"]["color"])
        check("B cycles six preset colours then returns to the theme colour",
              seen == ["#0B0F19", "#14211A", "#241A2E", "#2B2118", "#E8E2D4", "#DCE6F2", None], str(seen))
        s.key("b"); s.key("b")  # leave a colour set: #14211A
        s.shot("l8_bg.png")

        # ---------- undo / redo across creation and typing
        n_areas = len(s.state()["areas"])
        s.key("c"); s.type("undo me")
        check("typed a todo to undo", "undo me" in texts(s.area("Inbox")))
        s.ctrl("z")
        check("Ctrl+Z undoes the typing as one step", "undo me" not in texts(s.area("Inbox")) and s.area("Inbox") is not None)
        s.ctrl("z", shift=True)
        check("Ctrl+Shift+Z redoes it", "undo me" in texts(s.area("Inbox")))
        s.key("esc")

        # ---------- idle
        s.pump(3.5)
        t0 = cpu_ticks(s.proc.pid); time.sleep(4); t1 = cpu_ticks(s.proc.pid)
        idle = (t1 - t0) / os.sysconf("SC_CLK_TCK") / 4 * 100
        check(f"idle CPU ~0 ({idle:.2f}%)", idle < 0.5)
        out["rss"] = rss_kib(s.proc.pid)

        # ---------- persistence on disk
        s.pump(1.5)
        j = read_json(data)
        check("save file: version 2 with areas, notes, strokes, launchers, camera, theme, background",
              j["version"] == 2 and len(j["areas"]) == n_areas and len(j["notes"]) == 1 and len(j["strokes"]) == 1
              and len(j["launchers"]) == 3 and "zoom" in j["camera"] and j["theme"] == "dark" and j["background"]["color"] == "#14211A",
              str({k: (len(v) if isinstance(v, list) else v) for k, v in j.items() if k in ("areas", "notes", "strokes", "launchers")}))
        files = sorted(os.listdir(data))
        check("no temp file left behind", not any(f.endswith(".tmp") for f in files), str(files))
        out["json"] = j
        out["state"] = s.state()
        ok_exit = quit_and_wait(s)
        check("Ctrl+Q exits cleanly", ok_exit)
    finally:
        s.stop()
    return out


def restart_session(data, prev):
    s = Session(BIN, SHOTS, data)
    s.start()
    try:
        st = s.state()
        old = prev["state"]
        check("restart: every area and its todos restored",
              [(a["name"], texts(a)) for a in st["areas"]] == [(a["name"], texts(a)) for a in old["areas"]])
        check("restart: ticks, priorities and details restored",
              [a["todos"] for a in st["areas"]] == [a["todos"] for a in old["areas"]])
        check("restart: free note and stroke restored", len(st["notes"]) == 1 and st["notes"][0]["title"] == "Idea" and len(st["strokes"]) == 1)
        cam, oc = st["camera"], prev["json"]["camera"]
        check("restart: camera restored (not re-focused)", near(cam["x"], oc["x"], 0.05) and near(cam["zoom"], oc["zoom"], 1e-3), f"{cam} vs {oc}")
        check("restart: theme and background restored", st["theme"] == "Dark" and st["background"]["color"] == "#14211A")
        bak = os.path.join(data, "board.json.bak")
        check("restart: .bak is a copy of the file that just loaded", os.path.exists(bak) and open(bak, "rb").read() == open(os.path.join(data, "board.json"), "rb").read())
        mtime = os.path.getmtime(os.path.join(data, "board.json"))
        s.pump(2.0)
        check("viewing without editing does not rewrite the file", os.path.getmtime(os.path.join(data, "board.json")) == mtime)
        s.key("c"); s.type("after restart"); s.key("esc")
        s.pump(1.6)
        j = read_json(data)
        ids = [a["id"] for a in j["areas"]] + [n["id"] for n in j["notes"]] + [k["id"] for k in j["strokes"]]
        check("edits after a restart save, and ids stay unique", "after restart" in [t["text"] for a in j["areas"] for t in a["todos"]] and len(ids) == len(set(ids)))
        s.pump(3.0)  # let the idle shrink run before measuring
        print("   RSS after load + idle:", rss_kib(s.proc.pid), "KiB")
        quit_and_wait(s)
    finally:
        s.stop()


def bad_file_sessions():
    d = tempfile.mkdtemp(prefix="wallboard-e2e-bad-")
    path = os.path.join(d, "board.json")
    cases = [
        ("garbage file", b"{ definitely not json", "not valid JSON"),
        ("old version", b'{"version":1,"items":[]}', "unsupported version 1"),
        ("wrong structure", b'{"version":2,"areas":[{"nope":true}]}', "unexpected structure"),
    ]
    made = []
    try:
        for i, (label, content, expect) in enumerate(cases):
            with open(path, "wb") as f:
                f.write(content)
            s = Session(BIN, SHOTS, d)
            s.start()
            try:
                st = s.state()
                check(f"{label}: starts with a fresh default board, no crash", st and [a["name"] for a in st["areas"]] == NAMES and s.proc.poll() is None)
                log = "\n".join(s.lines)
                check(f"{label}: says why", "wallboard: ignoring" in log and expect in log, [l for l in s.lines if "wallboard:" in l][:1].__str__())
                suffix = ".bad" if i == 0 else f".bad.{i}"
                bp = path + suffix
                check(f"{label}: original kept as board.json{suffix}, byte for byte", os.path.exists(bp) and open(bp, "rb").read() == content)
                # The app may write a fresh file about 1 s after start (first-run camera). That must be
                # a valid default board, and the bad file's bytes must live only in the .bad file.
                fresh_ok = (not os.path.exists(path)) or (read_json(d)["version"] == 2 and open(path, "rb").read() != content)
                check(f"{label}: a fresh board.json (if written) is valid and the bad bytes are not reused", fresh_ok)
                made.append((bp, content))
                s.key("c"); s.type("fresh"); s.key("esc"); s.pump(1.6)
                j = read_json(d)
                check(f"{label}: new edits save a valid file", j["version"] == 2 and any("fresh" == t["text"] for a in j["areas"] for t in a["todos"]))
                quit_and_wait(s)
            finally:
                s.stop()
        check("all earlier .bad files are still intact", all(os.path.exists(p) and open(p, "rb").read() == c for p, c in made))
    finally:
        shutil.rmtree(d, ignore_errors=True)


def wallpaper_and_launcher_sessions():
    d = tempfile.mkdtemp(prefix="wallboard-e2e-wp-")
    marker = os.path.join(d, "launched.marker")
    png = os.path.join(d, "wp.png")
    make_png(png)
    board = {
        "version": 2,
        "areas": [{"id": 1, "name": "Inbox", "x": 0, "y": 0, "todos": [{"text": "hello"}]}],
        "launchers": [{"label": "Touch", "command": f"touch {marker}"}],
        "background": {"image": png, "dim": 0.3},
    }
    with open(os.path.join(d, "board.json"), "w") as f:
        json.dump(board, f)
    try:
        s = Session(BIN, SHOTS, d)
        s.start()
        try:
            st = s.state()
            check("custom launcher list from the file shows in the dock", len(st["dock"]) == 1)
            r = st["dock"][0]["rect"]
            check("nothing launched yet", not os.path.exists(marker))
            s.click(r[0] + r[2] / 2, r[1] + r[3] / 2)
            time.sleep(0.8)
            check("clicking a dock button runs its command detached", os.path.exists(marker))
            seen = []
            for _ in range(7):
                s.key("b")
                s.pump(0.5)  # the wallpaper frame decodes and scales a PNG first
                seen.append(s.state()["background"]["image_on"])
            st = s.state()
            check("B reaches the wallpaper after the colours", seen == [False] * 6 + [True] and st["wallpaper_loaded"], str(seen))
            s.pump(0.5)
            s.shot("l9_wallpaper.png")
            rss_wp = rss_kib(s.proc.pid)
            s.pump(3.5)
            rss_idle = rss_kib(s.proc.pid)
            check("the wallpaper cache is freed while idle", rss_idle < rss_wp - 4000, f"{rss_wp} -> {rss_idle} KiB")
            s.key("b")
            check("B after the wallpaper returns to the theme colour", s.state()["background"]["image_on"] is False and s.state()["background"]["color"] is None)
            s.key("b")  # a colour again (so the saved file has a known state)
            quit_and_wait(s)
        finally:
            s.stop()

        # wallpaper.png in the data directory is picked up without any JSON edit
        shutil.copy(png, os.path.join(d, "wallpaper.png"))
        j = json.load(open(os.path.join(d, "board.json")))
        j["background"] = {}
        json.dump(j, open(os.path.join(d, "board.json"), "w"))
        s = Session(BIN, SHOTS, d)
        s.start()
        try:
            for _ in range(7):
                s.key("b")
                s.pump(0.5)
            check("wallpaper.png in the data folder is used automatically", s.state()["background"]["image_on"] and s.state()["wallpaper_loaded"])
            quit_and_wait(s)
        finally:
            s.stop()

        # a JPEG wallpaper must fail politely, not crash
        jpg = os.path.join(d, "x.jpg")
        open(jpg, "wb").write(b"\xff\xd8\xff\xe0" + b"\0" * 16)
        os.remove(os.path.join(d, "wallpaper.png"))
        j["background"] = {"image": jpg, "image_on": True}
        json.dump(j, open(os.path.join(d, "board.json"), "w"))
        s = Session(BIN, SHOTS, d)
        s.start()
        try:
            log = "\n".join(s.lines)
            check("JPEG wallpaper: no crash, clear message, falls back to a colour",
                  s.proc.poll() is None and "JPEG" in log and "PNG" in log and s.state()["background"]["image_on"] is False,
                  [l for l in s.lines if "wallpaper" in l][:1].__str__())
            quit_and_wait(s)
        finally:
            s.stop()
    finally:
        shutil.rmtree(d, ignore_errors=True)


def main():
    data = tempfile.mkdtemp(prefix="wallboard-e2e-main-")
    try:
        prev = main_session(data)
        restart_session(data, prev)
        bad_file_sessions()
        wallpaper_and_launcher_sessions()
        print(f"\nRSS idle in the main session: {prev['rss']} KiB")
    finally:
        shutil.rmtree(data, ignore_errors=True)
    print(f"{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


main()

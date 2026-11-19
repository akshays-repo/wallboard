#!/usr/bin/env python3
"""End-to-end test of the git backup with the real binary and a LOCAL bare repository
(no network, no real GitHub). Takes about 3 minutes because it waits out real timers (shortened via the file's backup settings).

Usage: python3 -I tests/e2e_backup.py [binary] [screenshot_dir]
"""
import os, shutil, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wbtest import Session  # noqa: E402

BIN = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/wallboard")
SHOTS = sys.argv[2] if len(sys.argv) > 2 else "/tmp"
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""))


def git(d, *args):
    return subprocess.run(["git", "-C", d, *args], capture_output=True, text=True, check=True).stdout.strip()


def remote_count(bare):
    return len(git(bare, "log", "--format=%s", "main").splitlines())


def cli(data, *args):
    env = dict(os.environ, WALLBOARD_DATA_DIR=data)
    return subprocess.run([BIN, *args], capture_output=True, text=True, env=env, timeout=60)


def main():
    root = tempfile.mkdtemp(prefix="wallboard-e2e-git-")
    bare = os.path.join(root, "remote.git")
    data = os.path.join(root, "data")
    os.makedirs(bare)
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", bare], check=True)
    try:
        # ---------- command line
        h = cli(data, "--help")
        check("--help prints usage and the PRIVATE warning", h.returncode == 0 and "PRIVATE" in h.stdout and "--init-sync" in h.stdout)
        bad = cli(data, "--init-sync", "-oProxyCommand=evil")
        check("--init-sync refuses an option-looking URL", bad.returncode == 1 and "unsupported remote URL" in bad.stderr, bad.stderr.strip()[:80])
        ext = cli(data, "--init-sync", "ext::sh -c touch /tmp/pwned")
        check("--init-sync refuses command-running transports", ext.returncode == 1 and not os.path.exists("/tmp/pwned"))
        noarg = cli(data, "--init-sync")
        check("--init-sync without a URL explains itself", noarg.returncode == 2 and "needs the remote URL" in noarg.stderr)
        unknown = cli(data, "--frobnicate")
        check("unknown options are rejected", unknown.returncode == 2)
        ok = cli(data, "--init-sync", bare)
        check("--init-sync sets the folder up and pushes", ok.returncode == 0 and "PRIVATE" in ok.stdout and "pushed main" in ok.stdout,
              ok.stdout.strip().splitlines()[-1] if ok.stdout.strip() else ok.stderr)
        check("remote has the initial commit and board.json", remote_count(bare) == 1 and '"version": 2' in git(bare, "show", "main:board.json"))
        gi = open(os.path.join(data, ".gitignore")).read().split()
        check(".gitignore covers *.bak, *.bad and *.tmp", all(p in gi for p in ("*.bak", "*.bad", "*.tmp")), str(gi))

        # ---------- the default cadence is every 10 minutes
        import json
        board_path = os.path.join(data, "board.json")
        j = json.load(open(board_path))
        check("the saved default is one backup every 10 minutes (600 s)", j["backup"]["interval_secs"] == 600 and j["backup"]["debounce_secs"] == 30, str(j["backup"]))
        # Shorten the cadence so the test does not take 10 minutes: interval 20 s, debounce 4 s.
        j["backup"] = {"interval_secs": 20, "debounce_secs": 4}
        json.dump(j, open(board_path, "w"), indent=2)

        # ---------- the running app
        s = Session(BIN, SHOTS, data)
        s.start()
        try:
            # view-only changes: pan, zoom, theme
            s.goto(960, 540)
            s.wheel(+4)
            s.wheel(+2, ctrl=True)
            s.key("l")
            s.pump(1.8)
            check("view-only changes are saved to disk", "camera" in open(board_path).read())
            print("   view-only changes: waiting well past the debounce ...")
            time.sleep(9)
            check("pan/zoom/theme changes never create a commit", remote_count(bare) == 1 and len(git(data, "log", "--format=%s").splitlines()) == 1,
                  f"remote={remote_count(bare)}")

            # content change 1: first backup is not held back by the interval
            s.key("c"); s.type("first backed up edit"); s.key("esc")
            s.pump(1.8)
            check("saved to disk within ~1 s", "first backed up edit" in open(board_path).read())
            check("not pushed immediately (debounce)", remote_count(bare) == 1)
            time.sleep(6)
            check("pushed shortly after the debounce", remote_count(bare) == 2 and "first backed up edit" in git(bare, "show", "main:board.json"),
                  f"remote={remote_count(bare)}")
            check("the commit message says it is a wallboard backup", git(bare, "log", "-1", "--format=%s", "main").startswith("wallboard backup"))
            t_attempt = time.time()  # the first attempt just happened (within ~2 s of now)

            # content change 2 right away: must wait for the interval
            s.key("c"); s.type("second edit within the interval"); s.key("esc")
            s.pump(1.8)
            time.sleep(7)  # debounce has long passed, interval (20 s) has not
            check("a second change within the interval is held back", remote_count(bare) == 2, f"remote={remote_count(bare)}")
            print("   waiting for the interval to elapse ...")
            time.sleep(max(0, 28 - (time.time() - t_attempt)))
            s.pump(0.5)
            check("it is pushed once the interval has elapsed", remote_count(bare) == 3 and "second edit within the interval" in git(bare, "show", "main:board.json"),
                  f"remote={remote_count(bare)}")
            tracked = git(data, "ls-files").split()
            check("only board.json and .gitignore are tracked", sorted(tracked) == [".gitignore", "board.json"], str(tracked))

            # a change right before quitting
            s.key("c"); s.type("typed just before quitting"); s.key("esc")
            s.pump(1.8)
            ok_exit = False
            s.ctrl("q")
            try:
                s.proc.wait(timeout=6)
                ok_exit = True
            except Exception:
                pass
            s.pump(0.3)
            check("quitting is quick even with a pending backup", ok_exit)
            local = len(git(data, "log", "--format=%s").splitlines())
            check("work pending at exit is committed locally", local == 4, f"local commits={local}")
            check("but exit does not wait for the network", remote_count(bare) == 3)
        finally:
            s.stop()

        # ---------- a later run pushes what was left behind
        s = Session(BIN, SHOTS, data)
        s.start()
        try:
            print("   waiting 65 s for the startup push of the leftover commit ...")
            time.sleep(66)
            s.pump(0.5)
            check("the next run pushes the leftover commit after ~60 s", remote_count(bare) == 4 and "typed just before quitting" in git(bare, "show", "main:board.json"),
                  f"remote={remote_count(bare)}")
            s.ctrl("q")
            try:
                s.proc.wait(timeout=6)
            except Exception:
                pass
        finally:
            s.stop()
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print(f"\n{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


main()

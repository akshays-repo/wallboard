//! Git backup of `board.json` to a (private!) remote.
//!
//! If the data directory is a git repository, a background thread commits `board.json` and
//! pushes it: at most once every 10 minutes (configurable), 30 s after the last *content*
//! change, retrying later when offline or unauthenticated. It never prompts (`GIT_TERMINAL_PROMPT=0`, ssh
//! `BatchMode`), every git command has a timeout, and pan/zoom-only changes never trigger a
//! commit (the app only calls [`Backup::notify`] when the board content hash changes).
//!
//! `wallboard --init-sync <remote-url>` sets a data directory up as such a repository.

use crate::board::Board;
use crate::store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const DEFAULT_INTERVAL_SECS: u64 = 600;
pub const DEFAULT_DEBOUNCE_SECS: u64 = 30;

/// How often to back up, saved in `board.json` as `"backup": {"interval_secs": 600, ...}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSettings {
    /// At most one backup (commit + push) per this many seconds. Default: 600 (every 10 minutes).
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Wait this long after the last change before backing up, so a burst of edits is one commit.
    #[serde(default = "default_debounce")]
    pub debounce_secs: u64,
}

fn default_interval() -> u64 {
    DEFAULT_INTERVAL_SECS
}

fn default_debounce() -> u64 {
    DEFAULT_DEBOUNCE_SECS
}

impl Default for BackupSettings {
    fn default() -> Self {
        BackupSettings {
            interval_secs: DEFAULT_INTERVAL_SECS,
            debounce_secs: DEFAULT_DEBOUNCE_SECS,
        }
    }
}

impl BackupSettings {
    /// Clamp to sane values: an interval of 5 s to 1 day, a debounce of 1 s to the interval.
    pub fn sane(self) -> BackupSettings {
        let interval_secs = self.interval_secs.clamp(5, 86_400);
        BackupSettings {
            interval_secs,
            debounce_secs: self.debounce_secs.clamp(1, interval_secs),
        }
    }
}

impl Timing {
    pub fn from_settings(s: BackupSettings) -> Timing {
        let s = s.sane();
        Timing {
            debounce: Duration::from_secs(s.debounce_secs),
            min_gap: Duration::from_secs(s.interval_secs),
            // A failed push is retried well before the next interval, but not in a tight loop.
            retry: Duration::from_secs((s.interval_secs / 5).clamp(5, 120)),
            ..Timing::default()
        }
    }
}

/// All the delays in one place so tests can run in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Wait this long after the last content change before committing and pushing.
    pub debounce: Duration,
    /// Never start two sync attempts closer together than this.
    pub min_gap: Duration,
    /// Retry a failed push after this long.
    pub retry: Duration,
    /// After startup, try to push any commits left unpushed by a previous run.
    pub startup: Duration,
    /// Timeout for commands that touch the network (push).
    pub net_timeout: Duration,
    /// Timeout for local git commands.
    pub local_timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            debounce: Duration::from_secs(DEFAULT_DEBOUNCE_SECS),
            min_gap: Duration::from_secs(DEFAULT_INTERVAL_SECS),
            retry: Duration::from_secs(120),
            startup: Duration::from_secs(60),
            net_timeout: Duration::from_secs(40),
            local_timeout: Duration::from_secs(10),
        }
    }
}

/// True if `dir` is a git working tree.
pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

// ---- running git

/// Run `git -C dir <args>` without ever prompting, killing it after `timeout`.
/// Returns (exit code, stdout, stderr).
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<(i32, String, String), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        // BatchMode: fail instead of asking for a passphrase or a host-key confirmation.
        cmd.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=15",
        );
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "git {} timed out after {}s",
                    args.first().unwrap_or(&""),
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("git: {e}")),
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

fn git_ok(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let (code, out, err) = git(dir, args, timeout)?;
    if code == 0 {
        Ok(out)
    } else {
        let first = err.lines().next().unwrap_or("").to_string();
        Err(format!(
            "git {} failed ({code}): {first}",
            args.first().unwrap_or(&"")
        ))
    }
}

/// `-c user.name=.. -c user.email=..` arguments, only when the repo has no identity configured.
fn identity_args(dir: &Path, t: &Timing) -> Vec<String> {
    let has = |key: &str| {
        git(dir, &["config", "--get", key], t.local_timeout)
            .is_ok_and(|(c, out, _)| c == 0 && !out.is_empty())
    };
    let mut a = Vec::new();
    if !has("user.name") {
        a.extend(["-c".to_string(), "user.name=wallboard".to_string()]);
    }
    if !has("user.email") {
        a.extend([
            "-c".to_string(),
            "user.email=wallboard@localhost".to_string(),
        ]);
    }
    a
}

fn commit_message() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("wallboard backup (unix {secs})")
}

/// Stage and commit `board.json` if it changed. Returns whether a commit was made.
fn commit_board(dir: &Path, t: &Timing) -> Result<bool, String> {
    git_ok(dir, &["add", "--", "board.json"], t.local_timeout)?;
    let (code, _, _) = git(
        dir,
        &["diff", "--cached", "--quiet", "--", "board.json"],
        t.local_timeout,
    )?;
    if code == 0 {
        return Ok(false); // nothing changed since the last commit
    }
    let id = identity_args(dir, t);
    let msg = commit_message();
    let mut args: Vec<&str> = id.iter().map(String::as_str).collect();
    args.extend(["commit", "-q", "-m", &msg, "--", "board.json"]);
    git_ok(dir, &args, t.local_timeout)?;
    Ok(true)
}

/// Push the current branch to `origin`. Without a remote this is a no-op (local history only).
fn push(dir: &Path, t: &Timing) -> Result<bool, String> {
    if git(dir, &["remote", "get-url", "origin"], t.local_timeout)?.0 != 0 {
        return Ok(false);
    }
    let branch = git_ok(dir, &["symbolic-ref", "--short", "HEAD"], t.local_timeout)?;
    git_ok(dir, &["push", "-q", "-u", "origin", &branch], t.net_timeout)?;
    Ok(true)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub committed: bool,
    pub pushed: bool,
}

/// One sync attempt: optionally commit `board.json`, then push.
pub fn sync_once(dir: &Path, t: &Timing, commit: bool) -> Result<Outcome, String> {
    let committed = if commit { commit_board(dir, t)? } else { false };
    let pushed = push(dir, t)?;
    Ok(Outcome { committed, pushed })
}

// ---- the background thread

enum Msg {
    Changed,
    Shutdown,
}

pub struct Backup {
    tx: Sender<Msg>,
    done: Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl Backup {
    /// Start the thread if `dir` is a git repository.
    pub fn start(dir: PathBuf, timing: Timing, verbose: bool) -> Option<Backup> {
        if !is_repo(&dir) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("wallboard-backup".into())
            .spawn(move || {
                run(&dir, timing, rx, verbose);
                let _ = done_tx.send(());
            })
            .ok()?;
        Some(Backup {
            tx,
            done,
            handle: Some(handle),
        })
    }

    /// The board *content* changed (not just the view).
    pub fn notify(&self) {
        let _ = self.tx.send(Msg::Changed);
    }
}

impl Drop for Backup {
    fn drop(&mut self) {
        // Ask the thread to commit anything pending locally, and wait briefly for it.
        let _ = self.tx.send(Msg::Shutdown);
        if self.done.recv_timeout(Duration::from_secs(3)).is_ok() {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

fn run(dir: &Path, t: Timing, rx: Receiver<Msg>, verbose: bool) {
    let mut pending_commit = false;
    let mut due: Option<Instant> = Some(Instant::now() + t.startup);
    let mut last_attempt: Option<Instant> = None;
    let mut failing = false;
    loop {
        let wait = due.map_or(Duration::from_secs(3600), |d| {
            d.saturating_duration_since(Instant::now())
        });
        match rx.recv_timeout(wait) {
            Ok(Msg::Changed) => {
                pending_commit = true;
                let now = Instant::now();
                let earliest = last_attempt.map_or(now, |a| a + t.min_gap);
                due = Some((now + t.debounce).max(earliest));
            }
            Ok(Msg::Shutdown) => {
                if pending_commit {
                    let _ = commit_board(dir, &t); // local only: never block exit on the network
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                if due.is_some_and(|d| Instant::now() >= d) {
                    let result = sync_once(dir, &t, pending_commit);
                    last_attempt = Some(Instant::now());
                    match result {
                        Ok(o) => {
                            if verbose {
                                println!("[backup] committed={} pushed={}", o.committed, o.pushed);
                            }
                            pending_commit = false;
                            due = None;
                            failing = false;
                        }
                        Err(e) => {
                            if !failing {
                                eprintln!("wallboard: backup failed: {e} (will retry)");
                            }
                            failing = true;
                            due = Some(Instant::now() + t.retry);
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

// ---- `wallboard --init-sync <url>`

/// Only URL forms that cannot smuggle options or run commands.
pub fn validate_url(url: &str) -> Result<(), String> {
    let ok = url.starts_with("git@")
        || url.starts_with("ssh://")
        || url.starts_with("https://")
        || url.starts_with("file://")
        || url.starts_with('/');
    if url.is_empty()
        || url.starts_with('-')
        || url.contains(char::is_whitespace)
        || url.contains("::")
        || !ok
    {
        return Err(format!(
            "unsupported remote URL {url:?}: use git@host:user/repo.git, ssh://, https:// or an absolute path"
        ));
    }
    Ok(())
}

const GITIGNORE: &str = "# wallboard: only board.json belongs in the backup\n*.bak\n*.bad\n*.bad.*\n*.tmp\nwallpaper.*\n";

/// Turn `dir` into a git repository backing up `board.json` to `url`, and push it.
pub fn init_sync(dir: &Path, url: &str, t: &Timing) -> Result<Vec<String>, String> {
    validate_url(url)?;
    let mut said = Vec::new();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    // Make sure there is a board.json to back up.
    let board_path = dir.join("board.json");
    if !board_path.exists() {
        store::save(
            &board_path,
            &Board::default_board(),
            &store::View::default(),
        )
        .map_err(|e| format!("cannot write {}: {e}", board_path.display()))?;
        said.push(format!("created {} (no board yet)", board_path.display()));
    }

    if !is_repo(dir) {
        git_ok(dir, &["init", "-q", "-b", "main"], t.local_timeout)?;
        said.push("git init (branch main)".to_string());
    }
    let ignore = dir.join(".gitignore");
    let current = std::fs::read_to_string(&ignore).unwrap_or_default();
    if !current.contains("*.bad") {
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(GITIGNORE);
        std::fs::write(&ignore, text).map_err(|e| format!("cannot write .gitignore: {e}"))?;
        said.push("wrote .gitignore (*.bak, *.bad, *.tmp, wallpaper.*)".to_string());
    }
    let has_origin = git(dir, &["remote", "get-url", "origin"], t.local_timeout)?.0 == 0;
    if has_origin {
        git_ok(
            dir,
            &["remote", "set-url", "origin", "--", url],
            t.local_timeout,
        )?;
    } else {
        git_ok(
            dir,
            &["remote", "add", "--", "origin", url],
            t.local_timeout,
        )?;
    }
    said.push(format!("remote origin = {url}"));

    git_ok(
        dir,
        &["add", "--", ".gitignore", "board.json"],
        t.local_timeout,
    )?;
    let (code, _, _) = git(dir, &["diff", "--cached", "--quiet"], t.local_timeout)?;
    if code != 0 {
        let id = identity_args(dir, t);
        let mut args: Vec<&str> = id.iter().map(String::as_str).collect();
        args.extend(["commit", "-q", "-m", "wallboard: initial backup"]);
        git_ok(dir, &args, t.local_timeout)?;
        said.push("committed board.json".to_string());
    }
    let branch = git_ok(dir, &["symbolic-ref", "--short", "HEAD"], t.local_timeout)?;
    git_ok(dir, &["push", "-q", "-u", "origin", &branch], t.net_timeout).map_err(|e| {
        format!("{e}\n(the repository is set up locally; fix the remote or your SSH/HTTPS credentials and run this again)")
    })?;
    said.push(format!("pushed {branch} to origin"));
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fast() -> Timing {
        Timing {
            debounce: Duration::from_millis(250),
            min_gap: Duration::from_millis(900),
            retry: Duration::from_millis(400),
            startup: Duration::from_secs(3600),
            net_timeout: Duration::from_secs(20),
            local_timeout: Duration::from_secs(10),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wallboard-sync-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn g(dir: &Path, args: &[&str]) -> String {
        git_ok(dir, args, Duration::from_secs(20)).unwrap_or_else(|e| panic!("{e}"))
    }

    /// A bare repository standing in for GitHub, plus a data directory.
    fn rig(name: &str) -> (PathBuf, PathBuf) {
        let root = tmp(name);
        let bare = root.join("remote.git");
        fs::create_dir_all(&bare).unwrap();
        g(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        (bare, data)
    }

    fn remote_log(bare: &Path) -> Vec<String> {
        let out = g(bare, &["log", "--format=%s", "main"]);
        out.lines().map(String::from).collect()
    }

    fn remote_file(bare: &Path, name: &str) -> String {
        g(bare, &["show", &format!("main:{name}")])
    }

    #[test]
    fn url_validation_blocks_option_and_command_injection() {
        for ok in [
            "git@github.com:me/wallboard-data.git",
            "ssh://git@host/me/r.git",
            "https://github.com/me/r.git",
            "/srv/repo.git",
            "file:///srv/r.git",
        ] {
            assert!(validate_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-oProxyCommand=evil",
            "ext::sh -c touch /tmp/pwned",
            "git@host:r.git --upload-pack=x",
            "http://insecure/r.git",
            "relative/path",
            "ftp://x/y",
        ] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn git_helper_times_out_and_kills_the_process() {
        let d = tmp("timeout");
        // `git help -w` style hangs are hard to fake; use a config alias that sleeps.
        g(&d, &["init", "-q"]);
        g(&d, &["config", "alias.slow", "!sleep 30"]);
        let t0 = Instant::now();
        let r = git(&d, &["slow"], Duration::from_millis(400));
        assert!(r.is_err() && r.unwrap_err().contains("timed out"));
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn git_never_prompts_and_fails_fast_on_an_unreachable_host() {
        let d = tmp("noprompt");
        g(&d, &["init", "-q", "-b", "main"]);
        fs::write(d.join("board.json"), "{}").unwrap();
        g(
            &d,
            &["remote", "add", "origin", "ssh://git@127.0.0.1:9/nope.git"],
        );
        let t = fast();
        commit_board(&d, &t).unwrap();
        let t0 = Instant::now();
        let r = push(&d, &t);
        assert!(
            r.is_err(),
            "unreachable remote is an error, not a hang or a prompt"
        );
        assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
    }

    #[test]
    fn init_sync_sets_up_a_repo_pushes_and_ignores_scratch_files() {
        let (bare, data) = rig("init");
        let said = init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        assert!(
            said.iter().any(|s| s.starts_with("git init"))
                && said.iter().any(|s| s.starts_with("pushed")),
            "{said:?}"
        );
        assert!(is_repo(&data));
        let ignore = fs::read_to_string(data.join(".gitignore")).unwrap();
        for pat in ["*.bak", "*.bad", "*.tmp"] {
            assert!(ignore.lines().any(|l| l == pat), "{pat} in {ignore}");
        }
        assert_eq!(remote_log(&bare), ["wallboard: initial backup"]);
        let remote_board = remote_file(&bare, "board.json");
        assert!(remote_board.contains("\"version\": 2") && remote_board.contains("Inbox"));
        assert!(remote_file(&bare, ".gitignore").contains("*.bak"));
        // scratch files are never tracked
        fs::write(data.join("board.json.bak"), "x").unwrap();
        fs::write(data.join("board.json.bad"), "x").unwrap();
        fs::write(data.join("board.json.tmp"), "x").unwrap();
        assert_eq!(
            g(&data, &["status", "--porcelain"]),
            "",
            "ignored files do not show up"
        );
    }

    #[test]
    fn init_sync_uses_an_existing_board_and_is_repeatable() {
        let (bare, data) = rig("again");
        let mut b = Board::default_board();
        b.areas[0].todos.push(crate::board::Todo::new("keep me"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        assert!(
            remote_file(&bare, "board.json").contains("keep me"),
            "the existing board is what gets backed up"
        );
        // running it again (e.g. after changing the URL) is harmless and adds no duplicate commit
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        assert_eq!(remote_log(&bare).len(), 1);
    }

    #[test]
    fn init_sync_reports_a_bad_remote_but_keeps_the_local_repo() {
        let (_bare, data) = rig("badremote");
        let err = init_sync(&data, "/definitely/not/a/repo.git", &fast()).unwrap_err();
        assert!(
            err.contains("push") && err.contains("set up locally"),
            "{err}"
        );
        assert!(
            is_repo(&data) && !g(&data, &["log", "--format=%s"]).is_empty(),
            "local commit exists for a later retry"
        );
        assert!(init_sync(&data, "--upload-pack=evil", &fast()).is_err());
    }

    #[test]
    fn sync_once_commits_changes_pushes_and_skips_when_unchanged() {
        let (bare, data) = rig("once");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        let t = fast();
        assert_eq!(
            sync_once(&data, &t, true).unwrap(),
            Outcome {
                committed: false,
                pushed: true
            },
            "no change: no commit"
        );
        assert_eq!(remote_log(&bare).len(), 1);
        let mut b = Board::default_board();
        b.areas[1]
            .todos
            .push(crate::board::Todo::new("ship wallboard"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        assert_eq!(
            sync_once(&data, &t, true).unwrap(),
            Outcome {
                committed: true,
                pushed: true
            }
        );
        assert_eq!(remote_log(&bare).len(), 2);
        assert!(remote_file(&bare, "board.json").contains("ship wallboard"));
        assert!(remote_log(&bare)[0].starts_with("wallboard backup"));
    }

    #[test]
    fn push_only_mode_never_commits() {
        let (bare, data) = rig("pushonly");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        let mut b = Board::default_board();
        b.areas[2]
            .todos
            .push(crate::board::Todo::new("uncommitted edit"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        let o = sync_once(&data, &fast(), false).unwrap();
        assert!(!o.committed);
        assert_eq!(
            remote_log(&bare).len(),
            1,
            "a startup push must not commit new work"
        );
        assert!(
            g(&data, &["status", "--porcelain"]).contains("board.json"),
            "the edit is still just a working-tree change"
        );
    }

    #[test]
    fn a_local_only_repo_just_commits() {
        let d = tmp("localonly");
        g(&d, &["init", "-q", "-b", "main"]);
        store::save(
            &d.join("board.json"),
            &Board::default_board(),
            &store::View::default(),
        )
        .unwrap();
        let o = sync_once(&d, &fast(), true).unwrap();
        assert_eq!(
            o,
            Outcome {
                committed: true,
                pushed: false
            }
        );
        assert_eq!(g(&d, &["log", "--format=%s"]).lines().count(), 1);
    }

    #[test]
    fn commits_work_without_a_configured_git_identity() {
        let d = tmp("identity");
        g(&d, &["init", "-q", "-b", "main"]);
        // A repo-local blank identity cannot hide a global one, so just check the fallback logic directly.
        let t = fast();
        let args = identity_args(&d, &t);
        let has_global = git(&d, &["config", "--get", "user.email"], t.local_timeout)
            .is_ok_and(|(c, o, _)| c == 0 && !o.is_empty());
        assert_eq!(
            args.is_empty(),
            has_global,
            "fallback identity only when none is configured: {args:?}"
        );
        store::save(
            &d.join("board.json"),
            &Board::default_board(),
            &store::View::default(),
        )
        .unwrap();
        assert!(commit_board(&d, &t).unwrap());
    }

    #[test]
    fn background_thread_debounces_and_commits_once_per_burst() {
        let (bare, data) = rig("thread");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        let backup = Backup::start(data.clone(), fast(), false).expect("data dir is a repo");
        let mut b = Board::default_board();
        for i in 0..5 {
            b.areas[2]
                .todos
                .push(crate::board::Todo::new(&format!("burst {i}")));
            store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
            backup.notify();
            thread::sleep(Duration::from_millis(60));
        }
        thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            remote_log(&bare).len(),
            2,
            "five quick changes became one commit: {:?}",
            remote_log(&bare)
        );
        assert!(remote_file(&bare, "board.json").contains("burst 4"));
        drop(backup);
    }

    #[test]
    fn background_thread_respects_the_minimum_gap_between_attempts() {
        let (bare, data) = rig("gap");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        let backup = Backup::start(data.clone(), fast(), false).unwrap();
        let mut b = Board::default_board();
        b.areas[2].todos.push(crate::board::Todo::new("first"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        backup.notify();
        thread::sleep(Duration::from_millis(600)); // first commit fires at ~250ms
        assert_eq!(remote_log(&bare).len(), 2);
        let t_first = Instant::now();
        b.areas[2].todos.push(crate::board::Todo::new("second"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        backup.notify();
        thread::sleep(Duration::from_millis(400)); // debounce passed, but min_gap (900ms) has not
        assert_eq!(remote_log(&bare).len(), 2, "held back by the minimum gap");
        thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            remote_log(&bare).len(),
            3,
            "sent once the gap elapsed ({:?} later)",
            t_first.elapsed()
        );
        drop(backup);
    }

    #[test]
    fn background_thread_retries_after_a_failed_push() {
        let (bare, data) = rig("retry");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        // Break the remote, make a change, let the first attempt fail, then repair the remote.
        let moved = bare.with_extension("moved");
        fs::rename(&bare, &moved).unwrap();
        let backup = Backup::start(data.clone(), fast(), false).unwrap();
        let mut b = Board::default_board();
        b.areas[2]
            .todos
            .push(crate::board::Todo::new("offline edit"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        backup.notify();
        thread::sleep(Duration::from_millis(900));
        assert!(
            g(&data, &["log", "--format=%s"]).lines().count() >= 2,
            "the commit happened locally even though the push failed"
        );
        fs::rename(&moved, &bare).unwrap();
        thread::sleep(Duration::from_millis(1800));
        assert!(
            remote_file(&bare, "board.json").contains("offline edit"),
            "pushed on retry once the remote is back"
        );
        drop(backup);
    }

    #[test]
    fn dropping_the_backup_commits_pending_work_locally_without_waiting_for_the_network() {
        let (bare, data) = rig("drop");
        init_sync(&data, bare.to_str().unwrap(), &fast()).unwrap();
        let slow = Timing {
            debounce: Duration::from_secs(3600),
            ..fast()
        };
        let backup = Backup::start(data.clone(), slow, false).unwrap();
        let mut b = Board::default_board();
        b.areas[2]
            .todos
            .push(crate::board::Todo::new("last second edit"));
        store::save(&data.join("board.json"), &b, &store::View::default()).unwrap();
        backup.notify();
        thread::sleep(Duration::from_millis(100));
        let t0 = Instant::now();
        drop(backup);
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert!(
            g(&data, &["log", "-1", "--format=%s"]).starts_with("wallboard backup"),
            "committed at exit"
        );
        assert_eq!(
            remote_log(&bare).len(),
            1,
            "but not pushed (exit never waits on the network)"
        );
    }

    #[test]
    fn not_a_repo_means_no_backup_thread() {
        let d = tmp("norepo");
        assert!(!is_repo(&d));
        assert!(Backup::start(d, fast(), false).is_none());
    }

    #[test]
    fn default_cadence_is_every_ten_minutes() {
        let t = Timing::default();
        assert_eq!(t.min_gap, Duration::from_secs(600));
        assert_eq!(t.debounce, Duration::from_secs(30));
        assert_eq!(BackupSettings::default().interval_secs, 600);
    }

    #[test]
    fn settings_map_to_timing_and_are_clamped() {
        let t = Timing::from_settings(BackupSettings {
            interval_secs: 120,
            debounce_secs: 10,
        });
        assert_eq!((t.min_gap.as_secs(), t.debounce.as_secs()), (120, 10));
        assert!(t.retry < t.min_gap);
        let weird = BackupSettings {
            interval_secs: 0,
            debounce_secs: 99999,
        }
        .sane();
        assert_eq!(
            (weird.interval_secs, weird.debounce_secs),
            (5, 5),
            "debounce never exceeds the interval"
        );
        let huge = BackupSettings {
            interval_secs: u64::MAX,
            debounce_secs: 0,
        }
        .sane();
        assert_eq!((huge.interval_secs, huge.debounce_secs), (86_400, 1));
        let s: BackupSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, BackupSettings::default());
        let s: BackupSettings = serde_json::from_str(r#"{"interval_secs":60}"#).unwrap();
        assert_eq!((s.interval_secs, s.debounce_secs), (60, 30));
    }
}

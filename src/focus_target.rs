//! Which window dictation goes to: the one with keyboard focus (the default)
//! or the one under the mouse pointer.
//!
//! Typed text (wtype, paste) and the fcitx5 ghost preedit always land in the
//! keyboard-focused window. With Hyprland's `input:follow_mouse = 3` the
//! keyboard focus stays put while the mouse points elsewhere, so `mouse` lets
//! you dictate into the window you point at: at the start of an utterance
//! ears focuses the window under the pointer ([`FocusBorrow::begin`]), and
//! after the utterance's final delivery it gives focus back to the window
//! that had it ([`FocusBorrow::restore`]), unless focus moved meanwhile or
//! `restore_focus = false`.
//!
//! The setting is `focus_target = "keyboard" | "mouse"` in the config, and a
//! persisted runtime switch (`ears focus-target keyboard|mouse|toggle|status`
//! and the `focus-target-*` command socket verbs) that wins when present. It
//! lives in `$XDG_STATE_HOME/ears/focus-target` (override with
//! `EARS_FOCUS_TARGET_STATE`) and is read at each utterance, so a switch
//! takes effect at the next one.
//!
//! Hyprland only. Elsewhere `mouse` logs once and behaves as `keyboard`.
//!
//! The pointer never moves: Hyprland's `focuswindow` warps the cursor to the
//! window's middle unless `cursor:no_warps` is set, so when it is not, the
//! same `hyprctl --batch` request moves the cursor straight back to where it
//! was. One request is handled whole by the compositor, with no frame drawn
//! in between, and no global keyword is touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where dictation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FocusTarget {
    /// The keyboard-focused window (ears' behaviour before this setting).
    #[default]
    Keyboard,
    /// The window under the mouse pointer, focused for the utterance.
    Mouse,
}

impl FocusTarget {
    pub fn name(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "keyboard" => Some(Self::Keyboard),
            "mouse" => Some(Self::Mouse),
            _ => None,
        }
    }
}

/// What the engine and push-to-talk need: the configured target (the state
/// file overrides it) and whether to give focus back after the utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusSettings {
    pub target: FocusTarget,
    pub restore: bool,
}

impl FocusSettings {
    pub fn from_config(config: &crate::Config) -> Self {
        Self {
            target: config.focus_target,
            restore: config.restore_focus,
        }
    }
}

/// A request carried by the `focus-target-*` command socket verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTargetRequest {
    Keyboard,
    Mouse,
    Toggle,
    Status,
}

impl FocusTargetRequest {
    /// Parse a command socket verb (`focus-target-keyboard`, `-mouse`,
    /// `-toggle`, `-status`).
    pub fn from_command(verb: &str) -> Option<Self> {
        match verb {
            "focus-target-keyboard" => Some(Self::Keyboard),
            "focus-target-mouse" => Some(Self::Mouse),
            "focus-target-toggle" => Some(Self::Toggle),
            "focus-target-status" => Some(Self::Status),
            _ => None,
        }
    }

    /// Parse the CLI argument (`keyboard`, `mouse`, `toggle`, `status`).
    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::from_command(&format!("focus-target-{}", arg))
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::Keyboard => "focus-target-keyboard",
            Self::Mouse => "focus-target-mouse",
            Self::Toggle => "focus-target-toggle",
            Self::Status => "focus-target-status",
        }
    }

    /// The resulting target, given the current one.
    pub fn apply(self, current: FocusTarget) -> FocusTarget {
        match self {
            Self::Keyboard => FocusTarget::Keyboard,
            Self::Mouse => FocusTarget::Mouse,
            Self::Toggle => match current {
                FocusTarget::Keyboard => FocusTarget::Mouse,
                FocusTarget::Mouse => FocusTarget::Keyboard,
            },
            Self::Status => current,
        }
    }

    pub fn changes_state(self) -> bool {
        self != Self::Status
    }
}

/// The wire/CLI form of a target: `focus-target:keyboard` or `focus-target:mouse`.
pub fn describe(target: FocusTarget) -> String {
    format!("focus-target:{}", target.name())
}

/// Where the switch is persisted.
pub fn state_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("EARS_FOCUS_TARGET_STATE") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    directories::ProjectDirs::from("com", "heiervang", "ears")
        .and_then(|p| p.state_dir().map(|d| d.join("focus-target")))
}

/// The persisted switch, if there is a valid one.
pub fn load() -> Option<FocusTarget> {
    let path = state_path()?;
    match std::fs::read_to_string(&path) {
        Ok(value) => {
            let target = FocusTarget::parse(&value);
            if target.is_none() {
                tracing::warn!(path = %path.display(), "Invalid focus-target state; using the config");
            }
            target
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "Cannot read focus-target state; using the config");
            None
        }
    }
}

/// Persist the switch atomically.
pub fn save(target: FocusTarget) -> Result<()> {
    let path = state_path().context("No state directory for the focus-target switch")?;
    let dir = path.parent().context("Focus-target path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let tmp = dir.join(format!(".focus-target.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{}\n", target.name()))
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("Failed to replace {}: {}", path.display(), e)
    })
}

/// The target in force: the persisted switch when there is one, else the config.
pub fn effective(configured: FocusTarget) -> FocusTarget {
    load().unwrap_or(configured)
}

/// Carry out a request against the switch; the reply for the socket or CLI.
pub fn handle(request: FocusTargetRequest, configured: FocusTarget) -> Result<String> {
    let next = request.apply(effective(configured));
    if request.changes_state() {
        save(next)?;
        tracing::info!("Focus target switched to {}", next.name());
    }
    Ok(describe(next))
}

/// [`handle`] for a command socket: an error becomes an `error:` reply.
pub fn respond(request: FocusTargetRequest, configured: FocusTarget) -> String {
    handle(request, configured).unwrap_or_else(|e| {
        tracing::warn!("Failed to persist focus-target switch: {}", e);
        format!("error:focus-target: {e}")
    })
}

/// Deadline for one `hyprctl` request.
const HYPRCTL_TIMEOUT: Duration = Duration::from_millis(500);

/// Everything one decision needs, read in a single `hyprctl --batch`
/// request so the answers agree with each other.
const SNAPSHOT_BATCH: &str = "j/cursorpos;j/activewindow;j/clients;j/monitors;\
                              j/getoption cursor:no_warps;j/getoption input:follow_mouse";

/// A mapped, shown window as `hyprctl clients -j` describes it.
#[derive(Debug, Clone, PartialEq)]
struct Window {
    address: String,
    workspace: i64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    floating: bool,
    pinned: bool,
    /// 0 none, 1 maximized, 2 fullscreen.
    fullscreen: i64,
    /// 0 is the most recently focused window.
    focus_history: i64,
}

impl Window {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Stacking tier: fullscreen over pinned over floating over maximized
    /// over tiled. Within a tier the most recently focused is on top
    /// (Hyprland raises a floating window when it is focused).
    fn stacking(&self) -> (u8, std::cmp::Reverse<i64>) {
        let tier = if self.fullscreen >= 2 {
            4
        } else if self.pinned {
            3
        } else if self.floating {
            2
        } else if self.fullscreen == 1 {
            1
        } else {
            0
        };
        (tier, std::cmp::Reverse(self.focus_history))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Monitor {
    id: i64,
    active_workspace: i64,
    /// 0 when no special workspace is open on it.
    special_workspace: i64,
    focused: bool,
}

impl Monitor {
    /// The workspace it shows on top: an open special workspace covers the
    /// regular one.
    fn shown(&self) -> i64 {
        if self.special_workspace != 0 {
            self.special_workspace
        } else {
            self.active_workspace
        }
    }
}

/// The compositor's state at one moment.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    cursor: (f64, f64),
    active: Option<String>,
    windows: Vec<Window>,
    monitors: Vec<Monitor>,
    no_warps: bool,
    follow_mouse: i64,
}

/// A Hyprland window address (`0x` and hex digits): safe to put in a batch.
fn valid_address(address: &str) -> bool {
    address.len() > 2
        && address.starts_with("0x")
        && address[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn pair(v: &Value) -> Option<(f64, f64)> {
    let a = v.as_array()?;
    Some((number(a.first()?)?, number(a.get(1)?)?))
}

fn parse_window(v: &Value) -> Option<Window> {
    if v.get("mapped").and_then(Value::as_bool) == Some(false)
        || v.get("hidden").and_then(Value::as_bool) == Some(true)
        || v.get("acceptsInput").and_then(Value::as_bool) == Some(false)
    {
        return None;
    }
    let address = v.get("address")?.as_str()?.to_string();
    if !valid_address(&address) {
        return None;
    }
    let (x, y) = pair(v.get("at")?)?;
    let (width, height) = pair(v.get("size")?)?;
    // Older Hyprland: `fullscreen` is a bool (with `fullscreenMode`).
    let fullscreen = match v.get("fullscreen") {
        Some(Value::Bool(true)) => 2,
        Some(f) => f.as_i64().unwrap_or(0),
        None => 0,
    };
    Some(Window {
        address,
        workspace: v.get("workspace")?.get("id")?.as_i64()?,
        x,
        y,
        width,
        height,
        floating: v.get("floating").and_then(Value::as_bool).unwrap_or(false),
        pinned: v.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        fullscreen,
        focus_history: v
            .get("focusHistoryID")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX),
    })
}

fn parse_monitor(v: &Value) -> Option<Monitor> {
    Some(Monitor {
        id: v.get("id")?.as_i64()?,
        active_workspace: v.get("activeWorkspace")?.get("id")?.as_i64()?,
        special_workspace: v
            .get("specialWorkspace")
            .and_then(|s| s.get("id"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        focused: v.get("focused").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Parse the replies to [`SNAPSHOT_BATCH`], concatenated as hyprctl prints them.
fn parse_snapshot(bytes: &[u8]) -> Option<Snapshot> {
    let values = serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [cursor, active, clients, monitors, no_warps, follow_mouse] =
        <[Value; 6]>::try_from(values).ok()?;
    let cursor = (number(cursor.get("x")?)?, number(cursor.get("y")?)?);
    // No focused window: `{}`.
    let active = active
        .get("address")
        .and_then(Value::as_str)
        .filter(|a| valid_address(a))
        .map(str::to_string);
    Some(Snapshot {
        cursor,
        active,
        windows: clients
            .as_array()?
            .iter()
            .filter_map(parse_window)
            .collect(),
        monitors: monitors
            .as_array()?
            .iter()
            .filter_map(parse_monitor)
            .collect(),
        no_warps: no_warps.get("int").and_then(Value::as_i64).unwrap_or(0) != 0,
        follow_mouse: follow_mouse.get("int").and_then(Value::as_i64).unwrap_or(1),
    })
}

impl Snapshot {
    /// The monitor showing workspace `id` on top, if any does.
    fn monitor_showing(&self, workspace: i64) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.shown() == workspace)
    }

    /// The topmost visible window under the pointer.
    fn under_cursor(&self) -> Option<&Window> {
        self.windows
            .iter()
            .filter(|w| self.monitor_showing(w.workspace).is_some() && w.contains(self.cursor))
            .max_by_key(|w| w.stacking())
    }

    fn window(&self, address: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.address == address)
    }

    /// Can `focuswindow` take `w` without changing what is on screen? It
    /// switches to the window's workspace when that is not shown, and opens
    /// a special workspace on the focused monitor, so only a window that is
    /// on top of its monitor, and for a special workspace on the focused
    /// monitor, qualifies.
    fn focusable(&self, w: &Window) -> Result<(), &'static str> {
        let monitor = self
            .monitor_showing(w.workspace)
            .ok_or("its workspace is not shown")?;
        if monitor.special_workspace != 0 && !monitor.focused {
            return Err("it is on a special workspace of another monitor");
        }
        Ok(())
    }

    /// One batch request: focus `address`, and when Hyprland warps the
    /// pointer on focus, put it straight back.
    fn focus_batch(&self, address: &str) -> String {
        let mut batch = format!("dispatch focuswindow address:{address}");
        if !self.no_warps {
            let (x, y) = self.cursor;
            batch.push_str(&format!(
                ";dispatch movecursor {} {}",
                x.round() as i64,
                y.round() as i64
            ));
        }
        batch
    }
}

/// What borrowing focus for an utterance takes.
#[derive(Debug, Clone, PartialEq)]
struct BorrowPlan {
    batch: String,
    previous: Option<String>,
    borrowed: String,
}

/// Focus the window under the pointer, if it is another one than the
/// focused one; else why not.
fn borrow_plan(snapshot: &Snapshot) -> Result<BorrowPlan, &'static str> {
    let target = snapshot
        .under_cursor()
        .ok_or("no window under the pointer")?;
    if snapshot.active.as_deref() == Some(target.address.as_str()) {
        return Err("the window under the pointer already has focus");
    }
    snapshot.focusable(target)?;
    Ok(BorrowPlan {
        batch: snapshot.focus_batch(&target.address),
        previous: snapshot.active.clone(),
        borrowed: target.address.clone(),
    })
}

/// Give focus back to `previous`, if the borrowed window still has it;
/// else why not.
fn restore_plan(
    snapshot: &Snapshot,
    previous: &str,
    borrowed: &str,
) -> Result<String, &'static str> {
    if snapshot.active.as_deref() != Some(borrowed) {
        return Err("focus moved during the utterance");
    }
    if snapshot.follow_mouse == 1 {
        // Hyprland would hand focus straight back to the window under the
        // pointer as the cursor is put back.
        return Err("input:follow_mouse = 1 keeps focus under the pointer");
    }
    let window = snapshot
        .window(previous)
        .ok_or("the previous window is gone or hidden")?;
    snapshot.focusable(window)?;
    Ok(snapshot.focus_batch(previous))
}

fn hyprctl(args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new("hyprctl");
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    match crate::desktop::output_bounded(cmd, HYPRCTL_TIMEOUT) {
        Ok(out) if out.status.success() => Some(out.stdout),
        Ok(out) => {
            tracing::debug!("hyprctl {:?} exited with {}", args, out.status);
            None
        }
        Err(e) => {
            tracing::debug!("hyprctl {:?} failed: {}", args, e);
            None
        }
    }
}

fn snapshot() -> Option<Snapshot> {
    let out = hyprctl(&["--batch", SNAPSHOT_BATCH])?;
    let snapshot = parse_snapshot(&out);
    if snapshot.is_none() {
        tracing::debug!("Unreadable hyprctl snapshot; focus left alone");
    }
    snapshot
}

/// Run a dispatch batch; true when every dispatch answered `ok`.
fn dispatch(batch: &str) -> bool {
    let Some(out) = hyprctl(&["--batch", batch]) else {
        return false;
    };
    let reply = String::from_utf8_lossy(&out);
    let ok = reply.split_whitespace().all(|r| r == "ok") && reply.contains("ok");
    if !ok {
        tracing::warn!("hyprctl --batch {:?}: {}", batch, reply.trim());
    }
    ok
}

static WARNED_COMPOSITOR: AtomicBool = AtomicBool::new(false);

/// Running under Hyprland? Elsewhere `mouse` behaves as `keyboard`, said once.
fn hyprland() -> bool {
    let hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && std::env::var_os("SWAYSOCK").is_none()
        && std::env::var_os("NIRI_SOCKET").is_none();
    if !hyprland && !WARNED_COMPOSITOR.swap(true, Ordering::Relaxed) {
        tracing::warn!("focus_target = \"mouse\" needs Hyprland; following keyboard focus instead");
    }
    hyprland
}

/// Focus lent to the window under the pointer for one utterance.
///
/// Dropping it gives focus back (when `restore` is on and the borrowed
/// window still has it), so every way out of an utterance restores, errors
/// included. [`FocusBorrow::persist`] hands it to another process instead
/// (push-to-talk starts and stops in two `ears toggle` runs).
#[derive(Debug, Serialize, Deserialize)]
pub struct FocusBorrow {
    /// The window that had focus; None when none did (nothing to give back).
    previous: Option<String>,
    borrowed: String,
    restore: bool,
    #[serde(skip)]
    armed: bool,
}

impl FocusBorrow {
    /// Focus the window under the pointer when the target in force is
    /// `mouse` and it is not already focused. None: focus was left alone.
    /// Blocks for at most two bounded `hyprctl` calls.
    pub fn begin(settings: FocusSettings) -> Option<Self> {
        if effective(settings.target) != FocusTarget::Mouse || !hyprland() {
            return None;
        }
        let snapshot = snapshot()?;
        let plan = match borrow_plan(&snapshot) {
            Ok(plan) => plan,
            Err(why) => {
                tracing::debug!("Focus target mouse: focus left alone ({})", why);
                return None;
            }
        };
        if !dispatch(&plan.batch) {
            return None;
        }
        tracing::info!(
            "Focus target mouse: focused {} for the utterance (was {})",
            plan.borrowed,
            plan.previous.as_deref().unwrap_or("none")
        );
        Some(Self {
            previous: plan.previous,
            borrowed: plan.borrowed,
            restore: settings.restore,
            armed: true,
        })
    }

    /// Give focus back now (what dropping does).
    pub fn restore(mut self) {
        self.restore_now();
    }

    fn restore_now(&mut self) {
        if !std::mem::take(&mut self.armed) || !self.restore {
            return;
        }
        let Some(previous) = self.previous.as_deref() else {
            return;
        };
        let Some(snapshot) = snapshot() else {
            return;
        };
        match restore_plan(&snapshot, previous, &self.borrowed) {
            Ok(batch) => {
                if dispatch(&batch) {
                    tracing::info!("Focus target mouse: focus back to {}", previous);
                }
            }
            Err(why) => tracing::info!("Focus target mouse: focus not given back ({})", why),
        }
    }

    /// Hand the borrow to a later process ([`FocusBorrow::resume`]) instead
    /// of restoring now. On failure, focus is given back at once.
    pub fn persist(mut self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec(&self)?;
        std::fs::write(path, json)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        self.armed = false;
        Ok(())
    }

    /// Take over a borrow persisted by an earlier process; the file is
    /// removed either way.
    pub fn resume(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let _ = std::fs::remove_file(path);
        let mut borrow: Self = serde_json::from_slice(&bytes).ok()?;
        if !valid_address(&borrow.borrowed)
            || borrow
                .previous
                .as_deref()
                .is_some_and(|p| !valid_address(p))
        {
            return None;
        }
        borrow.armed = true;
        Some(borrow)
    }

    /// Forget a persisted borrow left by a run that never finished.
    pub fn discard(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

impl Drop for FocusBorrow {
    fn drop(&mut self) {
        self.restore_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three monitors like a real desk: a 4K main (logical 2400x1350 at
    /// 0,1440) showing workspace 5, a left one showing 2, and a top one with
    /// special workspace -98 open over workspace 3.
    const MONITORS: &str = r#"[
        {"id": 0, "name": "HDMI-A-1", "x": 0, "y": 1440, "width": 3840, "height": 2160, "scale": 1.6,
         "activeWorkspace": {"id": 5, "name": "5"}, "specialWorkspace": {"id": 0, "name": ""}, "focused": true},
        {"id": 1, "name": "DP-1", "x": -1200, "y": 0, "width": 1920, "height": 1200, "scale": 1.0,
         "activeWorkspace": {"id": 2, "name": "2"}, "specialWorkspace": {"id": 0, "name": ""}, "focused": false},
        {"id": 2, "name": "DP-2", "x": 158, "y": 288, "width": 2560, "height": 1440, "scale": 1.25,
         "activeWorkspace": {"id": 3, "name": "3"}, "specialWorkspace": {"id": -98, "name": "special:magic"}, "focused": false}
    ]"#;

    fn client(address: &str, ws: i64, at: (i64, i64), size: (i64, i64), extra: &str) -> String {
        format!(
            r#"{{"address": "{address}", "mapped": true, "hidden": false, "acceptsInput": true,
                "at": [{}, {}], "size": [{}, {}], "workspace": {{"id": {ws}, "name": "{ws}"}},
                "floating": false, "pinned": false, "fullscreen": 0, "focusHistoryID": 9,
                "class": "Alacritty" {extra}}}"#,
            at.0, at.1, size.0, size.1
        )
    }

    /// The replies to [`SNAPSHOT_BATCH`], as hyprctl prints them.
    fn batch_output(cursor: (i64, i64), active: &str, clients: &[String], no_warps: i64) -> String {
        let active = if active.is_empty() {
            "{}".to_string()
        } else {
            format!(r#"{{"address": "{active}", "class": "x"}}"#)
        };
        format!(
            "{{\n    \"x\": {},\n    \"y\": {}\n}}\n\n\n{active}\n\n\n[{}]\n\n\n{MONITORS}\n\n\n\
             {{\"option\": \"cursor:no_warps\", \"int\": {no_warps}, \"set\": false }}\n\n\n\
             {{\"option\": \"input:follow_mouse\", \"int\": 3, \"set\": true }}\n",
            cursor.0,
            cursor.1,
            clients.join(",")
        )
    }

    fn desk() -> Vec<String> {
        vec![
            // Workspace 5 on the main monitor: two tiled halves.
            client(
                "0xa1",
                5,
                (14, 1480),
                (1179, 1270),
                r#", "focusHistoryID": 1"#,
            ),
            client(
                "0xa2",
                5,
                (1207, 1480),
                (1179, 1270),
                r#", "focusHistoryID": 0"#,
            ),
            // A floating window over the left half, and a pinned one over that.
            client(
                "0xf1",
                5,
                (300, 1600),
                (500, 400),
                r#", "floating": true, "focusHistoryID": 3"#,
            ),
            client(
                "0xf2",
                5,
                (400, 1700),
                (200, 200),
                r#", "floating": true, "pinned": true, "focusHistoryID": 4"#,
            ),
            // Workspace 1 is not shown anywhere.
            client("0xb1", 1, (14, 1480), (2372, 1270), ""),
            // A grouped (hidden) tab and an unmapped window.
            client("0xb2", 5, (14, 1480), (1179, 1270), r#", "hidden": true"#),
            client("0xb3", 5, (14, 1480), (1179, 1270), r#", "mapped": false"#),
            // Left monitor, workspace 2.
            client("0xc1", 2, (-1186, 40), (1172, 913), ""),
            // Top monitor: workspace 3 under special workspace -98.
            client("0xd1", 3, (170, 300), (2000, 1100), ""),
            client("0xd2", -98, (500, 500), (800, 600), ""),
        ]
    }

    fn snap(cursor: (i64, i64), active: &str) -> Snapshot {
        parse_snapshot(batch_output(cursor, active, &desk(), 0).as_bytes()).expect("fixture parses")
    }

    fn picked(cursor: (i64, i64)) -> Option<String> {
        snap(cursor, "0xa2")
            .under_cursor()
            .map(|w| w.address.clone())
    }

    #[test]
    fn parses_the_batch_reply() {
        let s = snap((755, 2233), "0xa2");
        assert_eq!(s.cursor, (755.0, 2233.0));
        assert_eq!(s.active.as_deref(), Some("0xa2"));
        assert_eq!(s.monitors.len(), 3);
        assert!(!s.no_warps);
        assert_eq!(s.follow_mouse, 3);
        // Hidden and unmapped windows are dropped at parse time.
        assert!(s.window("0xb2").is_none() && s.window("0xb3").is_none());
        assert!(s.window("0xb1").is_some());
        let none = parse_snapshot(batch_output((0, 0), "", &desk(), 1).as_bytes()).unwrap();
        assert_eq!(none.active, None);
        assert!(none.no_warps);
        assert!(parse_snapshot(b"unknown request").is_none());
        assert!(parse_snapshot(b"{\"x\": 1, \"y\": 2}").is_none());
    }

    #[test]
    fn picks_the_window_under_the_pointer() {
        assert_eq!(picked((100, 1500)).as_deref(), Some("0xa1"), "tiled left");
        assert_eq!(picked((2000, 2000)).as_deref(), Some("0xa2"), "tiled right");
        assert_eq!(
            picked((-600, 500)).as_deref(),
            Some("0xc1"),
            "other monitor"
        );
        // The gap between the tiles, and a point on no window.
        assert_eq!(picked((1200, 2000)), None);
        assert_eq!(picked((2390, 3500)), None);
    }

    #[test]
    fn floating_and_pinned_cover_tiled() {
        assert_eq!(picked((320, 1620)).as_deref(), Some("0xf1"));
        assert_eq!(picked((450, 1750)).as_deref(), Some("0xf2"));
    }

    #[test]
    fn most_recently_focused_floating_is_on_top() {
        let mut clients = desk();
        clients.push(client(
            "0xf3",
            5,
            (310, 1610),
            (100, 100),
            r#", "floating": true, "focusHistoryID": 2"#,
        ));
        let s = parse_snapshot(batch_output((320, 1620), "0xa2", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor().unwrap().address, "0xf3");
    }

    #[test]
    fn fullscreen_wins() {
        let mut clients = desk();
        clients.push(client(
            "0xee",
            5,
            (0, 1440),
            (2400, 1350),
            r#", "fullscreen": 2, "focusHistoryID": 8"#,
        ));
        let s = parse_snapshot(batch_output((450, 1750), "0xa2", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor().unwrap().address, "0xee");
        // A maximized window sits under floating ones, over tiled ones.
        let mut clients = desk();
        clients.push(client(
            "0xab",
            5,
            (14, 1480),
            (2372, 1270),
            r#", "fullscreen": 1, "focusHistoryID": 8"#,
        ));
        let s = parse_snapshot(batch_output((320, 1620), "0xa2", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor().unwrap().address, "0xf1");
        let s = parse_snapshot(batch_output((100, 1500), "0xa2", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor().unwrap().address, "0xab");
        // Older Hyprland's boolean fullscreen.
        let mut clients = desk();
        clients.push(client(
            "0xef",
            5,
            (0, 1440),
            (2400, 1350),
            r#", "fullscreen": true"#,
        ));
        let s = parse_snapshot(batch_output((450, 1750), "0xa2", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor().unwrap().address, "0xef");
    }

    #[test]
    fn an_open_special_workspace_covers_its_monitor() {
        // Over the special window: it. Beside it, on the dimmed workspace
        // 3 underneath: nothing.
        assert_eq!(picked((600, 600)).as_deref(), Some("0xd2"));
        assert_eq!(picked((200, 1300)), None);
    }

    #[test]
    fn hidden_and_unshown_windows_are_never_picked() {
        // Only 0xb1 (workspace 1, not shown) covers this point besides the
        // tiles: the tile wins because it is shown.
        let mut clients = vec![client("0xb1", 1, (14, 1480), (2372, 1270), "")];
        clients.push(client(
            "0xb2",
            5,
            (14, 1480),
            (1179, 1270),
            r#", "hidden": true"#,
        ));
        let s = parse_snapshot(batch_output((100, 1500), "", &clients, 0).as_bytes()).unwrap();
        assert_eq!(s.under_cursor(), None);
    }

    #[test]
    fn borrow_focuses_the_pointed_window_and_puts_the_pointer_back() {
        let plan = borrow_plan(&snap((100, 1500), "0xa2")).unwrap();
        assert_eq!(plan.borrowed, "0xa1");
        assert_eq!(plan.previous.as_deref(), Some("0xa2"));
        assert_eq!(
            plan.batch,
            "dispatch focuswindow address:0xa1;dispatch movecursor 100 1500"
        );
        // cursor:no_warps set: focuswindow leaves the pointer alone.
        let s = parse_snapshot(batch_output((100, 1500), "0xa2", &desk(), 1).as_bytes()).unwrap();
        assert_eq!(
            borrow_plan(&s).unwrap().batch,
            "dispatch focuswindow address:0xa1"
        );
        // Nothing focused: borrow, with nothing to give back.
        let plan = borrow_plan(&snap((100, 1500), "")).unwrap();
        assert_eq!(plan.previous, None);
    }

    #[test]
    fn borrow_leaves_focus_alone_when_it_has_nothing_to_do() {
        assert!(
            borrow_plan(&snap((2000, 2000), "0xa2")).is_err(),
            "already focused"
        );
        assert!(
            borrow_plan(&snap((1200, 2000), "0xa2")).is_err(),
            "no window"
        );
        // A special workspace open on another monitor: focuswindow would
        // move it to the focused monitor.
        assert!(borrow_plan(&snap((600, 600), "0xa2")).is_err());
    }

    #[test]
    fn restore_only_while_the_borrowed_window_has_focus() {
        // The pointer has moved on since; it is put back where it is now.
        let s = snap((2000, 2000), "0xa1");
        assert_eq!(
            restore_plan(&s, "0xa2", "0xa1").unwrap(),
            "dispatch focuswindow address:0xa2;dispatch movecursor 2000 2000"
        );
        // The user focused something else meanwhile.
        assert!(restore_plan(&snap((2000, 2000), "0xc1"), "0xa2", "0xa1").is_err());
        // The previous window closed, or is on a workspace no longer shown.
        assert!(restore_plan(&s, "0x99", "0xa1").is_err());
        assert!(restore_plan(&s, "0xb1", "0xa1").is_err());
        // follow_mouse = 1 would hand focus straight back.
        let mut s = s;
        s.follow_mouse = 1;
        assert!(restore_plan(&s, "0xa2", "0xa1").is_err());
    }

    #[test]
    fn addresses_are_checked_before_they_reach_a_batch() {
        assert!(valid_address("0x55c311b18f80"));
        assert!(!valid_address("0x"));
        assert!(!valid_address("0x1;dispatch exec rm"));
        assert!(!valid_address("55c311b18f80"));
        let clients = vec![client(
            "0x1;dispatch exec x",
            5,
            (14, 1480),
            (1179, 1270),
            "",
        )];
        let s = parse_snapshot(batch_output((100, 1500), "", &clients, 0).as_bytes()).unwrap();
        assert!(s.windows.is_empty());
    }

    #[test]
    fn persisted_borrow_round_trips_without_restoring() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("focus-borrow.json");
        let borrow = FocusBorrow {
            previous: Some("0xa2".into()),
            borrowed: "0xa1".into(),
            restore: false,
            armed: true,
        };
        borrow.persist(&path).unwrap();
        let resumed = FocusBorrow::resume(&path).unwrap();
        assert!(!path.exists(), "resume consumes the file");
        assert_eq!(resumed.previous.as_deref(), Some("0xa2"));
        assert_eq!(resumed.borrowed, "0xa1");
        assert!(resumed.armed);
        // restore = false: dropping it never asks the compositor.
        drop(resumed);
        assert!(FocusBorrow::resume(&path).is_none());
        std::fs::write(
            &path,
            br#"{"previous":"0x1;x","borrowed":"0xa1","restore":true}"#,
        )
        .unwrap();
        assert!(
            FocusBorrow::resume(&path).is_none(),
            "a tampered address is refused"
        );
        std::fs::write(&path, b"{").unwrap();
        FocusBorrow::discard(&path);
        assert!(!path.exists());
    }

    #[test]
    fn parses_verbs_and_args() {
        assert_eq!(
            FocusTargetRequest::from_command("focus-target-mouse"),
            Some(FocusTargetRequest::Mouse)
        );
        assert_eq!(
            FocusTargetRequest::from_arg("toggle"),
            Some(FocusTargetRequest::Toggle)
        );
        assert_eq!(FocusTargetRequest::from_arg("sideways"), None);
        assert_eq!(FocusTargetRequest::from_command("typing-on"), None);
        for r in [
            FocusTargetRequest::Keyboard,
            FocusTargetRequest::Mouse,
            FocusTargetRequest::Toggle,
            FocusTargetRequest::Status,
        ] {
            assert_eq!(FocusTargetRequest::from_command(r.command()), Some(r));
        }
        use FocusTarget::*;
        assert_eq!(FocusTargetRequest::Toggle.apply(Keyboard), Mouse);
        assert_eq!(FocusTargetRequest::Toggle.apply(Mouse), Keyboard);
        assert_eq!(FocusTargetRequest::Status.apply(Mouse), Mouse);
        assert_eq!(FocusTargetRequest::Keyboard.apply(Mouse), Keyboard);
        assert!(!FocusTargetRequest::Status.changes_state());
        assert_eq!(describe(Mouse), "focus-target:mouse");
    }

    #[test]
    fn state_file_wins_over_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("focus-target");
        // SAFETY: this is the only test touching EARS_FOCUS_TARGET_STATE.
        std::env::set_var("EARS_FOCUS_TARGET_STATE", &path);
        assert_eq!(load(), None, "missing file: the config decides");
        assert_eq!(effective(FocusTarget::Mouse), FocusTarget::Mouse);
        assert_eq!(
            handle(FocusTargetRequest::Status, FocusTarget::Keyboard).unwrap(),
            "focus-target:keyboard"
        );
        assert_eq!(load(), None, "status writes nothing");
        assert_eq!(
            handle(FocusTargetRequest::Toggle, FocusTarget::Keyboard).unwrap(),
            "focus-target:mouse"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mouse\n");
        assert_eq!(effective(FocusTarget::Keyboard), FocusTarget::Mouse);
        save(FocusTarget::Keyboard).unwrap();
        assert_eq!(effective(FocusTarget::Mouse), FocusTarget::Keyboard);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(effective(FocusTarget::Mouse), FocusTarget::Mouse);
        std::env::remove_var("EARS_FOCUS_TARGET_STATE");
    }
}

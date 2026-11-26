//! Ghost completion: show in-progress speech as inline preedit text.
//!
//! Ghost mode does not type anything while you speak. Partial transcripts
//! are sent to the `earsghost` fcitx5 addon (see `fcitx5-addon/`), which
//! shows them as *preedit* in whichever application has input focus: the
//! app draws the text at its cursor, usually underlined, without it being
//! part of the document or reaching the shell. When the utterance ends the
//! final transcript is *committed*, which is when the application actually
//! receives it.
//!
//! Wire protocol (one line per command, one reply line per command):
//!
//! ```text
//! P <text>   show <text> as the ghost (replaces the previous one)
//! F <bytes> <text>   show with a frozen live prefix and mutable tail
//! B <bytes> <text>   show in the input-method panel (tmux overflow)
//! T          read snapshot: "OK state <bytes> <escaped text>"
//! C <text>   clear the ghost and commit <text>
//! X          clear the ghost
//! S          status
//! reply:     "OK preedit" | "OK panel" | "OK none" | "ERR <reason>"
//! ```
//!
//! Text escaping: backslash becomes `\\`, newline becomes `\n`.
//!
//! The addon clears any visible ghost when the connection closes, so a
//! crashed or stopped ears never leaves stale text behind.

use crate::ghost_terminal::Overflow;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Per-command I/O deadline. The addon answers from fcitx5's event loop in
/// well under a millisecond; anything slower means fcitx5 is wedged.
const IO_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Error)]
pub enum GhostError {
    #[error("ghost addon not reachable at {path}: {source}")]
    Unavailable {
        path: String,
        source: std::io::Error,
    },
    #[error("ghost addon I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("ghost addon replied with an error: {0}")]
    Rejected(String),
}

/// Displayed ghost text and its live forced-prefix boundary. A snapshot
/// never commits text and makes no promise about an optional final correction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GhostSnapshot {
    pub scope: &'static str,
    pub text: String,
    pub frozen_bytes: usize,
}

impl GhostSnapshot {
    fn parse(line: &str) -> Result<Self, GhostError> {
        let (count, escaped) = line
            .strip_prefix("OK state ")
            .and_then(|s| s.split_once(' '))
            .ok_or_else(|| GhostError::Rejected(line.to_string()))?;
        let frozen_bytes: usize = count
            .parse()
            .map_err(|_| GhostError::Rejected("invalid freeze boundary".into()))?;
        let mut text = String::new();
        let mut chars = escaped.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => text.push('\n'),
                    Some(c) => text.push(c),
                    None => return Err(GhostError::Rejected("truncated escape".into())),
                }
            } else {
                text.push(c);
            }
        }
        if frozen_bytes > text.len() || !text.is_char_boundary(frozen_bytes) {
            return Err(GhostError::Rejected("invalid freeze boundary".into()));
        }
        Ok(Self {
            scope: "live_decode",
            text,
            frozen_bytes,
        })
    }
}

/// How the addon displayed or delivered the last command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostDisplay {
    /// Drawn inline by the focused application.
    Preedit,
    /// The application cannot draw preedit; fcitx5 shows it in its panel.
    Panel,
    /// No input context has focus; nothing was shown or delivered.
    None,
}

/// What happened to a final transcript handed to the addon.
///
/// The addon commits before it replies, so a lost reply does not mean the
/// text was not delivered. Only outcomes where the commit certainly did not
/// happen may be retried by typing; retrying an [`Delivery::Unknown`] could
/// put the text in twice, possibly into another window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The focused application received the text.
    Delivered,
    /// Nothing was committed; the caller should type the text instead.
    NotDelivered,
    /// The commit may or may not have happened. Do not retry.
    Unknown,
}

/// How long to wait for Hyprland IPC / `hyprctl` before treating focus as unknown.
const FOCUS_PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Hard cap on bytes read from Hyprland's direct IPC socket.
const HYPRLAND_IPC_CAP: usize = 1024 * 1024;

/// Safety TTL for cached Hyprland focus while `.socket2.sock` is connected.
const FOCUS_CACHE_TTL: Duration = Duration::from_secs(2);

/// Focused Hyprland window metadata from `j/activewindow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprlandWindow {
    pub class: String,
    pub address: String,
    pub pid: Option<u32>,
}

/// Locate a Hyprland IPC socket (`.socket.sock` or `.socket2.sock`).
///
/// Checks `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/<filename>` first,
/// falling back to `/tmp/hypr/$HYPRLAND_INSTANCE_SIGNATURE/<filename>`.
pub(crate) fn hyprland_socket_path(filename: &str) -> Option<PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    if sig.is_empty() {
        return None;
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !runtime.is_empty() {
            let path = PathBuf::from(runtime)
                .join("hypr")
                .join(&sig)
                .join(filename);
            if path.exists() {
                return Some(path);
            }
        }
    }
    let tmp = PathBuf::from("/tmp/hypr").join(&sig).join(filename);
    tmp.exists().then_some(tmp)
}

/// Send a single request (e.g. `"j/activewindow"` or `"j/devices"`) over
/// Hyprland's direct Unix request socket (`.socket.sock`).
pub(crate) fn hyprland_ipc_request(request: &str, timeout: Duration) -> Option<Vec<u8>> {
    let path = hyprland_socket_path(".socket.sock")?;
    hyprland_ipc_request_at(&path, request, timeout)
}

pub(crate) fn hyprland_ipc_request_at(
    path: &Path,
    request: &str,
    timeout: Duration,
) -> Option<Vec<u8>> {
    use std::io::Read;

    let start = Instant::now();
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.write_all(request.as_bytes()).ok()?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = timeout.checked_sub(start.elapsed())?;
        if remaining.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(remaining)).ok()?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n > HYPRLAND_IPC_CAP {
                    return None;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(buf)
}

/// Parse Hyprland `j/activewindow` JSON.
///
/// Returns `Ok(Some(window))` when a window is focused, `Ok(None)` for valid
/// empty JSON (`{}` on an empty workspace), and `Err(())` for invalid JSON.
fn parse_hyprland_window_json(json: &[u8]) -> Result<Option<HyprlandWindow>, ()> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|_| ())?;
    let Some(obj) = v.as_object() else {
        return Err(());
    };
    let class = obj.get("class").and_then(|c| c.as_str());
    let address = obj.get("address").and_then(|a| a.as_str());
    if class.is_none() && address.is_none() {
        return Ok(None);
    }
    let pid = obj
        .get("pid")
        .and_then(|p| p.as_u64())
        .and_then(|p| u32::try_from(p).ok());
    Ok(Some(HyprlandWindow {
        class: class.unwrap_or_default().to_string(),
        address: address.unwrap_or_default().to_string(),
        pid,
    }))
}

#[derive(Debug, Clone)]
struct CachedWindow {
    window: Option<HyprlandWindow>,
    fetched_at: Instant,
}

fn invalidates_focus_cache(event_line: &str) -> bool {
    let Some((event, _)) = event_line.split_once(">>") else {
        return false;
    };
    matches!(
        event,
        "activewindow"
            | "activewindowv2"
            | "closewindow"
            | "workspace"
            | "workspacev2"
            | "focusedmon"
            | "focusedmonv2"
            | "activespecial"
            | "activespecialv2"
    )
}

struct HyprlandFocusCache {
    socket_path: PathBuf,
    cached: Arc<RwLock<Option<CachedWindow>>>,
    connected: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    ttl: Duration,
}

impl Drop for HyprlandFocusCache {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl HyprlandFocusCache {
    fn new(socket_path: PathBuf, socket2_path: PathBuf, ttl: Duration) -> Self {
        let cached: Arc<RwLock<Option<CachedWindow>>> = Arc::new(RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let cached_bg = Arc::clone(&cached);
        let connected_bg = Arc::clone(&connected);
        let generation_bg = Arc::clone(&generation);
        let stop_bg = Arc::clone(&stop);

        let _ = std::thread::Builder::new()
            .name("hyprland-focus-cache".into())
            .spawn(move || {
                let mut retry_delay = Duration::from_millis(250);
                while !stop_bg.load(Ordering::Acquire) {
                    match UnixStream::connect(&socket2_path) {
                        Ok(stream) => {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                            retry_delay = Duration::from_millis(250);
                            generation_bg.fetch_add(1, Ordering::AcqRel);
                            if let Ok(mut guard) = cached_bg.write() {
                                *guard = None;
                            }
                            connected_bg.store(true, Ordering::Release);

                            let mut reader = BufReader::new(stream);
                            let mut line = String::new();
                            while !stop_bg.load(Ordering::Acquire) {
                                line.clear();
                                match reader.read_line(&mut line) {
                                    Ok(0) => break,
                                    Ok(_) => {
                                        if invalidates_focus_cache(line.trim_end()) {
                                            generation_bg.fetch_add(1, Ordering::AcqRel);
                                            if let Ok(mut guard) = cached_bg.write() {
                                                *guard = None;
                                            }
                                        }
                                    }
                                    Err(e)
                                        if matches!(
                                            e.kind(),
                                            std::io::ErrorKind::Interrupted
                                                | std::io::ErrorKind::WouldBlock
                                                | std::io::ErrorKind::TimedOut
                                        ) =>
                                    {
                                        continue;
                                    }
                                    Err(_) => break,
                                }
                            }

                            connected_bg.store(false, Ordering::Release);
                            generation_bg.fetch_add(1, Ordering::AcqRel);
                            if let Ok(mut guard) = cached_bg.write() {
                                *guard = None;
                            }
                        }
                        Err(_) => {
                            connected_bg.store(false, Ordering::Release);
                        }
                    }

                    if stop_bg.load(Ordering::Acquire) {
                        break;
                    }
                    std::thread::sleep(retry_delay);
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(2));
                }
            });

        Self {
            socket_path,
            cached,
            connected,
            generation,
            stop,
            ttl,
        }
    }

    /// Return `Some(window)` when `.socket.sock` IPC succeeded (or cache hit),
    /// or `None` if direct IPC failed (caller should fall back to `hyprctl`).
    fn query(&self) -> Option<Option<HyprlandWindow>> {
        if self.connected.load(Ordering::Acquire) {
            if let Ok(guard) = self.cached.read() {
                if let Some(entry) = guard.as_ref() {
                    if entry.fetched_at.elapsed() < self.ttl {
                        return Some(entry.window.clone());
                    }
                }
            }
        }

        let gen_before = self.generation.load(Ordering::Acquire);
        let bytes =
            hyprland_ipc_request_at(&self.socket_path, "j/activewindow", FOCUS_PROBE_TIMEOUT)?;
        let window = parse_hyprland_window_json(&bytes).ok()?;

        if let Ok(mut guard) = self.cached.write() {
            if self.connected.load(Ordering::Acquire)
                && self.generation.load(Ordering::Acquire) == gen_before
            {
                *guard = Some(CachedWindow {
                    window: window.clone(),
                    fetched_at: Instant::now(),
                });
            }
        }

        Some(window)
    }

    #[cfg(test)]
    fn wait_connected(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.connected.load(Ordering::Acquire) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.connected.load(Ordering::Acquire)
    }
}

static HYPRLAND_FOCUS_CACHE: Mutex<Option<(PathBuf, Arc<HyprlandFocusCache>)>> = Mutex::new(None);

fn query_cached_hyprland_window(socket_path: PathBuf) -> Option<Option<HyprlandWindow>> {
    let cache = {
        let mut guard = HYPRLAND_FOCUS_CACHE.lock().ok()?;
        match guard.as_ref() {
            Some((cached_path, cache)) if cached_path == &socket_path => Arc::clone(cache),
            _ => {
                let socket2_path = socket_path
                    .parent()
                    .map(|dir| dir.join(".socket2.sock"))
                    .unwrap_or_else(|| PathBuf::from(".socket2.sock"));
                let cache = Arc::new(HyprlandFocusCache::new(
                    socket_path.clone(),
                    socket2_path,
                    FOCUS_CACHE_TTL,
                ));
                *guard = Some((socket_path, Arc::clone(&cache)));
                cache
            }
        }
    };
    cache.query()
}

/// Focused Hyprland window metadata (`class`, `address`, `pid`), if Hyprland
/// can be asked in time.
///
/// Queries Hyprland's direct Unix socket (`.socket.sock`) backed by a lazily
/// started `.socket2.sock` focus-event cache, falling back to
/// `hyprctl activewindow -j` only if socket env/connect fails.
pub fn hyprland_active_window() -> Option<HyprlandWindow> {
    if let Some(socket_path) = hyprland_socket_path(".socket.sock") {
        if let Some(window) = query_cached_hyprland_window(socket_path) {
            return window;
        }
    }
    let mut cmd = std::process::Command::new("hyprctl");
    cmd.args(["activewindow", "-j"]);
    active_window_from(cmd)
}

/// Class of the focused window, if the compositor can be asked in time:
/// Hyprland's `class`, or on niri (`NIRI_SOCKET` set) the window's `app_id`.
/// The ghost addon reports the focused program by the same name, so without
/// this every ghost commit on niri was refused and fell back to typing.
pub fn hyprland_active_class() -> Option<String> {
    hyprland_active_window()
        .map(|w| w.class)
        .filter(|c| !c.is_empty())
        .or_else(|| niri_focused_field("app_id"))
}

/// Address of the focused window, if it can be asked in time. Unlike the
/// class it tells two terminals apart. On niri: `niri:<window id>`.
pub fn hyprland_active_address() -> Option<String> {
    hyprland_active_window()
        .map(|w| w.address)
        .filter(|a| !a.is_empty())
        .or_else(|| niri_focused_field("id").map(|id| format!("niri:{id}")))
}

/// A field of niri's focused window (`niri msg --json focused-window`), only
/// when running under niri.
fn niri_focused_field(field: &str) -> Option<String> {
    std::env::var_os("NIRI_SOCKET")?;
    let mut cmd = std::process::Command::new("niri");
    cmd.args(["msg", "--json", "focused-window"]);
    cmd.stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let out = crate::desktop::output_bounded(cmd, FOCUS_PROBE_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    niri_field_from(&out.stdout, field)
}

/// `app_id` (string) or `id` (number) from niri's focused-window JSON;
/// `null` (nothing focused) gives None.
fn niri_field_from(json: &[u8], field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    match v.get(field)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Run a Hyprland `activewindow -j` style command and read its `class`.
#[cfg(test)]
fn active_class_from(cmd: std::process::Command) -> Option<String> {
    active_window_from(cmd)
        .map(|w| w.class)
        .filter(|c| !c.is_empty())
}

fn active_window_from(mut cmd: std::process::Command) -> Option<HyprlandWindow> {
    cmd.stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let out = crate::desktop::output_bounded(cmd, FOCUS_PROBE_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_hyprland_window_json(&out.stdout).ok()?
}

/// Default socket path, shared with the addon: `$XDG_RUNTIME_DIR/ears/ghost.sock`.
pub fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    runtime.join("ears").join("ghost.sock")
}

/// Escape text for one protocol line.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// PCM payload of a WAV file that may still be being written.
///
/// `pw-record` only fills in the RIFF/data sizes when it closes the file, so
/// the sizes are ignored: everything after the `data` chunk header up to the
/// end of what has been written so far is returned, trimmed to whole 16-bit
/// samples.
pub fn growing_wav_payload(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"data" {
            let body = &bytes[pos + 8..];
            return Some(&body[..body.len() & !1]);
        }
        // Chunks are word-aligned.
        pos = pos.checked_add(8 + size + (size & 1))?;
    }
    None
}

/// Follows a WAV file that is still being written and hands out only the PCM
/// bytes added since the last read, in whole 16-bit samples.
pub struct WavTail {
    path: PathBuf,
    /// Open at the next unread byte once the data chunk has been found.
    file: Option<std::fs::File>,
    /// Odd byte of a sample whose other half has not been written yet.
    carry: Option<u8>,
}

impl WavTail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            file: None,
            carry: None,
        }
    }

    /// PCM bytes written since the last call (empty until the header is).
    pub fn read_new(&mut self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        if self.file.is_none() {
            let bytes = std::fs::read(&self.path)?;
            let Some(payload) = growing_wav_payload(&bytes) else {
                return Ok(Vec::new());
            };
            let offset = payload.as_ptr() as usize - bytes.as_ptr() as usize;
            let mut file = std::fs::File::open(&self.path)?;
            file.seek(SeekFrom::Start(offset as u64))?;
            self.file = Some(file);
        }
        let mut out: Vec<u8> = self.carry.take().into_iter().collect();
        self.file
            .as_mut()
            .expect("opened above")
            .read_to_end(&mut out)?;
        if out.len() % 2 == 1 {
            self.carry = out.pop();
        }
        Ok(out)
    }
}

/// Write mono 16-bit PCM as a WAV file.
pub fn write_pcm16_wav(
    path: &std::path::Path,
    pcm: &[u8],
    sample_rate: u32,
) -> std::io::Result<()> {
    std::fs::write(path, crate::pcm::wav_from_bytes(pcm, sample_rate))
}

/// Client for the `earsghost` fcitx5 addon.
///
/// Connects lazily and reconnects after any failure, so fcitx5 restarts are
/// tolerated. Dropping the client closes the connection, which makes the
/// addon clear any ghost still on screen.
pub struct GhostClient {
    path: PathBuf,
    conn: Option<(UnixStream, BufReader<UnixStream>)>,
    active_class: fn() -> Option<String>,
    /// Remember an addon that does not understand frozen boundaries.
    no_frozen: bool,
    no_panel: bool,
    overflow_probe: fn(&str) -> Option<crate::ghost_terminal::Overflow>,
}

impl GhostClient {
    pub fn new(path: PathBuf) -> Self {
        let mut client = Self::with_focus_probe(path, hyprland_active_class);
        client.overflow_probe = crate::ghost_terminal::overflows;
        client
    }

    /// Client that asks `active_class` which window really has focus.
    pub fn with_focus_probe(path: PathBuf, active_class: fn() -> Option<String>) -> Self {
        Self {
            path,
            conn: None,
            active_class,
            no_frozen: false,
            no_panel: false,
            overflow_probe: |_| None,
        }
    }

    /// Is the addon reachable right now?
    pub fn probe(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("S").map(|(shown, _)| shown)
    }

    /// Observe without claiming the ghost, changing focus, or entering text.
    pub fn snapshot(&mut self) -> Result<GhostSnapshot, GhostError> {
        self.connect()?;
        let result = self
            .exchange_line("T")
            .and_then(|s| GhostSnapshot::parse(&s));
        if result.is_err() {
            self.conn = None;
        }
        result
    }

    /// Does the addon's input context plausibly belong to the focused window?
    ///
    /// fcitx5 keeps reporting the last app as focused when focus moves to a
    /// surface that never enables text input, so its answer alone would put
    /// ghost text (or the final commit) into a window the user left. This
    /// compares the addon's program with Hyprland's active window class. It
    /// is an app-level check, not window identity: two windows of the same
    /// app compare equal. When either side cannot tell, the answer is no, so
    /// the caller types instead of trusting a possibly stale context.
    fn focus_matches(&mut self) -> Result<bool, GhostError> {
        let (shown, program) = self.send("S")?;
        if shown == GhostDisplay::None {
            return Ok(false);
        }
        let Some(program) = program else {
            return Ok(false);
        };
        Ok((self.active_class)().is_some_and(|class| program.eq_ignore_ascii_case(&class)))
    }

    /// Show `text` as the ghost, replacing the previous one. Returns
    /// [`GhostDisplay::None`] (and clears any ghost) when the focused window
    /// cannot show it.
    pub fn preedit(&mut self, text: &str) -> Result<GhostDisplay, GhostError> {
        self.preedit_frozen(text, 0)
    }

    /// Show the exact forced-prefix boundary (UTF-8 bytes). Frozen text is
    /// highlighted preedit; the revisable suffix is underlined. This freezes live
    /// decoding only, not application delivery or an optional final correction.
    pub fn preedit_frozen(
        &mut self,
        text: &str,
        frozen_bytes: usize,
    ) -> Result<GhostDisplay, GhostError> {
        if !self.focus_matches()? {
            self.send("X")?;
            return Ok(GhostDisplay::None);
        }
        let frozen_bytes = crate::freeze::boundary(text, frozen_bytes);
        let overflow = if text.is_empty() {
            None
        } else {
            (self.overflow_probe)(text)
        };
        let wrapped;
        let (text, frozen_bytes) = match &overflow {
            Some(Overflow::Wrap(pane)) => {
                wrapped = crate::ghost_terminal::wrap_into_pane(text, frozen_bytes, pane);
                (wrapped.0.as_str(), wrapped.1)
            }
            _ => (text, frozen_bytes),
        };
        // The wire protocol drops CR; count bytes in the unescaped text the
        // addon will actually receive, not in the original transcript.
        let frozen_bytes = text[..frozen_bytes].bytes().filter(|b| *b != b'\r').count();
        if overflow == Some(Overflow::Panel) && !self.no_panel {
            match self.send(&format!("B {} {}", frozen_bytes, escape(text))) {
                Err(GhostError::Rejected(reply)) if reply == "ERR unknown command" => {
                    self.no_panel = true;
                }
                result => return result.map(|(shown, _)| shown),
            }
        }
        if frozen_bytes > 0 && !self.no_frozen {
            match self.send(&format!("F {} {}", frozen_bytes, escape(text))) {
                Err(GhostError::Rejected(reply)) if reply == "ERR unknown command" => {
                    self.no_frozen = true;
                }
                result => return result.map(|(shown, _)| shown),
            }
        }
        self.send(&format!("P {}", escape(text)))
            .map(|(shown, _)| shown)
    }

    /// Clear the ghost and deliver `text` to the focused application.
    pub fn commit(&mut self, text: &str) -> Delivery {
        match self.focus_matches() {
            Ok(true) => {}
            Ok(false) => {
                let _ = self.send("X");
                return Delivery::NotDelivered;
            }
            // Only a status query was sent: nothing can have been committed.
            Err(_) => return Delivery::NotDelivered,
        }
        match self.send(&format!("C {}", escape(text))) {
            Ok((GhostDisplay::None, _)) => Delivery::NotDelivered,
            Ok(_) => Delivery::Delivered,
            // Could not even connect: the command never left.
            Err(GhostError::Unavailable { .. }) => Delivery::NotDelivered,
            Err(_) => Delivery::Unknown,
        }
    }

    /// Clear the ghost without delivering anything.
    pub fn clear(&mut self) -> Result<GhostDisplay, GhostError> {
        self.send("X").map(|(shown, _)| shown)
    }

    fn connect(&mut self) -> Result<(), GhostError> {
        if self.conn.is_some() {
            return Ok(());
        }
        let stream = UnixStream::connect(&self.path).map_err(|source| GhostError::Unavailable {
            path: self.path.display().to_string(),
            source,
        })?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let reader = BufReader::new(stream.try_clone()?);
        self.conn = Some((stream, reader));
        Ok(())
    }

    fn send(&mut self, line: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
        self.connect()?;
        let result = self.exchange(line);
        if result.is_err() {
            // Any I/O failure invalidates the stream; reconnect next time.
            self.conn = None;
        }
        result
    }

    fn exchange(&mut self, line: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
        parse_reply(&self.exchange_line(line)?)
    }

    fn exchange_line(&mut self, line: &str) -> Result<String, GhostError> {
        let (stream, reader) = self.conn.as_mut().expect("connected above");
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut reply = String::new();
        if reader.read_line(&mut reply)? == 0 {
            return Err(GhostError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "addon closed the connection",
            )));
        }
        Ok(reply.trim_end_matches('\n').to_string())
    }
}

/// Bytes at the start of `shown` that are settled: the longest prefix it
/// shares with `settled`, both as displayed (filtered, spaced). Whatever
/// the filters did differently to the settled text alone is left open.
pub fn frozen_len(shown: &str, settled: &str) -> usize {
    if settled.trim().is_empty() {
        return 0;
    }
    shown
        .char_indices()
        .zip(settled.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8())
}

/// Parse "OK <display> [program]".
fn parse_reply(reply: &str) -> Result<(GhostDisplay, Option<String>), GhostError> {
    let mut words = reply.splitn(3, ' ');
    let shown = match (words.next(), words.next()) {
        (Some("OK"), Some("preedit")) => GhostDisplay::Preedit,
        (Some("OK"), Some("panel")) => GhostDisplay::Panel,
        (Some("OK"), _) => GhostDisplay::None,
        _ => return Err(GhostError::Rejected(reply.to_string())),
    };
    let program = words
        .next()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    Ok((shown, program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    /// Minimal stand-in for the addon: records lines, reports "TestApp" as
    /// focused and answers "OK preedit". With `drop_on_commit` it hangs up
    /// on `C` without replying, like an addon that crashed mid-commit.
    fn fake_addon_with(path: PathBuf, drop_on_commit: bool) -> mpsc::Receiver<String> {
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut writer = stream.try_clone().unwrap();
                let reader = BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    let commit = line.starts_with('C');
                    let reply: &[u8] = if line == "S" {
                        b"OK preedit TestApp\n"
                    } else if line.starts_with("B ") {
                        b"OK panel\n"
                    } else {
                        b"OK preedit\n"
                    };
                    let _ = tx.send(line);
                    if commit && drop_on_commit {
                        break;
                    }
                    let _ = writer.write_all(reply);
                }
            }
        });
        rx
    }

    fn fake_addon(path: PathBuf) -> mpsc::Receiver<String> {
        fake_addon_with(path, false)
    }

    fn focus_on_test_app() -> Option<String> {
        Some("testapp".to_string())
    }

    fn focus_elsewhere() -> Option<String> {
        Some("hover".to_string())
    }

    fn client(path: PathBuf) -> GhostClient {
        GhostClient::with_focus_probe(path, focus_on_test_app)
    }

    /// Commands the client sent, minus the status queries around them.
    fn commands(rx: &mpsc::Receiver<String>) -> Vec<String> {
        rx.try_iter().filter(|l| l != "S").collect()
    }

    #[test]
    fn snapshot_observer_never_claims_focus_or_ghost_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            for reply in ["OK state 5 hello tail\n", "OK state 0 \n"] {
                let mut command = String::new();
                reader.read_line(&mut command).unwrap();
                assert_eq!(command, "T\n");
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let mut observer = GhostClient::with_focus_probe(path, || panic!("observer probed focus"));
        assert_eq!(observer.snapshot().unwrap().frozen_bytes, 5);
        assert!(observer.snapshot().unwrap().text.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn snapshot_boundaries_roundtrip_and_reject_malformed_frames() {
        let state = GhostSnapshot::parse("OK state 4 blå\\nrevisable\\\\tail").unwrap();
        assert_eq!(&state.text[..state.frozen_bytes], "blå");
        assert_eq!(state.text, "blå\nrevisable\\tail");
        assert_eq!(GhostSnapshot::parse("OK state 0 ").unwrap().text, "");
        for bad in [
            "OK state 3 blå",
            "OK state 999 x",
            "OK state no x",
            "ERR unknown command",
        ] {
            assert!(GhostSnapshot::parse(bad).is_err());
        }
    }

    #[test]
    fn old_addon_panel_fallback_preserves_freeze_and_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let line = line.unwrap();
                    let response = if line == "S" {
                        "OK preedit TestApp\n"
                    } else if line.starts_with("B ") {
                        "ERR unknown command\n"
                    } else {
                        "OK preedit\n"
                    };
                    writer.write_all(response.as_bytes()).unwrap();
                    let done = line == "F 5 hello again";
                    seen.push(line);
                    if done {
                        return seen;
                    }
                }
            }
            seen
        });
        let mut client = client(path);
        client.overflow_probe = |_| Some(Overflow::Panel);
        client.preedit_frozen("hello tail", 5).unwrap();
        client.preedit_frozen("hello again", 5).unwrap();
        assert_eq!(
            server.join().unwrap(),
            [
                "S",
                "B 5 hello tail",
                "F 5 hello tail",
                "S",
                "F 5 hello again"
            ]
        );
    }

    #[test]
    fn overflowing_preview_uses_panel_without_changing_commit_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = client(path);
        client.overflow_probe = |_| Some(Overflow::Panel);
        assert_eq!(
            client.preedit_frozen("hello wide world", 5).unwrap(),
            GhostDisplay::Panel
        );
        client.commit("hello wide world");
        assert_eq!(
            commands(&rx),
            ["B 5 hello wide world", "C hello wide world"]
        );
    }

    #[test]
    fn overflowing_preview_wraps_into_the_pane_when_the_terminal_can() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = client(path);
        client.overflow_probe = |_| {
            Some(Overflow::Wrap(crate::ghost_terminal::Pane {
                left: 2,
                width: 10,
                cursor_x: 4,
                rows: 5,
            }))
        };
        assert_eq!(
            client.preedit_frozen("hello wide world", 10).unwrap(),
            GhostDisplay::Preedit
        );
        client.commit("hello wide world");
        // Frozen bytes count the inserted indentation; the commit is untouched.
        assert_eq!(
            commands(&rx),
            ["F 12 hello\\n  wide world", "C hello wide world"]
        );
        // CR removal must not move the boundary into the mutable UTF-8 tail.
        // This used to panic when preedit_frozen sliced the wrapped prefix.
        client.preedit_frozen("\raé tail", 2).unwrap();
        assert_eq!(commands(&rx), ["F 1 aé\\n  tail"]);
    }

    #[test]
    fn frozen_len_is_the_shared_prefix() {
        assert_eq!(frozen_len("Hello world, how", "Hello world,"), 12);
        assert_eq!(frozen_len("Hello world", "Hello there"), 6);
        assert_eq!(frozen_len("héllo", "hé"), 3);
        assert_eq!(frozen_len("hello", ""), 0);
        assert_eq!(frozen_len("hello", "  "), 0);
        assert_eq!(frozen_len("hello", "hello there"), 5);
    }

    #[test]
    fn frozen_preedit_keeps_utf8_boundary_and_escapes_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = client(path);
        client.preedit_frozen("blå\nnext", 4).unwrap();
        client.preedit_frozen("blå\nnext", 3).unwrap();
        assert_eq!(commands(&rx), ["F 4 blå\\nnext", "P blå\\nnext"]);
    }

    #[test]
    fn frozen_preedit_falls_back_with_an_older_addon() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            // Rejected replies discard the connection; the retry reconnects.
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let line = line.unwrap();
                    let response = if line == "S" {
                        "OK preedit TestApp\n"
                    } else if line.starts_with("F ") {
                        "ERR unknown command\n"
                    } else {
                        "OK preedit\n"
                    };
                    writer.write_all(response.as_bytes()).unwrap();
                    let done = line == "P hello again";
                    seen.push(line);
                    if done {
                        return seen;
                    }
                }
            }
            seen
        });
        let mut client = client(path);
        assert_eq!(
            client.preedit_frozen("hello tail", 5).unwrap(),
            GhostDisplay::Preedit
        );
        assert_eq!(
            client.preedit_frozen("hello again", 5).unwrap(),
            GhostDisplay::Preedit
        );
        assert_eq!(
            server.join().unwrap(),
            ["S", "F 5 hello tail", "P hello tail", "S", "P hello again"]
        );
    }

    #[test]
    fn growing_wav_payload_ignores_unfinished_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_pcm16_wav(&path, &[1, 0, 2, 0, 3, 0], 16000).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // Simulate pw-record mid-write: zero sizes and an odd trailing byte.
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        bytes.push(9);
        assert_eq!(growing_wav_payload(&bytes).unwrap(), &[1, 0, 2, 0, 3, 0]);
    }

    #[test]
    fn growing_wav_payload_skips_extra_chunks() {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[7, 7, 7, 0]); // 3 bytes + pad
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[5, 6]);
        assert_eq!(growing_wav_payload(&bytes).unwrap(), &[5, 6]);
        assert!(growing_wav_payload(b"nope").is_none());
    }

    #[test]
    fn wav_tail_hands_out_each_sample_once() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.wav");
        let mut tail = WavTail::new(&path);
        assert!(tail.read_new().is_err(), "no file yet");
        std::fs::write(&path, b"RIFF\0\0\0\0WA").unwrap();
        assert!(tail.read_new().unwrap().is_empty(), "header incomplete");
        write_pcm16_wav(&path, &[1, 0, 2], 16000).unwrap();
        assert_eq!(tail.read_new().unwrap(), vec![1, 0]);
        assert!(tail.read_new().unwrap().is_empty());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&[0, 3]).unwrap();
        assert_eq!(tail.read_new().unwrap(), vec![2, 0]);
        file.write_all(&[0]).unwrap();
        assert_eq!(tail.read_new().unwrap(), vec![3, 0]);
    }

    #[test]
    fn escape_roundtrips_specials() {
        assert_eq!(escape("a\\b\nc\r"), "a\\\\b\\nc");
        assert_eq!(escape("plain text"), "plain text");
    }

    #[test]
    fn parse_replies() {
        assert_eq!(
            parse_reply("OK preedit").unwrap(),
            (GhostDisplay::Preedit, None)
        );
        assert_eq!(parse_reply("OK panel").unwrap().0, GhostDisplay::Panel);
        assert_eq!(parse_reply("OK none").unwrap().0, GhostDisplay::None);
        assert_eq!(
            parse_reply("OK preedit Alacritty").unwrap(),
            (GhostDisplay::Preedit, Some("Alacritty".to_string()))
        );
        assert!(parse_reply("ERR nope").is_err());
        assert!(parse_reply("").is_err());
    }

    #[test]
    fn client_speaks_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = client(path);

        assert_eq!(client.preedit("hello wor").unwrap(), GhostDisplay::Preedit);
        assert_eq!(client.commit("hello world\nls"), Delivery::Delivered);
        client.clear().unwrap();

        assert_eq!(commands(&rx), ["P hello wor", "C hello world\\nls", "X"]);
    }

    #[test]
    fn focus_mismatch_types_instead_of_committing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = GhostClient::with_focus_probe(path, focus_elsewhere);

        assert_eq!(client.preedit("hi").unwrap(), GhostDisplay::None);
        assert_eq!(client.commit("hi"), Delivery::NotDelivered);
        assert_eq!(commands(&rx), ["X", "X"]);
    }

    #[test]
    fn hung_focus_probe_gives_up_quickly() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 5"]);
        let start = std::time::Instant::now();
        assert_eq!(active_class_from(cmd), None);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn focus_probe_reads_class() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", r#"echo '{"class":"hover","pid":1}'"#]);
        assert_eq!(active_class_from(cmd).as_deref(), Some("hover"));
    }

    #[test]
    fn unknown_focus_types_instead_of_committing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let rx = fake_addon(path.clone());
        let mut client = GhostClient::with_focus_probe(path, || None);
        assert_eq!(client.preedit("hi").unwrap(), GhostDisplay::None);
        assert_eq!(client.commit("hi"), Delivery::NotDelivered);
        assert_eq!(commands(&rx), ["X", "X"]);
    }

    #[test]
    fn lost_commit_reply_is_unknown_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let _rx = fake_addon_with(path.clone(), true);
        let mut client = client(path);
        assert_eq!(client.commit("hi"), Delivery::Unknown);
    }

    #[test]
    fn unreachable_addon_is_not_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(dir.path().join("missing.sock"));
        assert_eq!(client.commit("hi"), Delivery::NotDelivered);
    }

    #[test]
    fn unavailable_addon_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(dir.path().join("missing.sock"));
        assert!(matches!(
            client.preedit("x"),
            Err(GhostError::Unavailable { .. })
        ));
    }

    #[test]
    fn reconnects_after_addon_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghost.sock");
        let mut client = client(path.clone());
        assert!(client.preedit("x").is_err());

        let rx = fake_addon(path);
        assert_eq!(client.preedit("y").unwrap(), GhostDisplay::Preedit);
        assert_eq!(commands(&rx), ["P y"]);
    }
    #[test]
    fn niri_focused_window_fields() {
        let json = br#"{"id":8,"title":"ears-typing-test","app_id":"Alacritty","pid":1607,"is_focused":true}"#;
        assert_eq!(
            niri_field_from(json, "app_id").as_deref(),
            Some("Alacritty")
        );
        assert_eq!(niri_field_from(json, "id").as_deref(), Some("8"));
        assert_eq!(niri_field_from(b"null", "app_id"), None);
        assert_eq!(
            niri_field_from(br#"{"id":3,"app_id":null}"#, "app_id"),
            None
        );
    }

    #[test]
    fn parse_hyprland_window_handles_full_empty_and_invalid_json() {
        let full = br#"{"address":"0x55a1","class":"Alacritty","pid":4242,"title":"bash"}"#;
        assert_eq!(
            parse_hyprland_window_json(full),
            Ok(Some(HyprlandWindow {
                class: "Alacritty".into(),
                address: "0x55a1".into(),
                pid: Some(4242),
            }))
        );
        let neg_pid = br#"{"address":"0x1","class":"hover","pid":-1}"#;
        assert_eq!(
            parse_hyprland_window_json(neg_pid),
            Ok(Some(HyprlandWindow {
                class: "hover".into(),
                address: "0x1".into(),
                pid: None,
            }))
        );
        assert_eq!(parse_hyprland_window_json(b"{}"), Ok(None));
        assert!(parse_hyprland_window_json(b"not json").is_err());
    }

    #[test]
    fn hyprland_direct_socket_ipc_roundtrip_and_timeout() {
        use std::io::Read;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join(".socket.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            // Request 1: normal reply
            let (mut stream, _) = listener.accept().unwrap();
            let mut req = [0u8; 64];
            let n = stream.read(&mut req).unwrap();
            assert_eq!(&req[..n], b"j/activewindow");
            stream
                .write_all(br#"{"address":"0xabc","class":"Alacritty","pid":77}"#)
                .unwrap();
            drop(stream);

            // Request 2: hang to verify bounded timeout
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(250));
            drop(stream);
        });

        let bytes = hyprland_ipc_request_at(&sock, "j/activewindow", FOCUS_PROBE_TIMEOUT).unwrap();
        assert_eq!(
            parse_hyprland_window_json(&bytes),
            Ok(Some(HyprlandWindow {
                class: "Alacritty".into(),
                address: "0xabc".into(),
                pid: Some(77),
            }))
        );

        let start = Instant::now();
        assert!(
            hyprland_ipc_request_at(&sock, "j/activewindow", Duration::from_millis(60)).is_none()
        );
        assert!(start.elapsed() < Duration::from_millis(220));
        server.join().unwrap();
    }

    #[test]
    fn hyprland_focus_cache_serves_hits_and_invalidates_on_socket2_events() {
        use std::io::Read;
        use std::sync::atomic::AtomicUsize;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join(".socket.sock");
        let sock2 = dir.path().join(".socket2.sock");
        let req_listener = UnixListener::bind(&sock).unwrap();
        let ev_listener = UnixListener::bind(&sock2).unwrap();

        let current_reply = Arc::new(Mutex::new(
            r#"{"address":"0x1","class":"Alacritty","pid":100}"#.to_string(),
        ));
        let query_count = Arc::new(AtomicUsize::new(0));
        let stop_req = Arc::new(AtomicBool::new(false));

        let reply_ref = Arc::clone(&current_reply);
        let count_ref = Arc::clone(&query_count);
        let stop_ref = Arc::clone(&stop_req);
        let _ = req_listener.set_nonblocking(true);
        let req_server = std::thread::spawn(move || {
            while !stop_ref.load(Ordering::Acquire) {
                match req_listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut buf = [0u8; 64];
                        let _ = stream.read(&mut buf);
                        count_ref.fetch_add(1, Ordering::AcqRel);
                        let reply = reply_ref.lock().unwrap().clone();
                        let _ = stream.write_all(reply.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        let (ev_tx, ev_rx) = mpsc::channel::<Option<String>>();
        let ev_server = std::thread::spawn(move || {
            let (mut stream, _) = ev_listener.accept().unwrap();
            while let Ok(Some(line)) = ev_rx.recv() {
                stream.write_all(line.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
            drop(stream);
        });

        let cache = HyprlandFocusCache::new(sock.clone(), sock2.clone(), Duration::from_secs(5));
        assert!(
            cache.wait_connected(Duration::from_secs(2)),
            "focus cache thread did not connect to .socket2.sock"
        );

        // First query populates cache; subsequent queries do not hit .socket.sock.
        let first = cache.query().unwrap().unwrap();
        assert_eq!(first.class, "Alacritty");
        assert_eq!(first.address, "0x1");
        assert_eq!(first.pid, Some(100));
        assert_eq!(query_count.load(Ordering::Acquire), 1);

        let second = cache.query().unwrap().unwrap();
        assert_eq!(second, first);
        assert_eq!(query_count.load(Ordering::Acquire), 1);

        // Unrelated event must not invalidate the cache.
        let gen_before = cache.generation.load(Ordering::Acquire);
        ev_tx.send(Some("openlayer>>waybar\n".into())).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cache.generation.load(Ordering::Acquire), gen_before);
        let third = cache.query().unwrap().unwrap();
        assert_eq!(third, first);
        assert_eq!(query_count.load(Ordering::Acquire), 1);

        // Focus change event invalidates cache -> next query fetches new window.
        *current_reply.lock().unwrap() =
            r#"{"address":"0x2","class":"firefox","pid":200}"#.to_string();
        ev_tx
            .send(Some("activewindow>>firefox,Mozilla Firefox\n".into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while cache.generation.load(Ordering::Acquire) == gen_before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(cache.generation.load(Ordering::Acquire) > gen_before);

        let fourth = cache.query().unwrap().unwrap();
        assert_eq!(fourth.class, "firefox");
        assert_eq!(fourth.address, "0x2");
        assert_eq!(fourth.pid, Some(200));
        assert_eq!(query_count.load(Ordering::Acquire), 2);

        // closewindow event -> empty workspace `{}` returns Some(None) and caches it.
        let gen_before = cache.generation.load(Ordering::Acquire);
        *current_reply.lock().unwrap() = "{}".to_string();
        ev_tx.send(Some("closewindow>>0x2\n".into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while cache.generation.load(Ordering::Acquire) == gen_before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(cache.query(), Some(None));
        assert_eq!(cache.query(), Some(None));
        assert_eq!(query_count.load(Ordering::Acquire), 3);

        // Clean shutdown.
        drop(cache);
        let _ = ev_tx.send(None);
        ev_server.join().unwrap();
        stop_req.store(true, Ordering::Release);
        req_server.join().unwrap();
    }

    #[test]
    fn hyprland_focus_cache_refreshes_after_ttl() {
        use std::io::Read;
        use std::sync::atomic::AtomicUsize;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join(".socket.sock");
        let sock2 = dir.path().join(".socket2.sock");
        let req_listener = UnixListener::bind(&sock).unwrap();
        let ev_listener = UnixListener::bind(&sock2).unwrap();

        let query_count = Arc::new(AtomicUsize::new(0));
        let stop_req = Arc::new(AtomicBool::new(false));
        let count_ref = Arc::clone(&query_count);
        let stop_ref = Arc::clone(&stop_req);
        let _ = req_listener.set_nonblocking(true);
        let req_server = std::thread::spawn(move || {
            while !stop_ref.load(Ordering::Acquire) {
                match req_listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut buf = [0u8; 64];
                        let _ = stream.read(&mut buf);
                        let n = count_ref.fetch_add(1, Ordering::AcqRel) + 1;
                        let reply =
                            format!(r#"{{"address":"0x{n}","class":"Alacritty","pid":{n}}}"#);
                        let _ = stream.write_all(reply.as_bytes());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        let (ev_tx, ev_rx) = mpsc::channel::<()>();
        let ev_server = std::thread::spawn(move || {
            let (_stream, _) = ev_listener.accept().unwrap();
            let _ = ev_rx.recv();
        });

        let cache = HyprlandFocusCache::new(sock, sock2, Duration::from_millis(40));
        assert!(cache.wait_connected(Duration::from_secs(2)));

        let w1 = cache.query().unwrap().unwrap();
        assert_eq!(w1.pid, Some(1));
        assert_eq!(query_count.load(Ordering::Acquire), 1);

        std::thread::sleep(Duration::from_millis(60));
        let w2 = cache.query().unwrap().unwrap();
        assert_eq!(w2.pid, Some(2));
        assert_eq!(query_count.load(Ordering::Acquire), 2);

        drop(cache);
        let _ = ev_tx.send(());
        ev_server.join().unwrap();
        stop_req.store(true, Ordering::Release);
        req_server.join().unwrap();
    }
}

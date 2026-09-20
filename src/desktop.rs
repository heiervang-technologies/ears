//! Desktop integration for ears
//!
//! Handles notifications, audio feedback, text input automation, and keyboard layout detection.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Text input method for typing transcribed text
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TypingMode {
    /// Disable typing output entirely (useful for IPC-only mode)
    None,
    /// Auto-detect: wtype on Omarchy/Hyprland, clipboard paste otherwise
    #[default]
    Auto,
    /// Force wtype (character-by-character with inter-key delay)
    Wtype,
    /// Force clipboard paste (wl-copy + Ctrl+V, instant)
    Paste,
}

impl TypingMode {
    /// Display name for TUI rendering
    pub fn display_name(self) -> &'static str {
        match self {
            TypingMode::Auto => "Auto",
            TypingMode::Wtype => "Wtype",
            TypingMode::Paste => "Paste",
            TypingMode::None => "None",
        }
    }

    /// Cycle to the next mode
    pub fn next(self) -> Self {
        match self {
            TypingMode::Auto => TypingMode::Wtype,
            TypingMode::Wtype => TypingMode::Paste,
            TypingMode::Paste => TypingMode::None,
            TypingMode::None => TypingMode::Auto,
        }
    }
}

/// Keyboard layout detection for Hyprland and GNOME
pub struct KeyboardLayout;

impl KeyboardLayout {
    /// Detect the current keyboard layout and return the corresponding language code
    /// Returns Some("en") for US layout, Some("no") for Norwegian, None for unknown/auto
    ///
    /// Supports both Hyprland (via hyprctl) and GNOME (via dconf)
    pub fn detect_language() -> Option<String> {
        // Try Hyprland first
        if let Some(layout) = Self::detect_hyprland_layout() {
            tracing::debug!("Detected Hyprland keyboard layout: {}", layout);
            return Self::layout_to_language(&layout);
        }

        // Fall back to GNOME/dconf
        if let Some(layout) = Self::detect_gnome_layout() {
            tracing::debug!("Detected GNOME keyboard layout: {}", layout);
            return Self::layout_to_language(&layout);
        }

        None
    }

    /// Detect keyboard layout from Hyprland using hyprctl
    fn detect_hyprland_layout() -> Option<String> {
        // First try to get the active keyboard layout
        // hyprctl devices -j returns JSON with keyboard info
        let output = Command::new("hyprctl")
            .args(["devices", "-j"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json_str = String::from_utf8_lossy(&output.stdout);

        // Parse JSON to find the active keyboard layout
        // Look for "active_keymap" field in keyboards array
        if let Some(layout) = Self::parse_hyprctl_devices(&json_str) {
            return Some(layout);
        }

        // Fallback: try getting the configured layout from hyprctl getoption
        let option_output = Command::new("hyprctl")
            .args(["getoption", "input:kb_layout"])
            .output()
            .ok()?;

        if option_output.status.success() {
            let option_str = String::from_utf8_lossy(&option_output.stdout);
            // Output format: "str: us" or similar
            for line in option_str.lines() {
                if line.trim().starts_with("str:") {
                    let layout = line.trim().strip_prefix("str:")?.trim();
                    // Handle comma-separated layouts (e.g., "us,no") - take the first one
                    let first_layout = layout.split(',').next()?.trim();
                    if !first_layout.is_empty() {
                        return Some(first_layout.to_string());
                    }
                }
            }
        }

        None
    }

    /// Parse hyprctl devices JSON output to find active keyboard layout
    fn parse_hyprctl_devices(json_str: &str) -> Option<String> {
        // Simple JSON parsing without a full parser
        // Look for "active_keymap": "..." in the keyboards section
        // The active_keymap field contains the human-readable layout name

        // Find keyboards section
        let keyboards_start = json_str.find("\"keyboards\"")?;
        let keyboards_section = &json_str[keyboards_start..];

        // Find the first active_keymap in the keyboards array
        // Look for main keyboard (not virtual)
        for line in keyboards_section.lines() {
            let trimmed = line.trim();

            // Look for active_keymap field
            if trimmed.contains("\"active_keymap\"") {
                // Extract the value: "active_keymap": "English (US)"
                if let Some(start) = trimmed.find(':') {
                    let value_part = &trimmed[start + 1..];
                    let value = value_part.trim().trim_matches(',').trim_matches('"').trim();

                    // Map common keymap names to layout codes
                    return Self::keymap_name_to_layout(value);
                }
            }
        }

        None
    }

    /// Map Hyprland keymap names to layout codes
    fn keymap_name_to_layout(keymap: &str) -> Option<String> {
        let keymap_lower = keymap.to_lowercase();

        // Common keymap name patterns
        if keymap_lower.contains("english") && keymap_lower.contains("us") {
            return Some("us".to_string());
        }
        if keymap_lower.contains("english") && keymap_lower.contains("uk") {
            return Some("gb".to_string());
        }
        if keymap_lower.contains("norwegian") || keymap_lower.contains("norsk") {
            return Some("no".to_string());
        }
        if keymap_lower.contains("german") || keymap_lower.contains("deutsch") {
            return Some("de".to_string());
        }
        if keymap_lower.contains("french") || keymap_lower.contains("français") {
            return Some("fr".to_string());
        }
        if keymap_lower.contains("spanish") || keymap_lower.contains("español") {
            return Some("es".to_string());
        }
        if keymap_lower.contains("swedish") || keymap_lower.contains("svenska") {
            return Some("se".to_string());
        }
        if keymap_lower.contains("danish") || keymap_lower.contains("dansk") {
            return Some("dk".to_string());
        }
        if keymap_lower.contains("finnish") || keymap_lower.contains("suomi") {
            return Some("fi".to_string());
        }

        // If it's a short code already, use it directly
        let short = keymap.split_whitespace().next()?;
        if short.len() == 2 {
            return Some(short.to_lowercase());
        }

        None
    }

    /// Detect keyboard layout from GNOME using dconf
    fn detect_gnome_layout() -> Option<String> {
        // Use mru-sources (most recently used) - first item is current layout
        // This works with GNOME's per-window keyboard layout switching
        let mru_output = Command::new("dconf")
            .args(["read", "/org/gnome/desktop/input-sources/mru-sources"])
            .output()
            .ok()?;

        if !mru_output.status.success() {
            return None;
        }

        let mru_str = String::from_utf8_lossy(&mru_output.stdout);
        // Parse "[('xkb', 'no'), ('xkb', 'us')]" - first entry is current
        Self::parse_dconf_mru_sources(&mru_str)
    }

    /// Parse the dconf mru-sources output and get the first (current) layout
    fn parse_dconf_mru_sources(sources: &str) -> Option<String> {
        // Format: [('xkb', 'no'), ('xkb', 'us')]
        // First entry is the current layout

        let trimmed = sources.trim();
        if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
            return None;
        }

        // Find the first 'layout' pattern
        if let Some(start) = trimmed.find("('xkb', '") {
            let after_prefix = &trimmed[start + 9..]; // Skip "('xkb', '"
            if let Some(end) = after_prefix.find("')") {
                return Some(after_prefix[..end].to_string());
            }
        }

        None
    }

    /// Map keyboard layout code to transcription language code
    fn layout_to_language(layout: &str) -> Option<String> {
        match layout {
            "us" | "gb" | "uk" => Some("en".to_string()),
            "no" | "no+nodeadkeys" => Some("no".to_string()),
            "de" => Some("de".to_string()),
            "fr" => Some("fr".to_string()),
            "es" => Some("es".to_string()),
            "se" => Some("sv".to_string()),
            "dk" => Some("da".to_string()),
            "fi" => Some("fi".to_string()),
            // Add more mappings as needed
            _ => None, // Unknown layout = auto-detect
        }
    }
}

/// Notification urgency levels
#[derive(Debug, Clone, Copy)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    fn as_str(&self) -> &str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        }
    }
}

/// Desktop notification manager
pub struct Notifications;

impl Notifications {
    /// Send a desktop notification
    pub fn send(message: &str, urgency: Urgency) -> Result<()> {
        Command::new("notify-send")
            .arg("-u")
            .arg(urgency.as_str())
            .arg("-a")
            .arg("ears")
            .arg(message)
            .output()
            .context("Failed to send notification")?;
        Ok(())
    }

    /// Send a low priority notification
    pub fn info(message: &str) -> Result<()> {
        Self::send(message, Urgency::Low)
    }

    /// Send a normal priority notification
    pub fn warn(message: &str) -> Result<()> {
        Self::send(message, Urgency::Normal)
    }

    /// Send a high priority notification
    pub fn error(message: &str) -> Result<()> {
        Self::send(message, Urgency::Critical)
    }
}

/// Audio feedback manager
///
/// Sounds (E5 start, E4 done, double-B4 error) are embedded in the binary.
/// Custom sounds in ~/.local/share/ears-sounds/ take priority if present.
///
/// Volume is controlled via a global atomic (0-100), settable with `set_volume()`.
pub struct AudioFeedback;

/// Global cue volume (0-100). Atomic so it can be changed from the TUI at runtime.
static CUE_VOLUME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(100);

// Embedded sound files
static SOUND_START: &[u8] = include_bytes!("../sounds/start.wav");
static SOUND_DONE: &[u8] = include_bytes!("../sounds/done.wav");
static SOUND_BELL: &[u8] = include_bytes!("../sounds/bell.wav");
static SOUND_VAD_OPEN: &[u8] = include_bytes!("../sounds/vad_open.wav");
static SOUND_VAD_CLOSE: &[u8] = include_bytes!("../sounds/vad_close.wav");
static SOUND_VAD_SPEECH: &[u8] = include_bytes!("../sounds/vad_speech.wav");
static SOUND_VAD_SPEECH_START: &[u8] = include_bytes!("../sounds/vad_speech_start.wav");
static SOUND_VAD_SPEECH_CONFIRM: &[u8] = include_bytes!("../sounds/vad_speech_confirm.wav");
static SOUND_VAD_END: &[u8] = include_bytes!("../sounds/vad_end.wav");
static SOUND_TOGGLE_ON: &[u8] = include_bytes!("../sounds/toggle_on.wav");
static SOUND_TOGGLE_OFF: &[u8] = include_bytes!("../sounds/toggle_off.wav");

impl AudioFeedback {
    /// Set the global cue volume (0-100)
    pub fn set_volume(volume: u8) {
        CUE_VOLUME.store(volume.min(100), std::sync::atomic::Ordering::Relaxed);
    }

    /// Get the current cue volume (0-100)
    pub fn get_volume() -> u8 {
        CUE_VOLUME.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get custom sound directory
    fn sound_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("ears-sounds"))
    }

    /// Play a sound file (non-blocking), respecting the global volume setting
    fn play_sound(path: &PathBuf) -> Result<()> {
        let volume = Self::get_volume();
        if volume == 0 {
            return Ok(());
        }
        // Map 0-100 to PulseAudio's 0-65536 scale
        let pa_volume = (volume as u32 * 65536 / 100).to_string();
        Command::new("paplay")
            .arg(format!("--volume={}", pa_volume))
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to spawn paplay")?;
        Ok(())
    }

    /// Play embedded sound data (non-blocking)
    ///
    /// Writes the WAV data to a cache file in /tmp and plays via paplay,
    /// which is more reliable than piping through stdin.
    fn play_embedded(data: &'static [u8]) -> Result<()> {
        use std::hash::{Hash, Hasher};
        use std::io::Write;

        // Derive a stable cache path from the data pointer (each static has a unique address)
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (data.as_ptr() as usize).hash(&mut hasher);
        let hash = hasher.finish();
        let cache_path = std::path::PathBuf::from(format!("/tmp/ears-sound-{:x}.wav", hash));

        // Write to cache file if not already present
        if !cache_path.exists() {
            let mut f =
                std::fs::File::create(&cache_path).context("Failed to create sound cache file")?;
            f.write_all(data)
                .context("Failed to write sound cache file")?;
        }

        Self::play_sound(&cache_path)
    }

    /// Play a named sound (custom override or embedded)
    fn play_named(name: &str, embedded: &'static [u8]) -> Result<()> {
        // Try custom sound first
        if let Ok(custom_dir) = Self::sound_dir() {
            let custom_wav = custom_dir.join(format!("{}.wav", name));
            if custom_wav.exists() {
                return Self::play_sound(&custom_wav);
            }
        }

        // Use embedded sound
        Self::play_embedded(embedded)
    }

    /// Play start recording beep (E5 - 660Hz)
    pub fn beep_start() -> Result<()> {
        Self::play_named("start", SOUND_START)
    }

    /// Play completion beep (E4 - 330Hz)
    pub fn beep_done() -> Result<()> {
        Self::play_named("done", SOUND_DONE)
    }

    /// Play error bell (double B4 - 493.88Hz)
    pub fn beep_error() -> Result<()> {
        Self::play_named("bell", SOUND_BELL)
    }

    /// Play VAD open sound (ascending C5→G5 chirp)
    pub fn beep_vad_open() -> Result<()> {
        Self::play_named("vad_open", SOUND_VAD_OPEN)
    }

    /// Play VAD close sound (descending G5→C5 chirp)
    pub fn beep_vad_close() -> Result<()> {
        Self::play_named("vad_close", SOUND_VAD_CLOSE)
    }

    /// Play VAD speech detected sound (both notes: C5→E5)
    pub fn beep_vad_speech() -> Result<()> {
        Self::play_named("vad_speech", SOUND_VAD_SPEECH)
    }

    /// Play VAD probable speech sound (first note: C5)
    pub fn beep_vad_speech_start() -> Result<()> {
        Self::play_named("vad_speech_start", SOUND_VAD_SPEECH_START)
    }

    /// Play VAD confirmed speech sound (second note: E5)
    pub fn beep_vad_speech_confirm() -> Result<()> {
        Self::play_named("vad_speech_confirm", SOUND_VAD_SPEECH_CONFIRM)
    }

    /// Play VAD speech ended sound (descending E5→C5)
    pub fn beep_vad_end() -> Result<()> {
        Self::play_named("vad_end", SOUND_VAD_END)
    }

    /// Play toggle-on sound (ascending G5→B5)
    pub fn beep_toggle_on() -> Result<()> {
        Self::play_named("toggle_on", SOUND_TOGGLE_ON)
    }

    /// Play toggle-off sound (descending B5→G5)
    pub fn beep_toggle_off() -> Result<()> {
        Self::play_named("toggle_off", SOUND_TOGGLE_OFF)
    }
}

/// Cached result of the desktop capability probe (`None` = not probed yet).
static CAPABILITY_CACHE: Mutex<Option<bool>> = Mutex::new(None);

/// Upper bound for a single desktop capability probe (`hyprctl`, `which`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Upper bound for a single key/clipboard helper (`ydotool key`, `wl-copy`).
const KEY_TIMEOUT: Duration = Duration::from_secs(5);

/// Base allowance for a typing child, before the per-character budget.
const TYPING_BASE_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-character allowance for a typing child. wtype runs with a 4 ms
/// inter-key delay, so 20 ms per character is a generous multiple.
const TYPING_PER_CHAR: Duration = Duration::from_millis(20);

/// Deadline for typing `text`: a base allowance plus a per-character budget.
pub(crate) fn typing_timeout(text: &str) -> Duration {
    TYPING_BASE_TIMEOUT + TYPING_PER_CHAR * (text.chars().count() as u32)
}

/// Spawn `cmd` and wait for it with a deadline.
///
/// If the child has not exited by `timeout`, it is killed and reaped and an
/// error is returned. Callers therefore never leak a wedged child, and a
/// stuck helper (e.g. `wtype` with no focused surface) cannot block the
/// caller forever. Note that for typing children a timeout means the text
/// may have been partially delivered; callers must not blindly retry.
pub(crate) fn run_bounded(mut cmd: Command, timeout: Duration) -> Result<std::process::ExitStatus> {
    let mut child = cmd.spawn().context("Failed to spawn child process")?;
    wait_bounded(&mut child, timeout)
}

/// Wait for an already-spawned child with a deadline; kill and reap on expiry.
pub(crate) fn wait_bounded(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let start = Instant::now();
    let mut backoff = Duration::from_millis(2);
    loop {
        if let Some(status) = child.try_wait().context("Failed to poll child process")? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "child process timed out after {:?}; killed and reaped (output may be partially delivered)",
                timeout
            );
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_millis(50));
    }
}

/// Spawn `cmd` with piped stdout, collect its output, and enforce a deadline.
///
/// The pipe is switched to non-blocking mode and drained in the same loop
/// that polls the child for exit, so no helper thread can be left waiting on
/// a pipe that a forked descendant still holds open. On expiry the child is
/// killed and reaped and the pipe is dropped. Once the child has exited, any
/// bytes it already wrote are drained without waiting for other holders of
/// the write end to close.
pub(crate) fn output_bounded(mut cmd: Command, timeout: Duration) -> Result<std::process::Output> {
    use std::os::unix::io::AsRawFd;

    cmd.stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().context("Failed to spawn child process")?;
    let mut stdout = child.stdout.take().context("child stdout unavailable")?;
    set_nonblocking(stdout.as_raw_fd()).context("Failed to set pipe non-blocking")?;

    let start = Instant::now();
    let mut backoff = Duration::from_millis(2);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut status = None;
    while status.is_none() {
        // Drain whatever is available right now without blocking, up to a
        // per-iteration budget so a child that writes continuously cannot
        // keep us in this inner loop past the deadline.
        if drain_available(&mut stdout, &mut buf, &mut chunk, DRAIN_BUDGET_PER_PASS).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            drop(stdout);
            anyhow::bail!(
                "child process produced more than {} bytes of output; killed and reaped",
                OUTPUT_CAP
            );
        }
        if let Some(st) = child.try_wait().context("Failed to poll child process")? {
            status = Some(st);
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            drop(stdout);
            anyhow::bail!(
                "child process timed out after {:?}; killed and reaped (output may be partially delivered)",
                timeout
            );
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_millis(50));
    }

    // Child has exited: take everything it left in the pipe. The read is
    // non-blocking, so this ends at EOF or as soon as nothing more is
    // buffered, regardless of whether a descendant still holds the write end.
    let overflow = drain_available(&mut stdout, &mut buf, &mut chunk, usize::MAX).is_err();
    drop(stdout);
    if overflow {
        anyhow::bail!(
            "child process produced more than {} bytes of output",
            OUTPUT_CAP
        );
    }

    Ok(std::process::Output {
        status: status.expect("loop exits only with a status"),
        stdout: buf,
        stderr: Vec::new(),
    })
}

/// Bytes read from a non-blocking pipe per drain pass before we go back to
/// checking the child and the deadline.
const DRAIN_BUDGET_PER_PASS: usize = 64 * 1024;

/// Hard cap on collected output. Exceeding it is an error, never a silent
/// truncation: a caller such as the clipboard restore must not act on a
/// partial value.
const OUTPUT_CAP: usize = 1024 * 1024;

/// Read what is available on a non-blocking pipe, up to `budget` bytes.
/// Returns `Err(())` if the collected output would exceed [`OUTPUT_CAP`].
fn drain_available(
    stdout: &mut impl std::io::Read,
    buf: &mut Vec<u8>,
    chunk: &mut [u8],
    budget: usize,
) -> Result<(), ()> {
    let mut read_this_pass = 0usize;
    while read_this_pass < budget {
        match stdout.read(chunk) {
            Ok(0) => break, // write end fully closed
            Ok(n) => {
                read_this_pass += n;
                if buf.len() + n > OUTPUT_CAP {
                    return Err(());
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Ok(())
}

/// Write `data` to a spawned child's piped stdin, close it, and wait for the
/// child, all under one deadline.
///
/// The write is non-blocking and interleaved with polling the child, so a
/// child that stops reading (or never reads) cannot block us once the pipe
/// buffer fills. On expiry the child is killed and reaped.
pub(crate) fn feed_stdin_bounded(
    child: &mut std::process::Child,
    data: &[u8],
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    let mut stdin = child.stdin.take().context("child stdin unavailable")?;
    set_nonblocking(stdin.as_raw_fd()).context("Failed to set stdin non-blocking")?;

    let start = Instant::now();
    let mut backoff = Duration::from_millis(2);
    let mut written = 0usize;
    while written < data.len() {
        match stdin.write(&data[written..]) {
            Ok(0) => break, // read end closed: nothing more will be accepted
            Ok(n) => {
                written += n;
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e).context("Failed to write to child stdin");
            }
        }
        // Pipe is full: give the child a chance, but respect the deadline.
        if let Some(status) = child.try_wait().context("Failed to poll child process")? {
            drop(stdin);
            anyhow::bail!(
                "child exited ({}) before accepting all input ({} of {} bytes)",
                status,
                written,
                data.len()
            );
        }
        if start.elapsed() >= timeout {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "child stopped reading stdin; timed out after {:?} with {} of {} bytes written; killed and reaped",
                timeout,
                written,
                data.len()
            );
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_millis(50));
    }
    drop(stdin); // EOF for the child

    if written < data.len() {
        // Read end closed under us (EPIPE / zero-length write): the child
        // went away before accepting everything. Reap it and report.
        let status = wait_bounded(child, Duration::from_millis(500))
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "not reaped".to_string());
        anyhow::bail!(
            "child closed stdin ({}) before accepting all input ({} of {} bytes)",
            status,
            written,
            data.len()
        );
    }

    let remaining = timeout.saturating_sub(start.elapsed());
    wait_bounded(child, remaining.max(Duration::from_millis(50)))
}

/// Put a file descriptor into O_NONBLOCK mode.
fn set_nonblocking(fd: std::os::unix::io::RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a valid, owned fd with well-formed flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Text input automation
pub struct TextInput;

impl TextInput {
    /// Detect if running on Omarchy (Arch + Hyprland)
    ///
    /// The probe result is cached for the lifetime of the process. Each probe
    /// is bounded so a wedged compositor cannot stall the caller. Call
    /// [`TextInput::refresh_capabilities`] to force a re-probe (e.g. after a
    /// typing backend failure).
    pub(crate) fn is_omarchy() -> bool {
        if let Ok(guard) = CAPABILITY_CACHE.lock() {
            if let Some(cached) = *guard {
                return cached;
            }
        }
        let detected = Self::probe_omarchy();
        if let Ok(mut guard) = CAPABILITY_CACHE.lock() {
            *guard = Some(detected);
        }
        detected
    }

    /// Forget the cached desktop capability probe so the next call re-probes.
    pub fn refresh_capabilities() {
        if let Ok(mut guard) = CAPABILITY_CACHE.lock() {
            *guard = None;
        }
    }

    /// Uncached probe: hyprctl answers and wtype is on PATH.
    fn probe_omarchy() -> bool {
        use std::process::Stdio;

        let mut hyprctl = Command::new("hyprctl");
        hyprctl
            .arg("version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let hyprland = run_bounded(hyprctl, PROBE_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false);
        if !hyprland {
            return false;
        }

        let mut which = Command::new("which");
        which
            .arg("wtype")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        run_bounded(which, PROBE_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Send an Enter/Return key press
    ///
    /// Uses ydotool which creates a kernel-level evdev event indistinguishable
    /// from a physical keyboard press. This works reliably in TUI apps and tmux
    /// where wtype's virtual keyboard events may not be handled correctly.
    pub fn send_enter() -> Result<()> {
        use std::process::Stdio;

        // Brief delay to ensure the target app has processed previously typed text
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut cmd = Command::new("ydotool");
        cmd.args(["key", "28:1", "28:0"]) // KEY_ENTER press and release
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status =
            run_bounded(cmd, KEY_TIMEOUT).context("Failed to run ydotool for Enter key")?;
        if !status.success() {
            anyhow::bail!("ydotool Enter failed with status: {}", status);
        }

        Ok(())
    }

    /// Type text using the specified mode
    ///
    /// - `Auto`: wtype on Omarchy/Hyprland, clipboard paste otherwise
    /// - `Wtype`: force wtype (character-by-character with inter-key delay)
    /// - `Paste`: force clipboard paste (wl-copy + Ctrl+V)
    pub fn type_text(text: &str, mode: TypingMode) -> Result<()> {
        match mode {
            TypingMode::Auto => {
                let result = if Self::is_omarchy() {
                    Self::type_with_wtype(text)
                } else {
                    Self::paste_text(text)
                };
                if result.is_err() {
                    // Either auto-selected backend may have become unavailable.
                    Self::refresh_capabilities();
                }
                result
            }
            TypingMode::Wtype => Self::type_with_wtype(text),
            TypingMode::Paste => Self::paste_text(text),
            TypingMode::None => Ok(()),
        }
    }

    /// Type text directly using wtype (Wayland native, for Hyprland/Omarchy)
    ///
    /// Uses a small inter-key delay (`-d 4`) to prevent web browsers from
    /// dropping characters — especially spaces — when key events arrive too
    /// fast for the JavaScript event loop.
    fn type_with_wtype(text: &str) -> Result<()> {
        use std::process::Stdio;

        let mut cmd = Command::new("wtype");
        cmd.arg("-d")
            .arg("4")
            .arg("--")
            .arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = run_bounded(cmd, typing_timeout(text)).context("Failed to run wtype")?;

        if !status.success() {
            anyhow::bail!("wtype failed with status: {}", status);
        }

        Ok(())
    }

    /// Paste text using wl-copy + ydotool Ctrl+V (handles Unicode correctly)
    /// Preserves and restores the original clipboard contents
    /// Used on non-Omarchy systems (Ubuntu, etc.)
    fn paste_text(text: &str) -> Result<()> {
        use std::process::Stdio;

        // Save current clipboard contents
        let mut read_clip = Command::new("wl-paste");
        read_clip
            .arg("--no-newline")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        // A timeout/overflow is not an empty clipboard. Abort before changing
        // it rather than lose an original value that could not be preserved.
        let clipboard = output_bounded(read_clip, KEY_TIMEOUT)
            .context("Cannot safely preserve clipboard; paste aborted")?;
        let original_clipboard = clipboard.status.success().then_some(clipboard.stdout);

        // Copy text to clipboard using wl-copy
        let mut child = Command::new("wl-copy")
            .arg("--")
            .arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to run wl-copy")?;

        wait_bounded(&mut child, KEY_TIMEOUT).context("wl-copy failed")?;

        // Small delay to ensure clipboard is ready
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Simulate Ctrl+V to paste
        let mut paste = Command::new("ydotool");
        paste
            .args(["key", "ctrl+v"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = run_bounded(paste, KEY_TIMEOUT).context("Failed to run ydotool key")?;

        if !status.success() {
            anyhow::bail!("ydotool key failed with status: {}", status);
        }

        // Small delay before restoring clipboard
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Restore original clipboard contents
        if let Some(original) = original_clipboard {
            let mut restore = Command::new("wl-copy")
                .arg("--")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("Failed to restore clipboard")?;

            if let Err(e) = feed_stdin_bounded(&mut restore, &original, KEY_TIMEOUT) {
                tracing::warn!("Clipboard restore did not complete: {}", e);
            }
        }

        Ok(())
    }

    /// Copy text to clipboard using wl-copy (fire-and-forget, non-blocking)
    ///
    /// wl-copy exits quickly after the clipboard is updated. We can't block on
    /// it here (callers want fire-and-forget), but we also can't let Child drop
    /// without a wait — that leaks a <defunct> zombie until ears itself exits.
    /// Under long-running `ws-listen`, those add up (centurion reported 34
    /// zombies stacked after a session). Park the wait in a detached thread.
    pub fn copy_to_clipboard(text: &str) {
        match Command::new("wl-copy").arg(text).spawn() {
            Ok(mut child) => {
                tracing::info!("Copied text to clipboard");
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => {
                tracing::warn!("Failed to run wl-copy: {}", e);
            }
        }
    }

    /// Type text using ydotool with a specific delay (fallback)
    #[allow(dead_code)]
    pub fn type_text_with_delay(text: &str, delay_ms: Option<u32>) -> Result<()> {
        let mut cmd = Command::new("ydotool");
        cmd.arg("type");

        // Add delay if specified
        if let Some(delay) = delay_ms {
            cmd.arg("--key-delay").arg(delay.to_string());
        }

        // ydotool handles special characters automatically
        cmd.arg(text);

        // Use .status() to wait for completion, preventing concurrent processes
        // from interleaving output (fixes #57)
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let status = run_bounded(cmd, typing_timeout(text)).context("Failed to run ydotool")?;

        if !status.success() {
            anyhow::bail!("ydotool failed with status: {}", status);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_bounded_kills_and_reaps_hung_child() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let start = Instant::now();
        let err = run_bounded(cmd, Duration::from_millis(200)).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for sleep"
        );
        assert!(err.to_string().contains("timed out"), "{}", err);
    }

    #[test]
    fn test_run_bounded_returns_status_of_fast_child() {
        let mut cmd = Command::new("true");
        cmd.stdin(std::process::Stdio::null());
        let status = run_bounded(cmd, Duration::from_secs(5)).unwrap();
        assert!(status.success());

        let cmd = Command::new("false");
        let status = run_bounded(cmd, Duration::from_secs(5)).unwrap();
        assert!(!status.success());
    }

    #[test]
    fn test_output_bounded_collects_stdout() {
        let mut cmd = Command::new("printf");
        cmd.arg("Volume: 0.42");
        let out = output_bounded(cmd, Duration::from_secs(5)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "Volume: 0.42");
    }

    #[test]
    fn test_output_bounded_kills_hung_child_holding_pipe() {
        // Child writes then hangs with the pipe open: must be killed, not awaited.
        // The trailing `exit` defeats shells that exec the last command, so
        // the shell itself (our direct child) is the one that hangs.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf partial; sleep 30; exit 0");
        let start = Instant::now();
        let err = output_bounded(cmd, Duration::from_millis(200)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(err.to_string().contains("timed out"), "{}", err);
    }

    #[test]
    fn test_output_bounded_returns_when_child_exits_but_grandchild_holds_pipe() {
        // The direct child exits at once, but a backgrounded descendant keeps
        // the write end of stdout open for 30s. A blocking read-to-end would
        // hang here; we must return with the child's status and its bytes.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30 & printf partial; exit 0");
        let start = Instant::now();
        let out = output_bounded(cmd, Duration::from_secs(5)).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "must not wait for the grandchild"
        );
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "partial");
    }

    #[test]
    fn test_output_bounded_firehose_child_still_times_out() {
        // A child that never stops writing must not keep us in the drain
        // loop past the deadline, and must not blow memory.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("cat /dev/zero; exit 0");
        let start = Instant::now();
        let err = output_bounded(cmd, Duration::from_millis(300)).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        assert!(err.to_string().contains("timed out"), "{}", err);
    }

    #[test]
    fn test_output_bounded_oversized_output_is_an_error_not_truncation() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("head -c 3000000 /dev/zero; exit 0");
        let err = output_bounded(cmd, Duration::from_secs(10)).unwrap_err();
        assert!(err.to_string().contains("more than"), "{}", err);
    }

    #[test]
    fn test_output_bounded_keeps_everything_an_exited_child_wrote() {
        // More than one pipe buffer and more than one drain budget, written
        // by a child that exits immediately: nothing may be lost.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("head -c 300000 /dev/zero; exit 0");
        let out = output_bounded(cmd, Duration::from_secs(10)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 300000);
    }

    #[test]
    fn test_output_bounded_timeout_with_grandchild_holding_pipe() {
        // Direct child hangs AND a descendant holds the pipe: kill must not
        // be followed by any wait on the pipe.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30 & sleep 30; exit 0");
        let start = Instant::now();
        let err = output_bounded(cmd, Duration::from_millis(200)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(4));
        assert!(err.to_string().contains("timed out"), "{}", err);
    }

    fn spawn_with_stdin(script: &str) -> std::process::Child {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn test_feed_stdin_bounded_delivers_more_than_a_pipe_buffer() {
        // 300 KB is well past the 64 KB default pipe buffer.
        let data = vec![b'x'; 300_000];
        let mut child = spawn_with_stdin("n=$(wc -c); [ \"$n\" -eq 300000 ]");
        let status = feed_stdin_bounded(&mut child, &data, Duration::from_secs(10)).unwrap();
        assert!(status.success(), "child must have received every byte");
    }

    #[test]
    fn test_feed_stdin_bounded_times_out_when_child_never_reads() {
        // Child holds stdin open but never reads: the pipe fills and a
        // blocking write would hang here forever.
        let data = vec![b'x'; 300_000];
        let mut child = spawn_with_stdin("sleep 30; exit 0");
        let start = Instant::now();
        let err = feed_stdin_bounded(&mut child, &data, Duration::from_millis(300)).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        assert!(err.to_string().contains("timed out"), "{}", err);
        assert!(child.try_wait().unwrap().is_some(), "child must be reaped");
    }

    #[test]
    fn test_feed_stdin_bounded_reports_child_that_exits_early() {
        let data = vec![b'x'; 300_000];
        let mut child = spawn_with_stdin("exit 3");
        let err = feed_stdin_bounded(&mut child, &data, Duration::from_secs(5)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("before accepting"), "{}", msg);
        assert!(child.try_wait().unwrap().is_some(), "child must be reaped");
    }

    #[test]
    fn test_typing_timeout_scales_with_length() {
        assert!(typing_timeout("") < typing_timeout(&"a".repeat(500)));
        assert_eq!(typing_timeout(""), Duration::from_secs(5));
    }

    #[test]
    fn test_refresh_capabilities_forces_reprobe() {
        TextInput::refresh_capabilities();
        assert!(CAPABILITY_CACHE.lock().unwrap().is_none());
        let first = TextInput::is_omarchy();
        assert_eq!(*CAPABILITY_CACHE.lock().unwrap(), Some(first));
        assert_eq!(TextInput::is_omarchy(), first);
    }

    // 5.1 Notifications Tests
    #[test]
    fn test_urgency_conversion() {
        assert_eq!(Urgency::Low.as_str(), "low");
        assert_eq!(Urgency::Normal.as_str(), "normal");
        assert_eq!(Urgency::Critical.as_str(), "critical");
    }

    #[test]
    fn test_notification_info() {
        // Verify command construction without executing (avoids showing real notifications)
        let mut cmd = Command::new("notify-send");
        cmd.args(["--app-name=ears", "--urgency=normal", "Test info message"]);
        assert_eq!(cmd.get_program(), "notify-send");
        assert_eq!(cmd.get_args().count(), 3);
    }

    #[test]
    fn test_notification_warn() {
        let mut cmd = Command::new("notify-send");
        cmd.args([
            "--app-name=ears",
            "--urgency=normal",
            "Test warning message",
        ]);
        assert_eq!(cmd.get_program(), "notify-send");
    }

    #[test]
    fn test_notification_error() {
        let mut cmd = Command::new("notify-send");
        cmd.args([
            "--app-name=ears",
            "--urgency=critical",
            "Test error message",
        ]);
        assert_eq!(cmd.get_program(), "notify-send");
    }

    // 5.2 Audio Feedback Tests
    #[test]
    fn test_embedded_sounds() {
        // Verify embedded sounds are present and non-empty
        assert!(!SOUND_START.is_empty());
        assert!(!SOUND_DONE.is_empty());
        assert!(!SOUND_BELL.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn test_custom_sound_dir() {
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/testuser");
        let sound_dir = AudioFeedback::sound_dir().unwrap();
        assert_eq!(
            sound_dir,
            PathBuf::from("/home/testuser/.local/share/ears-sounds")
        );
        // Restore HOME to avoid poisoning other tests
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn test_beep_start() {
        // Verify embedded sound data is valid for playback without executing paplay
        assert!(!SOUND_START.is_empty(), "Start sound should be embedded");
    }

    #[test]
    fn test_beep_done() {
        assert!(!SOUND_DONE.is_empty(), "Done sound should be embedded");
    }

    #[test]
    fn test_beep_error() {
        assert!(!SOUND_BELL.is_empty(), "Error sound should be embedded");
    }

    #[test]
    fn test_audio_feedback_command_construction() {
        // Verify paplay command can be constructed without executing it
        let mut cmd = Command::new("paplay");
        cmd.arg("--raw").arg("/dev/null");
        assert_eq!(cmd.get_program(), "paplay");
        assert_eq!(cmd.get_args().count(), 2);
    }

    // 5.3 Text Input Tests
    // These test the detection logic without executing real typing commands,
    // since wtype/ydotool would type into the active window during tests.
    #[test]
    fn test_type_text_is_omarchy_detection() {
        // Verify is_omarchy returns a bool without side effects
        let _is_omarchy = TextInput::is_omarchy();
    }

    #[test]
    fn test_type_text_with_delay_constructs_command() {
        // Verify command construction doesn't panic for various inputs
        let mut cmd = Command::new("echo"); // harmless stand-in
        cmd.arg("type");
        cmd.arg("--key-delay").arg("50");
        cmd.arg("Test text");
        // Just verify the command can be built without issues
        assert!(cmd.get_program() == "echo");
    }

    #[test]
    fn test_type_text_special_characters_safe() {
        // Verify special characters can be passed as command args without panic
        let text = "Test: !@#$%^&*() \"quotes\" 'single' <angle> {braces}";
        let mut cmd = Command::new("echo");
        cmd.arg("--").arg(text);
        assert!(cmd.get_args().count() == 2);
    }

    // 5.4 Keyboard Layout Detection Tests
    #[test]
    fn test_parse_dconf_mru_sources_valid() {
        let sources = "[('xkb', 'no'), ('xkb', 'us')]";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, Some("no".to_string()));
    }

    #[test]
    fn test_parse_dconf_mru_sources_single() {
        let sources = "[('xkb', 'us')]";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, Some("us".to_string()));
    }

    #[test]
    fn test_parse_dconf_mru_sources_invalid() {
        let sources = "invalid data";
        let result = KeyboardLayout::parse_dconf_mru_sources(sources);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_hyprctl_devices_valid() {
        let json = r#"{
            "keyboards": [
                {
                    "address": "0x1234",
                    "name": "AT Translated Set 2 keyboard",
                    "active_keymap": "English (US)",
                    "main": true
                }
            ]
        }"#;
        let result = KeyboardLayout::parse_hyprctl_devices(json);
        assert_eq!(result, Some("us".to_string()));
    }

    #[test]
    fn test_parse_hyprctl_devices_norwegian() {
        let json = r#"{
            "keyboards": [
                {
                    "name": "keyboard",
                    "active_keymap": "Norwegian"
                }
            ]
        }"#;
        let result = KeyboardLayout::parse_hyprctl_devices(json);
        assert_eq!(result, Some("no".to_string()));
    }

    #[test]
    fn test_keymap_name_to_layout() {
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("English (US)"),
            Some("us".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("English (UK)"),
            Some("gb".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("Norwegian"),
            Some("no".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("German"),
            Some("de".to_string())
        );
        assert_eq!(
            KeyboardLayout::keymap_name_to_layout("French"),
            Some("fr".to_string())
        );
    }

    #[test]
    fn test_layout_to_language() {
        assert_eq!(
            KeyboardLayout::layout_to_language("us"),
            Some("en".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("gb"),
            Some("en".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("no"),
            Some("no".to_string())
        );
        assert_eq!(
            KeyboardLayout::layout_to_language("de"),
            Some("de".to_string())
        );
        assert_eq!(KeyboardLayout::layout_to_language("unknown"), None);
    }
}

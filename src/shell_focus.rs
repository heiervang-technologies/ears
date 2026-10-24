//! Is the focused window a terminal sitting at a shell prompt?
//!
//! VAD mode switches bash mode on per utterance from this: speech aimed at
//! a bash prompt is decoded under the bash grammar, speech aimed at anything
//! else (an agent TUI, an editor, a browser) is plain dictation. Asked once
//! at speech onset, so it is cheap: one `hyprctl` call, then one `tmux`
//! call or a few `/proc` reads.

use crate::ghost_terminal::{output, parent_pid};

/// Foreground programs that mean "a shell prompt".
const SHELLS: &[&str] = &["bash", "zsh", "fish", "sh", "dash"];

/// Is the focused window an Alacritty whose foreground program is a shell?
/// False when anything cannot be determined.
pub fn focused_shell() -> bool {
    foreground_command().is_some_and(|command| is_shell(&command))
}

/// The foreground program of the focused Alacritty window, if it is one.
pub fn foreground_command() -> Option<String> {
    let json = output("hyprctl", &["activewindow", "-j"])?;
    let window = serde_json::from_str::<serde_json::Value>(&json).ok()?;
    // Alacritty gives each window its own PID, so ancestry identifies the
    // window's processes. Multi-window terminal servers would need more.
    if !window["class"].as_str()?.eq_ignore_ascii_case("Alacritty") {
        return None;
    }
    let terminal = window["pid"].as_u64().and_then(|p| u32::try_from(p).ok())?;
    terminal_foreground(terminal)
}

/// The foreground program of the Alacritty window with process `terminal`.
pub fn terminal_foreground(terminal: u32) -> Option<String> {
    if terminal <= 1 {
        return None;
    }
    // In tmux, the pane of the client this window runs decides.
    if let Some(clients) = output(
        "tmux",
        &[
            "list-clients",
            "-F",
            "#{client_pid} #{pane_current_command}",
        ],
    ) {
        if let Some(command) = pane_command(&clients, terminal, parent_pid) {
            return Some(command);
        }
    }
    // Otherwise the foreground process group of the window's shell's tty.
    let shell = child_of(terminal)?;
    let stat = std::fs::read_to_string(format!("/proc/{shell}/stat")).ok()?;
    let foreground = tty_foreground(&stat)?;
    let comm = std::fs::read_to_string(format!("/proc/{foreground}/comm")).ok()?;
    Some(comm.trim().to_string())
}

fn is_shell(command: &str) -> bool {
    let name = command.trim_start_matches('-'); // login shells: "-bash"
    SHELLS.contains(&name)
}

/// `pane_current_command` of the tmux client descended from `terminal`.
fn pane_command(
    clients: &str,
    terminal: u32,
    parent: impl Fn(u32) -> Option<u32>,
) -> Option<String> {
    for row in clients.lines() {
        let Some((pid, command)) = row.split_once(' ') else {
            continue;
        };
        let Ok(mut pid) = pid.parse::<u32>() else {
            continue;
        };
        // Bounded even for malformed proc data.
        for _ in 0..32 {
            if pid == terminal {
                return Some(command.trim().to_string());
            }
            match parent(pid) {
                Some(next) if next > 1 && next != pid => pid = next,
                _ => break,
            }
        }
    }
    None
}

/// The first child process of `pid` (Alacritty's shell).
fn child_of(pid: u32) -> Option<u32> {
    let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok();
    if let Some(child) = children
        .as_deref()
        .and_then(|c| c.split_whitespace().next())
        .and_then(|c| c.parse().ok())
    {
        return Some(child);
    }
    // Kernels without CONFIG_PROC_CHILDREN: scan.
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .find(|&candidate| parent_pid(candidate) == Some(pid))
}

/// `tpgid` (the tty's foreground process group) from a `/proc/<pid>/stat`.
fn tty_foreground(stat: &str) -> Option<u32> {
    // After "comm) ": state ppid pgrp session tty_nr tpgid ...
    let tpgid: i64 = stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(5)?
        .parse()
        .ok()?;
    u32::try_from(tpgid).ok().filter(|&p| p > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_include_login_shells() {
        assert!(is_shell("bash"));
        assert!(is_shell("-zsh"));
        assert!(!is_shell("claude"));
        assert!(!is_shell("nvim"));
        assert!(!is_shell("bashtop"));
    }

    #[test]
    fn tmux_client_of_the_window_decides() {
        // 300 (tmux client) <- 200 (shell) <- 100 (Alacritty); 900 elsewhere.
        let parent = |pid| match pid {
            300 => Some(200),
            200 => Some(100),
            100 => Some(1),
            900 => Some(800),
            800 => Some(1),
            _ => None,
        };
        let clients = "900 claude\n300 bash\n";
        assert_eq!(pane_command(clients, 100, parent).as_deref(), Some("bash"));
        assert_eq!(pane_command("900 claude\n", 100, parent), None);
        assert_eq!(pane_command("garbage\n300\n", 100, parent), None);
    }

    #[test]
    fn foreground_group_comes_from_stat() {
        let stat = "4242 (ba sh) S 100 4242 4242 34816 5151 4194304 0 0";
        assert_eq!(tty_foreground(stat), Some(5151));
        assert_eq!(tty_foreground("4242 (bash) S 100 4242 4242 0 -1 0"), None);
        assert_eq!(tty_foreground("nonsense"), None);
    }
}

//! Keep overflowing tmux previews out of the terminal's preedit overlay.
//! Only a client descended from the focused terminal may supply pane geometry.

use std::process::{Command, Stdio};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    let out = crate::desktop::output_bounded(cmd, Duration::from_millis(100)).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn overflows(text: &str) -> bool {
    let Some(json) = output("hyprctl", &["activewindow", "-j"]) else {
        return false;
    };
    let Ok(window) = serde_json::from_str::<serde_json::Value>(&json) else {
        return false;
    };
    let Some(class) = window["class"].as_str() else {
        return false;
    };
    // Alacritty gives each window its own PID. Multi-window terminal servers
    // need stronger window identity before ancestry can safely select a pane.
    if !class.eq_ignore_ascii_case("Alacritty") {
        return false;
    }
    let Some(pid) = window["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
        return false;
    };
    let Some(clients) = output(
        "tmux",
        &[
            "list-clients",
            "-F",
            "#{client_pid} #{pane_width} #{cursor_x}",
        ],
    ) else {
        return false;
    };
    pane_remaining(&clients, pid, parent_pid).is_some_and(|remaining| exceeds(text, remaining))
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn pane_remaining(
    clients: &str,
    terminal: u32,
    parent: impl Fn(u32) -> Option<u32>,
) -> Option<usize> {
    if terminal <= 1 {
        return None;
    }
    for row in clients.lines() {
        let values: Vec<_> = row.split_whitespace().collect();
        if values.len() != 3 {
            continue;
        }
        let (Ok(mut pid), Ok(width), Ok(cursor)) = (
            values[0].parse::<u32>(),
            values[1].parse::<usize>(),
            values[2].parse::<usize>(),
        ) else {
            continue;
        };
        if width == 0 || cursor > width {
            continue;
        }
        // Bound traversal even for malformed proc data; never use another window's pane.
        for _ in 0..32 {
            if pid == terminal {
                return Some(width - cursor);
            }
            let Some(next) = parent(pid) else { break };
            if next <= 1 || next == pid {
                break;
            }
            pid = next;
        }
    }
    None
}

fn exceeds(text: &str, remaining: usize) -> bool {
    text.contains(['\n', '\r', '\t']) || text.width() > remaining
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_only_the_focused_terminals_client() {
        let clients = "500 120 3\n700 40 35\n";
        let parent = |pid| match pid {
            500 => Some(400),
            700 => Some(600),
            600 => Some(42),
            _ => None,
        };
        assert_eq!(pane_remaining(clients, 42, parent), Some(5));
        assert_eq!(pane_remaining(clients, 99, parent), None);
        assert_eq!(pane_remaining("700 2 3", 42, parent), None);
        assert_eq!(pane_remaining("700 0 0", 42, parent), None);
    }
    #[test]
    fn widths_include_wide_glyphs_and_combining_marks() {
        assert!(!exceeds("hello", 5));
        assert!(exceeds("hello!", 5));
        assert!(!exceeds("你好", 4));
        assert!(exceeds("你好", 3));
        assert!(!exceeds("a\u{301}", 1));
        assert!(exceeds("a\nb", 80));
        assert!(exceeds("a\tb", 80));
        assert!(!exceeds("", 0));
    }
}

//! Keep overflowing tmux previews inside their pane: wrapped in the
//! terminal when it can draw multi-row preedit, else in the fcitx popup.
//! Only a client descended from the focused terminal may supply pane geometry.

use std::process::{Command, Stdio};
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Where a preview too long for the cursor's pane row goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// The fcitx input-method popup.
    Panel,
    /// Wrapped over the pane rows below the cursor (patched Alacritty with
    /// `colors.preedit.wrap`).
    Wrap(Pane),
}

/// The focused tmux pane, in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pane {
    pub left: usize,
    pub width: usize,
    pub cursor_x: usize,
    /// Rows from the cursor's row to the pane bottom, inclusive.
    pub rows: usize,
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    let out = crate::desktop::output_bounded(cmd, Duration::from_millis(100)).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn overflows(text: &str) -> Option<Overflow> {
    let json = output("hyprctl", &["activewindow", "-j"])?;
    let window = serde_json::from_str::<serde_json::Value>(&json).ok()?;
    let class = window["class"].as_str()?;
    // Alacritty gives each window its own PID. Multi-window terminal servers
    // need stronger window identity before ancestry can safely select a pane.
    if !class.eq_ignore_ascii_case("Alacritty") {
        return None;
    }
    let pid = window["pid"].as_u64().and_then(|p| u32::try_from(p).ok())?;
    let clients = output(
        "tmux",
        &[
            "list-clients",
            "-F",
            "#{client_pid} #{pane_width} #{cursor_x} #{pane_left} #{pane_height} #{cursor_y}",
        ],
    )?;
    let pane = focused_pane(&clients, pid, parent_pid)?;
    if !exceeds(text, pane.width - pane.cursor_x) {
        return None;
    }
    Some(if alacritty_wraps() {
        Overflow::Wrap(pane)
    } else {
        Overflow::Panel
    })
}

/// Does the Alacritty config turn on `colors.preedit.wrap`?
fn alacritty_wraps() -> bool {
    crate::ghost_style::alacritty_config()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| wrap_enabled(&text))
}

fn wrap_enabled(config: &str) -> bool {
    config
        .parse::<toml::Table>()
        .ok()
        .and_then(|table| table.get("colors")?.get("preedit")?.get("wrap")?.as_bool())
        .unwrap_or(false)
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

fn focused_pane(clients: &str, terminal: u32, parent: impl Fn(u32) -> Option<u32>) -> Option<Pane> {
    if terminal <= 1 {
        return None;
    }
    for row in clients.lines() {
        let values: Vec<_> = row.split_whitespace().collect();
        if values.len() != 6 {
            continue;
        }
        let Ok(mut pid) = values[0].parse::<u32>() else {
            continue;
        };
        let Ok(numbers) = values[1..]
            .iter()
            .map(|v| v.parse::<usize>())
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        let [width, cursor_x, left, height, cursor_y] = numbers[..] else {
            continue;
        };
        if width == 0 || cursor_x > width || cursor_y >= height {
            continue;
        }
        // Bound traversal even for malformed proc data; never use another window's pane.
        for _ in 0..32 {
            if pid == terminal {
                return Some(Pane {
                    left,
                    width,
                    cursor_x,
                    rows: height - cursor_y,
                });
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

/// Word-wrap `text` into `pane`, starting at the cursor. Continuation rows
/// are indented to the pane's left edge; the terminal leaves indentation
/// undrawn. When the rows below the cursor run out, the oldest words give
/// way to an ellipsis. Returns the wrapped text and `frozen` (a char-boundary
/// byte offset into `text`) mapped into it.
pub(crate) fn wrap_into_pane(text: &str, frozen: usize, pane: &Pane) -> (String, usize) {
    // Map the original prefix through the same normalization as the preview.
    // Keeping the old offset after removing CR can freeze mutable text or
    // leave an offset inside a multibyte character.
    let frozen = text[..crate::freeze::boundary(text, frozen)]
        .bytes()
        .filter(|&b| b != b'\r')
        .count();
    let text = text.replace('\r', "").replace('\t', " ");
    let first = pane.width.saturating_sub(pane.cursor_x).max(1);
    let rows = pane.rows.max(1);

    // Drop whole words from the front until the rest fits the rows.
    let starts = std::iter::once(0).chain(
        text.char_indices()
            .filter(|&(i, c)| i > 0 && c != ' ' && text[..i].ends_with([' ', '\n']))
            .map(|(i, _)| i),
    );
    let mut layout = None;
    for start in starts {
        let lines = wrap_lines(&text, start, first, pane.width.max(1));
        let fits = lines.len() <= rows;
        layout = Some((start, lines));
        if fits {
            break;
        }
    }
    let Some((start, mut lines)) = layout else {
        return (text, frozen);
    };
    // A single word too long for every row: keep its newest rows.
    if lines.len() > rows {
        lines.drain(..lines.len() - rows);
    }

    let mut out = String::new();
    let mut frozen_out = None;
    let elided = start > 0 || lines.first().is_some_and(|line| line.start > start);
    for (n, line) in lines.iter().enumerate() {
        if n > 0 {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', pane.left));
        }
        if frozen_out.is_none() && frozen < line.start {
            frozen_out = Some(out.len());
        }
        out.push_str(&text[line.clone()]);
        if frozen_out.is_none() && frozen <= line.end {
            frozen_out = Some(out.len() - (line.end - frozen));
        }
    }
    let frozen_out = frozen_out.unwrap_or(out.len());
    if elided {
        // The ellipsis replaces the dropped start; it is frozen when that was.
        let mut with = String::from(ELLIPSIS);
        with.push_str(&out);
        let marked = if frozen > 0 { ELLIPSIS.len() } else { 0 };
        return (with, frozen_out + marked);
    }
    (out, frozen_out)
}

const ELLIPSIS: &str = "\u{2026} ";

/// Byte ranges of `text[start..]` per row, greedy word wrap: `first` columns
/// on the cursor row, `width` after. Spaces at a wrap are dropped.
fn wrap_lines(text: &str, start: usize, first: usize, width: usize) -> Vec<std::ops::Range<usize>> {
    // The ellipsis takes room on the first row when words were dropped.
    let mut capacity = if start > 0 {
        first.saturating_sub(ELLIPSIS.width()).max(1)
    } else {
        first
    };
    let mut lines = Vec::new();
    let (mut line_start, mut used) = (start, 0);
    let mut last_space: Option<usize> = None;
    let mut i = start;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let next = i + c.len_utf8();
        if c == '\n' {
            lines.push(line_start..i);
            (line_start, used, last_space, capacity) = (next, 0, None, width);
            i = next;
            continue;
        }
        let w = c.width().unwrap_or(0);
        if used + w > capacity && used > 0 {
            if c == ' ' {
                // Break at this space and drop it.
                lines.push(line_start..i);
                (line_start, used, last_space, capacity) = (next, 0, None, width);
                i = next;
                continue;
            }
            if let Some(space) = last_space {
                lines.push(line_start..space);
                line_start = space + 1;
            } else {
                lines.push(line_start..i);
                line_start = i;
            }
            capacity = width;
            last_space = None;
            used = text[line_start..i].width();
        }
        if c == ' ' {
            last_space = Some(i);
        }
        used += w;
        i = next;
    }
    lines.push(line_start..text.len());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_only_the_focused_terminals_client() {
        let clients = "500 120 3 0 40 39\n700 40 35 80 30 10\n";
        let parent = |pid| match pid {
            500 => Some(400),
            700 => Some(600),
            600 => Some(42),
            _ => None,
        };
        let pane = Pane {
            left: 80,
            width: 40,
            cursor_x: 35,
            rows: 20,
        };
        assert_eq!(focused_pane(clients, 42, parent), Some(pane));
        assert_eq!(focused_pane(clients, 99, parent), None);
        assert_eq!(focused_pane("700 2 3 0 5 0", 42, parent), None);
        assert_eq!(focused_pane("700 0 0 0 5 0", 42, parent), None);
        assert_eq!(focused_pane("700 9 3 0 5 5", 42, parent), None);
        assert_eq!(focused_pane("700 9 3", 42, parent), None);
    }

    fn pane(cursor_x: usize, rows: usize) -> Pane {
        Pane {
            left: 2,
            width: 10,
            cursor_x,
            rows,
        }
    }

    #[test]
    fn wraps_words_into_the_pane() {
        let text = "hello there big world";
        // Cursor row has 6 columns; later rows are indented to the pane.
        let (out, frozen) = wrap_into_pane(text, 11, &pane(4, 5));
        assert_eq!(out, "hello\n  there big\n  world");
        assert_eq!(&out[..frozen], "hello\n  there");
        // A boundary on a dropped space lands at the end of its row.
        let (_, frozen) = wrap_into_pane(text, 5, &pane(4, 5));
        assert_eq!(frozen, 5);
        assert_eq!(wrap_into_pane(text, 0, &pane(4, 5)).1, 0);
        assert_eq!(wrap_into_pane(text, text.len(), &pane(4, 5)).1, out.len());
    }

    #[test]
    fn long_words_break_and_newlines_start_rows() {
        let (out, _) = wrap_into_pane("abcdefghijklm\nxy", 0, &pane(6, 5));
        assert_eq!(out, "abcd\n  efghijklm\n  xy");
        let (out, _) = wrap_into_pane("a\tb\r", 0, &pane(0, 5));
        assert_eq!(out, "a b");
    }

    #[test]
    fn carriage_returns_preserve_the_original_frozen_prefix() {
        for (text, boundary, expected) in [
            ("\raé tail", 2, "a"),
            ("a\r\né tail", 3, "a\n  "),
            ("\rblå\tmutable", 5, "blå"),
            ("blå\r tail", 4, "blå"),
            ("\rblå tail", 0, ""),
        ] {
            let (out, frozen) = wrap_into_pane(text, boundary, &pane(0, 5));
            assert!(out.is_char_boundary(frozen), "{text:?}: {out:?}, {frozen}");
            assert_eq!(&out[..frozen], expected, "{text:?}");
        }
    }

    #[test]
    fn oldest_words_give_way_when_rows_run_out() {
        let text = "one two three four five six seven";
        let (out, frozen) = wrap_into_pane(text, 3, &pane(0, 2));
        assert_eq!(out, "\u{2026} five six\n  seven");
        // The frozen boundary was dropped with its words: only the ellipsis stays frozen.
        assert_eq!(&out[..frozen], "\u{2026} ");
        let (out, frozen) = wrap_into_pane(text, text.len(), &pane(0, 2));
        assert_eq!(frozen, out.len());
        let (_, frozen) = wrap_into_pane(text, 0, &pane(0, 2));
        assert_eq!(frozen, 0);
    }

    #[test]
    fn reads_the_alacritty_wrap_switch() {
        assert!(wrap_enabled("[colors.preedit]\nwrap = true\n"));
        assert!(!wrap_enabled("[colors.preedit]\nwrap = false\n"));
        assert!(!wrap_enabled("[colors.primary]\nforeground = \"#fff\"\n"));
        assert!(!wrap_enabled("not toml ["));
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

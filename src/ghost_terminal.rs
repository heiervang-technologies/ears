//! Keep overflowing tmux previews inside their pane: wrapped in the
//! terminal when it can draw multi-row preedit, else in the fcitx popup.
//! Only a client descended from the focused terminal may supply pane geometry.

use std::ops::Range;
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
    let width = pane.width.max(1);
    let first = pane.width.saturating_sub(pane.cursor_x);
    let rows = pane.rows.max(1);
    // The ellipsis marks dropped words, on the cursor row when it fits there.
    let ellipsis = first >= ELLIPSIS.width();
    let wrap_from = |start: usize| {
        let first = if start > 0 && ellipsis {
            first - ELLIPSIS.width()
        } else {
            first
        };
        wrap_lines(&text, start, first, width)
    };

    // Drop whole words from the front until the rest fits the rows; if even
    // the last word does not, drop characters.
    let words = text
        .char_indices()
        .filter(|&(i, c)| i > 0 && !c.is_whitespace() && text[..i].ends_with([' ', '\n']))
        .map(|(i, _)| i);
    let chars = text
        .char_indices()
        .filter(|&(i, c)| i > 0 && !c.is_whitespace())
        .map(|(i, _)| i);
    let Some((start, lines)) = std::iter::once(0)
        .chain(words)
        .chain(chars)
        .map(|start| (start, wrap_from(start)))
        .find(|(_, lines)| lines.len() <= rows)
    else {
        // Not one character fits (a single row with the cursor at its edge).
        return (String::new(), 0);
    };

    let mut out = String::new();
    let mut frozen_out = None;
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
    if start > 0 && ellipsis {
        // The ellipsis replaces the dropped start; it is frozen when that was.
        let marked = if frozen > 0 { ELLIPSIS.len() } else { 0 };
        return (format!("{ELLIPSIS}{out}"), frozen_out + marked);
    }
    (out, frozen_out)
}

const ELLIPSIS: &str = "\u{2026} ";

/// Byte ranges of `text[start..]` per row: greedy word wrap with `first`
/// columns on the cursor row (possibly none) and `width` after. Spaces at a
/// wrap are dropped; a word longer than a row is split. Only a glyph wider
/// than the whole pane can overflow a row.
fn wrap_lines(text: &str, start: usize, first: usize, width: usize) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut capacity = first;
    let (mut line_start, mut used) = (start, 0);
    // The last space in the row, and the width of the word after it.
    let (mut space, mut word) = (None::<usize>, 0);
    for (offset, c) in text[start..].char_indices() {
        let i = start + offset;
        let next = i + c.len_utf8();
        if c == '\n' {
            lines.push(line_start..i);
            (line_start, used, space, word, capacity) = (next, 0, None, 0, width);
            continue;
        }
        let w = c.width().unwrap_or(0);
        if c == ' ' {
            if used + 1 > capacity {
                // Break here and drop the space.
                lines.push(line_start..i);
                (line_start, used, space, word, capacity) = (next, 0, None, 0, width);
            } else {
                (used, space, word) = (used + 1, Some(i), 0);
            }
            continue;
        }
        if used + w <= capacity {
            used += w;
            word += w;
            continue;
        }
        match space {
            // Move the word to the next row when it fits there whole.
            Some(at) if word + w <= width => {
                lines.push(line_start..at);
                line_start = at + 1;
                used = word + w;
            }
            // Split the word, or leave the cursor row empty when not even
            // this glyph fits on it.
            _ if used > 0 || lines.is_empty() => {
                lines.push(line_start..i);
                line_start = i;
                used = w;
            }
            // A glyph wider than the pane: place it anyway to make progress.
            _ => used += w,
        }
        (space, word, capacity) = (None, used, width);
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
    fn a_long_word_elided_into_one_row_fits_after_the_cursor() {
        // #187: the newest rows of a split word kept their full width.
        let pane = Pane {
            left: 2,
            width: 10,
            cursor_x: 4,
            rows: 1,
        };
        let (out, _) = wrap_into_pane("abcdefghijklmnop", 0, &pane);
        assert_eq!(out, "\u{2026} mnop");
    }

    #[test]
    fn a_cursor_at_the_pane_edge_starts_on_the_next_row() {
        let (out, frozen) = wrap_into_pane("hello world", 5, &pane(10, 3));
        assert_eq!(out, "\n  hello\n  world");
        assert_eq!(&out[..frozen], "\n  hello");
        // One row and no room on it: nothing to show.
        assert_eq!(wrap_into_pane("hello", 3, &pane(10, 1)), (String::new(), 0));
        // Too narrow for the ellipsis: words are dropped without one.
        assert_eq!(wrap_into_pane("ab cd", 0, &pane(9, 1)).0, "d");
    }

    /// Every row fits: the cursor row after the cursor, later rows the pane.
    fn assert_fits(text: &str, frozen: usize, pane: &Pane) {
        let (out, mapped) = wrap_into_pane(text, frozen, pane);
        assert!(
            out.is_char_boundary(mapped),
            "{text:?} {pane:?}: {out:?} {mapped}"
        );
        let rows: Vec<_> = out.split('\n').collect();
        assert!(rows.len() <= pane.rows.max(1), "{text:?} {pane:?}: {out:?}");
        for (n, row) in rows.iter().enumerate() {
            let (room, row) = if n == 0 {
                (pane.width - pane.cursor_x, *row)
            } else {
                (pane.width, &row[pane.left..])
            };
            assert!(row.width() <= room, "{text:?} {pane:?}: row {n} {row:?}");
        }
    }

    #[test]
    fn rows_always_fit_the_pane() {
        let texts = [
            "abcdefghijklmnop",
            "one two three four five six seven",
            "a bb ccc dddd eeeee ffffff ggggggg",
            "你好世界 wide 你好 glyphs",
            "  leading and  double  spaces ",
            "new\nlines\n\nand words",
        ];
        for text in texts {
            for width in 2..12 {
                for cursor_x in 0..=width {
                    for rows in 1..4 {
                        let pane = Pane {
                            left: 1,
                            width,
                            cursor_x,
                            rows,
                        };
                        for frozen in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
                            assert_fits(text, frozen, &pane);
                        }
                    }
                }
            }
        }
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

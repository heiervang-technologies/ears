//! Bash mode text: what the grammar cannot do by itself.
//!
//! The grammar (`grammars/bash.gbnf`) keeps the model's output lowercase,
//! structured shell, but the model still writes what it hears. Symbols that
//! were spoken arrive as words ("ls dash la", "cd dot dot"), and a command
//! may end with the sentence period the grammar lets through. [`clean`]
//! turns those into the shell text that was meant. Quoted strings are left
//! alone: their words are data.

/// Shell text for a constrained transcript: spoken symbols joined, a
/// trailing sentence period or comma dropped.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text.trim();
    while !rest.is_empty() {
        // Unquoted run, then a quoted string (kept verbatim).
        let open = rest.find(['"', '\'']).unwrap_or(rest.len());
        out.push_str(&symbols(&rest[..open]));
        rest = &rest[open..];
        let Some(quote) = rest.chars().next() else {
            break;
        };
        let close = rest[1..].find(quote).map_or(rest.len(), |i| i + 2);
        out.push_str(&rest[..close]);
        rest = &rest[close..];
    }
    let mut out = out.trim().to_string();
    // A word cannot end in "." or "," (see the grammar), so one there is
    // sentence punctuation. Dots after dots or spaces are a path ("cd ..").
    let mut chars = out.chars().rev();
    if let (Some('.' | ','), Some(before)) = (chars.next(), chars.next()) {
        if before.is_alphanumeric() {
            out.pop();
        }
    }
    out
}

/// [`clean`] for a live partial whose first `frozen` bytes have settled:
/// the cleaned text and how much of it is still settled.
pub fn clean_frozen(text: &str, frozen: usize) -> (String, usize) {
    let cleaned = clean(text);
    let frozen = text
        .get(..frozen.min(text.len()))
        .map(clean)
        .filter(|settled| cleaned.starts_with(settled.as_str()))
        .map_or(0, |settled| settled.len());
    (cleaned, frozen)
}

/// Spoken symbols in an unquoted run, keeping its outer whitespace.
fn symbols(run: &str) -> String {
    let lead = &run[..run.len() - run.trim_start().len()];
    let trail = &run[run.trim_end().len()..];
    let words: Vec<&str> = run.split_whitespace().collect();
    if words.is_empty() {
        return run.to_string();
    }
    // Each piece is text plus whether it glues to the previous / next word.
    let mut pieces: Vec<(String, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let (text, glue_prev, glue_next, used) = match (w, next) {
            ("dot", Some("dot")) => ("..", false, false, 2),
            ("dash" | "double", Some("dash")) => ("--", false, true, 2),
            ("dash" | "minus", _) if next.is_some() => ("-", false, true, 1),
            ("dot", _) if i > 0 && next.is_some() => (".", true, true, 1),
            ("slash", _) => (
                "/",
                i > 0 && continues_path(&words, i - 1),
                next.is_some(),
                1,
            ),
            ("pipe", _) => ("|", false, false, 1),
            ("tilde", _) => ("~", false, next.is_some(), 1),
            ("and", Some("and")) if i > 0 => ("&&", false, false, 2),
            _ => (w, false, false, 1),
        };
        pieces.push((text.to_string(), glue_prev, glue_next));
        i += used;
    }
    let mut out = String::from(lead);
    let mut glue = true;
    for (text, glue_prev, glue_next) in pieces {
        if !(glue || glue_prev) {
            out.push(' ');
        }
        out.push_str(&text);
        glue = glue_next;
    }
    out.push_str(trail);
    out
}

/// Does a "slash" after `words[i]` continue it as a path ("src slash
/// main")? Not after a command name ("cd slash etc") or a flag.
fn continues_path(words: &[&str], i: usize) -> bool {
    let separator = |w: &str| matches!(w, "|" | "pipe" | "&&" | "||" | ";");
    let word = words[i];
    let command_position = i == 0 || separator(words[i - 1]) || words[i - 1] == "sudo";
    !command_position && !word.starts_with('-') && !separator(word)
}

#[cfg(test)]
mod tests {
    use super::{clean, clean_frozen};

    #[test]
    fn plain_commands_pass_through() {
        assert_eq!(clean("git status"), "git status");
        assert_eq!(clean("  cargo build --release  "), "cargo build --release");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn sentence_period_is_dropped_but_paths_keep_dots() {
        assert_eq!(clean("git status."), "git status");
        assert_eq!(clean("ls,"), "ls");
        assert_eq!(clean("cd .."), "cd ..");
        assert_eq!(clean("cat readme.md."), "cat readme.md");
        assert_eq!(clean("cd ../.."), "cd ../..");
        assert_eq!(clean("ls ."), "ls .");
    }

    #[test]
    fn spoken_symbols_become_shell() {
        assert_eq!(clean("ls dash la"), "ls -la");
        assert_eq!(
            clean("cargo build dash dash release"),
            "cargo build --release"
        );
        assert_eq!(
            clean("cargo build double dash release"),
            "cargo build --release"
        );
        assert_eq!(clean("cd dot dot"), "cd ..");
        assert_eq!(clean("cat readme dot md"), "cat readme.md");
        assert_eq!(clean("cd slash etc"), "cd /etc");
        assert_eq!(clean("sudo ls slash etc"), "sudo ls /etc");
        assert_eq!(clean("ls src slash main"), "ls src/main");
        assert_eq!(clean("cd tilde slash code"), "cd ~/code");
        assert_eq!(clean("ps aux pipe grep ears"), "ps aux | grep ears");
        assert_eq!(clean("make and and make install"), "make && make install");
        assert_eq!(clean("kill minus nine"), "kill -nine");
    }

    #[test]
    fn quoted_strings_are_data() {
        assert_eq!(
            clean("git commit dash m \"fix the dash dot\""),
            "git commit -m \"fix the dash dot\""
        );
        assert_eq!(clean("echo 'a pipe b' pipe wc"), "echo 'a pipe b' | wc");
        // An unclosed quote (a partial) keeps everything after it.
        assert_eq!(clean("echo \"a dot b"), "echo \"a dot b");
    }

    #[test]
    fn settled_part_survives_cleaning() {
        assert_eq!(clean_frozen("git status.", 3), ("git status".into(), 3));
        assert_eq!(clean_frozen("ls dash la", 7), ("ls -la".into(), 0));
        assert_eq!(clean_frozen("ls dash la", 2), ("ls -la".into(), 2));
        assert_eq!(clean_frozen("ls", 99), ("ls".into(), 2));
    }

    #[test]
    fn a_lone_symbol_word_stays_a_word() {
        assert_eq!(clean("echo dot"), "echo dot");
        assert_eq!(clean("echo dash"), "echo dash");
    }
}

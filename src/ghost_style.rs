//! How ghost text looks, from one setting.
//!
//! The input method only hands applications the ghost's text; each
//! application draws it itself. So the `[ghost]` config is written into the
//! applications that can be told how to draw it:
//!
//! * Alacritty (with the preedit-colors patch): `[colors.preedit]` in
//!   `alacritty.toml`, live-reloaded.
//! * Hover: `hover.ime.ghost_preedit_color` and
//!   `hover.ime.ghost_frozen_color` in each profile's `user.js`, read at
//!   browser start.
//!
//! The frozen colour is for the settled start of the ghost, the part the
//! live decoder will no longer change. ears marks it as the input method's
//! highlighted range, which is what the apps colour.
//!
//! Chromium, Firefox and GTK4 apps such as Walker keep their own style.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Named colours offered in the TUI; any `#rrggbb` works in the config.
pub const PRESETS: &[(&str, &str)] = &[
    ("grey", "#7a8699"),
    ("blue-grey", "#5f7391"),
    ("orange", "#e8913a"),
    ("yellow", "#e0c040"),
    ("green-yellow", "#b8d44a"),
];

/// The `[ghost]` config section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GhostStyle {
    /// `#rrggbb`, `#rgb`, or a preset name. None: each app's default.
    #[serde(default)]
    pub color: Option<String>,
    /// Underline the ghost text. Default: false.
    #[serde(default)]
    pub underline: bool,
    /// Colour of the settled (frozen) start of the ghost, same formats as
    /// `color`. None: drawn like the rest.
    #[serde(default)]
    pub frozen_color: Option<String>,
}

impl GhostStyle {
    /// The colour as `#rrggbb`, if set and valid.
    pub fn hex(&self) -> Option<String> {
        self.color.as_deref().and_then(normalize_color)
    }

    /// The frozen colour as `#rrggbb`, if set and valid.
    pub fn frozen_hex(&self) -> Option<String> {
        self.frozen_color.as_deref().and_then(normalize_color)
    }

    /// The next preset after the current colour (for the TUI).
    pub fn next_preset(&self) -> &'static str {
        next_preset_after(self.hex())
    }

    /// The next frozen colour: presets, then back to none.
    pub fn next_frozen(&self) -> Option<&'static str> {
        match self.frozen_hex() {
            Some(hex) if PRESETS.last().is_some_and(|(_, h)| *h == hex) => None,
            current => Some(next_preset_after(current)),
        }
    }

    /// A short label: the preset name if it is one, else the hex value.
    pub fn label(&self) -> String {
        label_of(self.hex(), "app default")
    }

    /// Label of the frozen colour.
    pub fn frozen_label(&self) -> String {
        label_of(self.frozen_hex(), "same")
    }
}

fn next_preset_after(current: Option<String>) -> &'static str {
    let i = PRESETS
        .iter()
        .position(|(_, hex)| Some(*hex) == current.as_deref())
        .map_or(0, |i| (i + 1) % PRESETS.len());
    PRESETS[i].0
}

fn label_of(hex: Option<String>, none: &str) -> String {
    match hex {
        None => none.to_string(),
        Some(hex) => PRESETS
            .iter()
            .find(|(_, h)| *h == hex)
            .map_or(hex.clone(), |(name, _)| name.to_string()),
    }
}

/// `#rrggbb` for a preset name, `#rrggbb` or `#rgb` (case-insensitive).
pub fn normalize_color(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some((_, hex)) = PRESETS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(value))
    {
        return Some(hex.to_string());
    }
    let digits = value.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        6 => Some(format!("#{}", digits.to_ascii_lowercase())),
        3 => Some(format!(
            "#{}",
            digits
                .chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_ascii_lowercase()
        )),
        _ => None,
    }
}

/// Where one application's setting was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub app: &'static str,
    pub path: PathBuf,
    /// False when the file already said this.
    pub changed: bool,
}

/// Write the style into every supported application found on this machine.
/// Missing applications are skipped; errors are logged and skipped.
pub fn apply(style: &GhostStyle) -> Vec<Applied> {
    let mut applied = Vec::new();
    if let Some(path) = alacritty_config() {
        match apply_alacritty(&path, style) {
            Ok(changed) => applied.push(Applied {
                app: "alacritty",
                path,
                changed,
            }),
            Err(e) => tracing::warn!("Ghost style for Alacritty ({}): {}", path.display(), e),
        }
    }
    for profile in hover_profiles() {
        let path = profile.join("user.js");
        match apply_hover(&path, style) {
            Ok(changed) => applied.push(Applied {
                app: "hover",
                path,
                changed,
            }),
            Err(e) => tracing::warn!("Ghost style for Hover ({}): {}", path.display(), e),
        }
    }
    applied
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn alacritty_config() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    let path = base.join("alacritty").join("alacritty.toml");
    path.exists().then_some(path)
}

/// Profile directories listed in `~/.hover/profiles.ini`.
fn hover_profiles() -> Vec<PathBuf> {
    let Some(root) = home().map(|h| h.join(".hover")) else {
        return Vec::new();
    };
    let Ok(ini) = std::fs::read_to_string(root.join("profiles.ini")) else {
        return Vec::new();
    };
    profile_dirs(&root, &ini)
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

/// Profile paths from a Mozilla `profiles.ini`.
fn profile_dirs(root: &Path, ini: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut in_profile = false;
    let mut relative = true;
    let mut path: Option<String> = None;
    let flush = |path: &mut Option<String>, relative: bool, dirs: &mut Vec<PathBuf>| {
        if let Some(p) = path.take() {
            let dir = if relative {
                root.join(&p)
            } else {
                PathBuf::from(&p)
            };
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    };
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            flush(&mut path, relative, &mut dirs);
            in_profile = line.starts_with("[Profile");
            relative = true;
        } else if in_profile {
            if let Some(v) = line.strip_prefix("Path=") {
                path = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("IsRelative=") {
                relative = v != "0";
            }
        }
    }
    flush(&mut path, relative, &mut dirs);
    dirs
}

/// Set `[colors.preedit]` in an alacritty.toml, keeping everything else.
fn apply_alacritty(path: &Path, style: &GhostStyle) -> std::io::Result<bool> {
    let old = std::fs::read_to_string(path)?;
    // Ghost text is drawn over the terminal, without a background box.
    let mut keys: Vec<(&str, String)> = vec![
        ("underline", style.underline.to_string()),
        ("background", "false".to_string()),
    ];
    // Without a colour the section keeps whatever foreground it has.
    if let Some(hex) = style.hex() {
        keys.insert(0, ("foreground", format!("\"{hex}\"")));
    }
    // An empty value removes the key: no frozen colour, no highlight.
    keys.push((
        "highlight_foreground",
        style
            .frozen_hex()
            .map_or(String::new(), |hex| format!("\"{hex}\"")),
    ));
    let new = set_toml_keys(&old, "colors.preedit", &keys);
    write_if_changed(path, &old, &new)
}

/// Set the Hover ghost colour prefs in a user.js, keeping everything else.
fn apply_hover(path: &Path, style: &GhostStyle) -> std::io::Result<bool> {
    let old = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let prefs = [
        ("hover.ime.ghost_preedit_color", style.hex()),
        ("hover.ime.ghost_frozen_color", style.frozen_hex()),
    ];
    let mut lines: Vec<String> = old
        .lines()
        .filter(|l| !prefs.iter().any(|(p, _)| l.contains(&format!("\"{p}\""))))
        .map(str::to_string)
        .collect();
    for (pref, hex) in prefs {
        lines.push(format!(
            "user_pref(\"{pref}\", \"{}\");",
            hex.unwrap_or_default()
        ));
    }
    let mut new = lines.join("\n");
    new.push('\n');
    write_if_changed(path, &old, &new)
}

/// Replace or add `key = value` lines in `[section]`; add the section at the
/// end if missing. A key with an empty value is removed instead. Other
/// lines, comments and formatting are kept.
fn set_toml_keys(text: &str, section: &str, keys: &[(&str, String)]) -> String {
    let header = format!("[{section}]");
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        let mut out = text.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("# Ghost text style, written by ears (`[ghost]` in its config).\n");
        out.push_str(&header);
        out.push('\n');
        for (k, v) in keys.iter().filter(|(_, v)| !v.is_empty()) {
            out.push_str(&format!("{k} = {v}\n"));
        }
        return out;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| start + 1 + i);
    let mut out: Vec<String> = lines[..=start].iter().map(|l| l.to_string()).collect();
    let mut pending: Vec<&(&str, String)> = keys.iter().collect();
    for line in &lines[start + 1..end] {
        let key = line.split('=').next().unwrap_or("").trim();
        if let Some(i) = pending.iter().position(|(k, _)| *k == key) {
            let (k, v) = pending.remove(i);
            if !v.is_empty() {
                out.push(format!("{k} = {v}"));
            }
        } else {
            out.push(line.to_string());
        }
    }
    // New keys go after the section's last setting, before blank lines.
    let mut insert_at = out.len();
    while insert_at > start + 1 && out[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }
    for (k, v) in pending.into_iter().filter(|(_, v)| !v.is_empty()) {
        out.insert(insert_at, format!("{k} = {v}"));
        insert_at += 1;
    }
    out.extend(lines[end..].iter().map(|l| l.to_string()));
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Replace the file (through symlinks, e.g. into a dotfiles repo) in one step.
fn write_if_changed(path: &Path, old: &str, new: &str) -> std::io::Result<bool> {
    if old == new {
        return Ok(false);
    }
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = target.with_extension("ears-tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_normalize() {
        assert_eq!(normalize_color("#E0C040").as_deref(), Some("#e0c040"));
        assert_eq!(normalize_color("#fa0").as_deref(), Some("#ffaa00"));
        assert_eq!(normalize_color("Orange").as_deref(), Some("#e8913a"));
        assert_eq!(normalize_color("e0c040"), None);
        assert_eq!(normalize_color("#12345"), None);
        assert_eq!(normalize_color("#gg0000"), None);
    }

    #[test]
    fn presets_cycle() {
        let mut style = GhostStyle::default();
        assert_eq!(style.label(), "app default");
        assert_eq!(style.next_preset(), "grey");
        style.color = Some("yellow".into());
        assert_eq!(style.label(), "yellow");
        assert_eq!(style.next_preset(), "green-yellow");
        style.color = Some("green-yellow".into());
        assert_eq!(style.next_preset(), "grey");
        style.color = Some("#123456".into());
        assert_eq!(style.label(), "#123456");
    }

    const ALACRITTY: &str = "\
[window]
opacity = 0.9

# Ears ghost completion
[colors.preedit]
foreground = \"#5f7391\"
underline = false

[font]
size = 11
";

    #[test]
    fn toml_section_is_updated_in_place() {
        let keys = [
            ("foreground", "\"#e0c040\"".to_string()),
            ("underline", "true".to_string()),
        ];
        let out = set_toml_keys(ALACRITTY, "colors.preedit", &keys);
        assert_eq!(
            out,
            ALACRITTY
                .replace("\"#5f7391\"", "\"#e0c040\"")
                .replace("underline = false", "underline = true")
        );
    }

    #[test]
    fn toml_missing_keys_and_section_are_added() {
        let text = "[colors.preedit]\nforeground = \"#111111\"\n\n[font]\nsize = 11\n";
        let out = set_toml_keys(text, "colors.preedit", &[("underline", "false".into())]);
        assert_eq!(
            out,
            "[colors.preedit]\nforeground = \"#111111\"\nunderline = false\n\n[font]\nsize = 11\n"
        );
        let out = set_toml_keys(
            "[font]\nsize = 11\n",
            "colors.preedit",
            &[("underline", "false".into())],
        );
        assert!(out.starts_with("[font]\nsize = 11\n\n# Ghost text style"));
        assert!(out.ends_with("[colors.preedit]\nunderline = false\n"));
        assert!(toml::from_str::<toml::Value>(&out).is_ok());
    }

    #[test]
    fn alacritty_file_is_written_through_symlinks_only_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        std::fs::write(&real, ALACRITTY).unwrap();
        let link = dir.path().join("alacritty.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let style = GhostStyle {
            color: Some("orange".into()),
            underline: false,
            ..Default::default()
        };
        assert!(apply_alacritty(&link, &style).unwrap());
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::read_to_string(&real)
            .unwrap()
            .contains("foreground = \"#e8913a\""));
        assert!(
            !apply_alacritty(&link, &style).unwrap(),
            "unchanged the second time"
        );
    }

    #[test]
    fn hover_unreadable_content_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.js");
        let bytes = b"user_pref(\"custom\", true);\n\xff";
        std::fs::write(&path, bytes).unwrap();
        let error = apply_hover(&path, &GhostStyle::default()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn hover_missing_file_is_created_but_directory_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.js");
        assert!(apply_hover(&path, &GhostStyle::default()).unwrap());
        assert!(!apply_hover(&path, &GhostStyle::default()).unwrap());
        let invalid = dir.path().join("directory.js");
        std::fs::create_dir(&invalid).unwrap();
        assert!(apply_hover(&invalid, &GhostStyle::default()).is_err());
        assert!(invalid.is_dir());
    }

    #[test]
    fn hover_pref_is_replaced_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.js");
        std::fs::write(&path, "user_pref(\"a.b\", true);\n").unwrap();
        let style = GhostStyle {
            color: Some("#e0c040".into()),
            underline: false,
            ..Default::default()
        };
        assert!(apply_hover(&path, &style).unwrap());
        let style = GhostStyle {
            color: Some("green-yellow".into()),
            underline: false,
            ..Default::default()
        };
        assert!(apply_hover(&path, &style).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "user_pref(\"a.b\", true);\nuser_pref(\"hover.ime.ghost_preedit_color\", \"#b8d44a\");\nuser_pref(\"hover.ime.ghost_frozen_color\", \"\");\n"
        );
    }

    #[test]
    fn frozen_colour_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alacritty.toml");
        std::fs::write(&path, ALACRITTY).unwrap();
        let mut style = GhostStyle {
            color: Some("grey".into()),
            frozen_color: Some("#fff".into()),
            ..Default::default()
        };
        assert!(apply_alacritty(&path, &style).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(
            "underline = false\nbackground = false\nhighlight_foreground = \"#ffffff\"\n"
        ));
        style.frozen_color = None;
        assert!(apply_alacritty(&path, &style).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("highlight_foreground"), "{text}");
        assert!(toml::from_str::<toml::Value>(&text).is_ok());
    }

    #[test]
    fn frozen_presets_cycle_through_none() {
        let mut style = GhostStyle::default();
        assert_eq!(style.frozen_label(), "same");
        assert_eq!(style.next_frozen(), Some("grey"));
        style.frozen_color = Some("green-yellow".into());
        assert_eq!(style.next_frozen(), None);
        style.frozen_color = Some("orange".into());
        assert_eq!(style.frozen_label(), "orange");
        assert_eq!(style.next_frozen(), Some("yellow"));
    }

    #[test]
    fn profiles_ini_is_parsed() {
        let ini = "[Install4F96D1932A9F858E]\nDefault=abc.Default\n\n[Profile1]\nName=x\nIsRelative=1\nPath=abc.Default\n\n[Profile0]\nName=y\nIsRelative=0\nPath=/abs/prof\n\n[General]\nVersion=2\n";
        let root = Path::new("/home/u/.hover");
        assert_eq!(
            profile_dirs(root, ini),
            vec![root.join("abc.Default"), PathBuf::from("/abs/prof")]
        );
    }
}

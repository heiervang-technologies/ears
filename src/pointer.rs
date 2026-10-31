//! The screen and the pointer, on Hyprland or sway.
//!
//! Vision mode needs four things from the compositor: which output has focus
//! (and where it sits in the layout), a screenshot of it, and moving and
//! clicking the pointer. Hyprland and sway answer these differently:
//!
//! | | Hyprland | sway |
//! |---|---|---|
//! | output | `hyprctl -j monitors` | `swaymsg -t get_outputs` |
//! | screenshot | `grim -o OUT` | `grim -o OUT` |
//! | move | `hyprctl dispatch movecursor` | `swaymsg seat - cursor set` |
//! | click, scroll | `ydotool` | `swaymsg seat - cursor press/release` |
//!
//! sway is checked first (`$SWAYSOCK`): a sway machine may carry a
//! `hyprctl` shim that answers `activewindow` only.

use anyhow::{bail, Context, Result};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    Hyprland,
    Sway,
}

impl Compositor {
    pub fn detect() -> Result<Self> {
        if std::env::var_os("SWAYSOCK").is_some() {
            Ok(Self::Sway)
        } else if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            Ok(Self::Hyprland)
        } else {
            bail!("neither sway ($SWAYSOCK) nor Hyprland ($HYPRLAND_INSTANCE_SIGNATURE) found")
        }
    }
}

/// The focused output, in layout (logical) coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// Logical size.
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

fn run(cmd: &str, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        bail!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn json(cmd: &str, args: &[&str]) -> Result<serde_json::Value> {
    serde_json::from_slice(&run(cmd, args)?).with_context(|| format!("{cmd}: not JSON"))
}

pub fn focused_output(compositor: Compositor) -> Result<Output> {
    match compositor {
        Compositor::Hyprland => {
            let monitors = json("hyprctl", &["-j", "monitors"])?;
            let m = monitors
                .as_array()
                .and_then(|a| a.iter().find(|m| m["focused"].as_bool() == Some(true)))
                .context("no focused monitor")?;
            let scale = m["scale"].as_f64().unwrap_or(1.0);
            // Hyprland reports the mode in pixels; a rotated output swaps them.
            let (mut w, mut h) = (
                m["width"].as_f64().unwrap_or(0.0),
                m["height"].as_f64().unwrap_or(0.0),
            );
            if m["transform"].as_i64().unwrap_or(0) % 2 == 1 {
                std::mem::swap(&mut w, &mut h);
            }
            Ok(Output {
                name: m["name"].as_str().unwrap_or_default().to_string(),
                x: m["x"].as_f64().unwrap_or(0.0),
                y: m["y"].as_f64().unwrap_or(0.0),
                width: w / scale,
                height: h / scale,
                scale,
            })
        }
        Compositor::Sway => {
            let outputs = json("swaymsg", &["-r", "-t", "get_outputs"])?;
            let o = outputs
                .as_array()
                .and_then(|a| a.iter().find(|o| o["focused"].as_bool() == Some(true)))
                .context("no focused output")?;
            let r = &o["rect"];
            Ok(Output {
                name: o["name"].as_str().unwrap_or_default().to_string(),
                x: r["x"].as_f64().unwrap_or(0.0),
                y: r["y"].as_f64().unwrap_or(0.0),
                width: r["width"].as_f64().unwrap_or(0.0),
                height: r["height"].as_f64().unwrap_or(0.0),
                scale: o["scale"].as_f64().unwrap_or(1.0),
            })
        }
    }
}

/// JPEG of the whole output, in its pixels.
pub fn screenshot(output: &Output) -> Result<Vec<u8>> {
    run("grim", &["-o", &output.name, "-t", "jpeg", "-q", "85", "-"])
}

/// Move the pointer to layout coordinates.
pub fn move_to(compositor: Compositor, x: f64, y: f64) -> Result<()> {
    let (x, y) = (x.round().to_string(), y.round().to_string());
    match compositor {
        Compositor::Hyprland => run("hyprctl", &["dispatch", "movecursor", &format!("{x} {y}")])?,
        Compositor::Sway => run("swaymsg", &["seat", "-", "cursor", "set", &x, &y])?,
    };
    Ok(())
}

pub fn click(compositor: Compositor, button: Button) -> Result<()> {
    match compositor {
        Compositor::Hyprland => {
            let code = match button {
                Button::Left => "0xC0",
                Button::Right => "0xC1",
                Button::Middle => "0xC2",
            };
            run("ydotool", &["click", code])?;
        }
        Compositor::Sway => {
            let b = match button {
                Button::Left => "button1",
                Button::Middle => "button2",
                Button::Right => "button3",
            };
            run("swaymsg", &["seat", "-", "cursor", "press", b])?;
            run("swaymsg", &["seat", "-", "cursor", "release", b])?;
        }
    }
    Ok(())
}

/// Scroll by `notches` wheel steps where the pointer is: positive is down.
pub fn scroll(compositor: Compositor, notches: i32) -> Result<()> {
    match compositor {
        Compositor::Hyprland => {
            run(
                "ydotool",
                &["mousemove", "--wheel", "--", "0", &(-notches).to_string()],
            )?;
        }
        Compositor::Sway => {
            // sway turns button4/5 into one axis step each.
            let b = if notches < 0 { "button4" } else { "button5" };
            for _ in 0..notches.unsigned_abs() {
                run("swaymsg", &["seat", "-", "cursor", "press", b])?;
                run("swaymsg", &["seat", "-", "cursor", "release", b])?;
            }
        }
    }
    Ok(())
}

//! Vision mode: find what was described on screen with SAM 3, show it, act.
//!
//! See `docs/VISION_MODE.md`. One look is: screenshot of the focused output,
//! `POST /segment` with the described target as SAM's text prompt,
//! candidates ranked and drawn as a pink overlay (`vision.json` plus a mask
//! image, read by the ears LED scene), then an action on the picked
//! candidate at the point deepest inside its mask.
//!
//! Coordinates in `vision.json` are output-local logical pixels, the same
//! shape (`x, y, w, h`) other target finders on the desktop use.

use crate::pointer::{self, Button, Compositor, Output};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Vision mode's colour (cyan and teal are dictation's).
pub const PINK: [u8; 3] = [0xff, 0x4f, 0xd8];
/// What `vision.json` is called in the runtime dir.
pub const FILE: &str = "vision.json";
/// The overlay image beside it.
pub const IMAGE: &str = "vision.png";
/// Candidates shown at most.
pub const MAX_CANDIDATES: usize = 5;

/// `[vision]` config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionConfig {
    /// The sam-segment server (SAM 3, `POST /segment`).
    #[serde(default = "default_server")]
    pub server: String,
    /// Seconds to wait for the answer to a guess. Default: 8.
    #[serde(default = "default_confirm_s")]
    pub confirm_s: u64,
}

fn default_server() -> String {
    "http://192.168.8.170:30190".to_string()
}

fn default_confirm_s() -> u64 {
    8
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            server: default_server(),
            confirm_s: default_confirm_s(),
        }
    }
}

/// What to do with the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Show it and wait for a command.
    Outline,
    Click(ButtonName),
    DoubleClick,
    Move,
    /// Scroll there; positive is down.
    Scroll(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonName {
    Left,
    Right,
    Middle,
}

impl From<ButtonName> for Button {
    fn from(b: ButtonName) -> Self {
        match b {
            ButtonName::Left => Button::Left,
            ButtonName::Right => Button::Right,
            ButtonName::Middle => Button::Middle,
        }
    }
}

/// Which of several matches is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// 1-based, in reading order.
    Nth(usize),
    Last,
    Top,
    Bottom,
    Left,
    Right,
}

/// A spoken request, split into SAM's prompt and what ears applies itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub action: Action,
    pub which: Option<Which>,
    /// The noun phrase for SAM, articles and position words removed.
    pub target: String,
}

const VERBS: &[(&str, Action)] = &[
    ("right click", Action::Click(ButtonName::Right)),
    ("left click", Action::Click(ButtonName::Left)),
    ("middle click", Action::Click(ButtonName::Middle)),
    ("double click", Action::DoubleClick),
    ("click on", Action::Click(ButtonName::Left)),
    ("click", Action::Click(ButtonName::Left)),
    ("press", Action::Click(ButtonName::Left)),
    ("move to", Action::Move),
    ("hover over", Action::Move),
    ("hover", Action::Move),
    ("go to", Action::Move),
];

const ORDINALS: &[(&str, usize)] = &[
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
    ("sixth", 6),
];

/// Spoken numbers up to twenty (and digits).
pub fn number(word: &str) -> Option<i32> {
    const WORDS: &[&str] = &[
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    let w = word.trim_matches(|c: char| !c.is_alphanumeric());
    w.parse()
        .ok()
        .or_else(|| WORDS.iter().position(|&n| n == w).map(|i| i as i32))
        .or(match w {
            "to" | "too" => Some(2),
            "for" => Some(4),
            "thirty" => Some(30),
            "forty" => Some(40),
            "fifty" => Some(50),
            _ => None,
        })
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// "up twenty", "scroll down five", "down": a scroll, positive down.
pub fn parse_scroll(text: &str) -> Option<i32> {
    let w = words(text);
    let w: Vec<&str> = w
        .iter()
        .map(String::as_str)
        .filter(|w| *w != "scroll")
        .collect();
    let sign = match *w.first()? {
        "up" => -1,
        "down" => 1,
        _ => return None,
    };
    let n = match w.get(1) {
        None => 5,
        Some(word) => number(word)?,
    };
    (w.len() <= 2).then_some(sign * n.clamp(1, 50))
}

impl Request {
    pub fn parse(text: &str) -> Request {
        let mut w = words(text);
        let mut action = Action::Outline;
        let joined = w.join(" ");
        for (verb, a) in VERBS {
            if joined == *verb || joined.starts_with(&format!("{verb} ")) {
                action = *a;
                w.drain(..verb.split(' ').count());
                break;
            }
        }
        let mut which = None;
        // "on the left", "at the top", "top", "leftmost", "the second", "number 2", "last"
        let mut keep = Vec::new();
        let mut i = 0;
        while i < w.len() {
            let word = w[i].as_str();
            let pos = match word {
                "top" | "topmost" | "upper" => Some(Which::Top),
                "bottom" | "bottommost" | "lower" => Some(Which::Bottom),
                "left" | "leftmost" => Some(Which::Left),
                "right" | "rightmost" => Some(Which::Right),
                "last" => Some(Which::Last),
                _ => ORDINALS
                    .iter()
                    .find(|(o, _)| *o == word)
                    .map(|(_, n)| Which::Nth(*n)),
            };
            // A position word only at the edges of the phrase ("left arrow"
            // in the middle stays part of the target).
            let edge = i == 0
                || i + 1 == w.len()
                || (i >= 1 && matches!(w[i - 1].as_str(), "the" | "on" | "at"));
            if let (Some(p), true, None) = (pos, edge, which) {
                which = Some(p);
            } else if word == "number" && i + 1 < w.len() {
                if let Some(n) = number(&w[i + 1]).filter(|n| *n > 0) {
                    which = Some(Which::Nth(n as usize));
                    i += 2;
                    continue;
                }
                keep.push(w[i].clone());
            } else {
                keep.push(w[i].clone());
            }
            i += 1;
        }
        // Filler around the target.
        while keep.first().is_some_and(|w| {
            matches!(
                w.as_str(),
                "the" | "a" | "an" | "on" | "at" | "that" | "this"
            )
        }) {
            keep.remove(0);
        }
        while keep
            .last()
            .is_some_and(|w| matches!(w.as_str(), "one" | "the" | "on" | "at" | "side" | "please"))
        {
            keep.pop();
        }
        Request {
            action,
            which,
            target: keep.join(" "),
        }
    }
}

/// One SAM instance on the screen.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub score: f32,
    /// Mask at SAM's image size, row-major.
    pub mask: Mask,
    /// Bounding box in mask pixels.
    pub bbox: [usize; 4],
    /// Point deepest inside the mask, in mask pixels.
    pub point: (usize, usize),
}

#[derive(Debug, Clone)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub px: Vec<bool>,
}

impl Mask {
    pub fn area(&self) -> usize {
        self.px.iter().filter(|p| **p).count()
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.px[y * self.w + x]
    }
}

/// COCO RLE (pycocotools `rleFrString`), column-major, `size` is [h, w].
pub fn rle_decode(counts: &str, h: usize, w: usize) -> Result<Mask> {
    let s = counts.as_bytes();
    let mut cnts: Vec<i64> = Vec::new();
    let mut p = 0;
    while p < s.len() {
        let (mut x, mut k, mut more) = (0i64, 0, true);
        while more {
            let c = *s.get(p).context("truncated RLE")? as i64 - 48;
            x |= (c & 0x1f) << (5 * k);
            more = c & 0x20 != 0;
            p += 1;
            k += 1;
            if !more && c & 0x10 != 0 {
                x |= -1i64 << (5 * k);
            }
        }
        if cnts.len() > 2 {
            x += cnts[cnts.len() - 2];
        }
        cnts.push(x);
    }
    let mut col = vec![false; h * w];
    let mut i = 0usize;
    for (j, n) in cnts.iter().enumerate() {
        let n = usize::try_from(*n).context("negative RLE run")?;
        if j % 2 == 1 {
            let end = (i + n).min(h * w);
            col[i..end].iter_mut().for_each(|v| *v = true);
        }
        i += n;
    }
    // Column-major to row-major.
    let mut px = vec![false; h * w];
    for x in 0..w {
        for y in 0..h {
            px[y * w + x] = col[x * h + y];
        }
    }
    Ok(Mask { w, h, px })
}

/// Bounding box and the point farthest from the mask's edge (a chamfer
/// distance transform): the centroid of a ring or an L can fall outside.
fn shape(mask: &Mask) -> Option<([usize; 4], (usize, usize))> {
    let (w, h) = (mask.w, mask.h);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if mask.at(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return None;
    }
    // Distances inside the box only, with a one-pixel border of background.
    let (bw, bh) = (x1 - x0 + 3, y1 - y0 + 3);
    const INF: u32 = u32::MAX / 4;
    let mut d = vec![0u32; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            let inside =
                x >= 1 && y >= 1 && x < bw - 1 && y < bh - 1 && mask.at(x0 + x - 1, y0 + y - 1);
            d[y * bw + x] = if inside { INF } else { 0 };
        }
    }
    for y in 1..bh - 1 {
        for x in 1..bw - 1 {
            let i = y * bw + x;
            if d[i] == 0 {
                continue;
            }
            let v = [
                d[i - 1] + 3,
                d[i - bw] + 3,
                d[i - bw - 1] + 4,
                d[i - bw + 1] + 4,
            ];
            d[i] = d[i].min(*v.iter().min().unwrap_or(&INF));
        }
    }
    let mut best = (0u32, (x0, y0));
    for y in (1..bh - 1).rev() {
        for x in (1..bw - 1).rev() {
            let i = y * bw + x;
            if d[i] == 0 {
                continue;
            }
            let v = [
                d[i + 1] + 3,
                d[i + bw] + 3,
                d[i + bw + 1] + 4,
                d[i + bw - 1] + 4,
            ];
            d[i] = d[i].min(*v.iter().min().unwrap_or(&INF));
            if d[i] >= best.0 {
                best = (d[i], (x0 + x - 1, y0 + y - 1));
            }
        }
    }
    Some(([x0, y0, x1 - x0 + 1, y1 - y0 + 1], best.1))
}

#[derive(Deserialize)]
struct SegmentReply {
    instances: Vec<Instance>,
    image_size: [usize; 2],
}

#[derive(Deserialize)]
struct Instance {
    mask_rle: Rle,
    score: f32,
}

#[derive(Deserialize)]
struct Rle {
    counts: String,
    size: [usize; 2],
}

/// Everything SAM found for `text` on `jpeg`, best first, with masks.
pub async fn segment(
    server: &str,
    jpeg: &[u8],
    text: &str,
) -> Result<(Vec<Candidate>, [usize; 2])> {
    let url = format!("{}/segment", server.trim_end_matches('/'));
    let body = serde_json::json!({"image_b64": BASE64.encode(jpeg), "prompt": {"text": text}});
    let reply = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if reply.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        bail!("SAM is still loading ({url})");
    }
    if !reply.status().is_success() {
        bail!("SAM: {} from {url}", reply.status());
    }
    let reply: SegmentReply = reply.json().await.context("SAM reply")?;
    let mut out = Vec::new();
    for inst in reply.instances {
        let [h, w] = inst.mask_rle.size;
        let mask = rle_decode(&inst.mask_rle.counts, h, w)?;
        if let Some((bbox, point)) = shape(&mask) {
            out.push(Candidate {
                score: inst.score,
                mask,
                bbox,
                point,
            });
        }
    }
    Ok((out, reply.image_size))
}

/// Drop masks that cannot be a click target (specks, half the screen),
/// apply the position word, keep the best few.
pub fn rank(mut cands: Vec<Candidate>, which: Option<Which>) -> Vec<Candidate> {
    if let Some(first) = cands.first() {
        let screen = (first.mask.w * first.mask.h) as f64;
        cands.retain(|c| {
            let a = (c.bbox[2] * c.bbox[3]) as f64;
            a >= 36.0 && a <= 0.4 * screen
        });
    }
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    let centre = |c: &Candidate| {
        (
            c.bbox[0] as f64 + c.bbox[2] as f64 / 2.0,
            c.bbox[1] as f64 + c.bbox[3] as f64 / 2.0,
        )
    };
    // Reading order: rows (by top edge, a third of a row's height apart), then left to right.
    let reading = |v: &mut Vec<Candidate>| {
        v.sort_by(|a, b| {
            let row = (a.bbox[3].min(b.bbox[3]) as f64 / 3.0).max(4.0);
            let (ay, by) = (a.bbox[1] as f64, b.bbox[1] as f64);
            if (ay - by).abs() > row {
                ay.total_cmp(&by)
            } else {
                a.bbox[0].cmp(&b.bbox[0])
            }
        })
    };
    match which {
        None => {}
        Some(Which::Nth(n)) => {
            reading(&mut cands);
            if n >= 1 && n <= cands.len() {
                let c = cands.remove(n - 1);
                cands.insert(0, c);
            }
        }
        Some(Which::Last) => {
            reading(&mut cands);
            if let Some(c) = cands.pop() {
                cands.insert(0, c);
            }
        }
        Some(w) => {
            let key = |c: &Candidate| {
                let (x, y) = centre(c);
                match w {
                    Which::Top => y,
                    Which::Bottom => -y,
                    Which::Left => x,
                    _ => -x,
                }
            };
            cands.sort_by(|a, b| key(a).total_cmp(&key(b)));
        }
    }
    cands.truncate(MAX_CANDIDATES);
    cands
}

/// A candidate as the overlay and the pointer see it: output-local logical px.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Shown {
    pub n: usize,
    pub score: f32,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Where an action lands.
    pub point: [f64; 2],
}

/// `vision.json`: what the overlay draws.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub id: String,
    /// "looking", "pick", "acting", "none" (nothing found), "warming"
    pub state: String,
    pub prompt: String,
    pub output: String,
    /// Logical size of the output.
    pub width: f64,
    pub height: f64,
    /// Mask overlay (same aspect as the output), empty when none.
    pub image: String,
    pub picked: usize,
    pub candidates: Vec<Shown>,
    /// Shown under the outlines, like a decision's question.
    pub hint: String,
}

pub fn shown(
    cands: &[Candidate],
    image_size: [usize; 2],
    output: &Output,
    label: &str,
) -> Vec<Shown> {
    let sx = output.width / image_size[0].max(1) as f64;
    let sy = output.height / image_size[1].max(1) as f64;
    cands
        .iter()
        .enumerate()
        .map(|(i, c)| Shown {
            n: i + 1,
            score: c.score,
            label: label.to_string(),
            x: c.bbox[0] as f64 * sx,
            y: c.bbox[1] as f64 * sy,
            w: c.bbox[2] as f64 * sx,
            h: c.bbox[3] as f64 * sy,
            point: [(c.point.0 as f64 + 0.5) * sx, (c.point.1 as f64 + 0.5) * sy],
        })
        .collect()
}

/// The overlay image: the picked candidate filled and outlined in pink, the
/// others outlined faintly.
pub fn render(cands: &[Candidate], picked: usize, path: &Path) -> Result<()> {
    let Some(first) = cands.first() else {
        return Ok(());
    };
    let (w, h) = (first.mask.w, first.mask.h);
    let mut rgba = vec![0u8; w * h * 4];
    // Others first, so the picked one is drawn over them.
    let order = (0..cands.len()).filter(|i| *i != picked).chain([picked]);
    for i in order {
        let Some(c) = cands.get(i) else { continue };
        let m = &c.mask;
        let on = |x: isize, y: isize| {
            x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m.at(x as usize, y as usize)
        };
        let [bx, by, bw, bh] = c.bbox;
        for y in by.saturating_sub(2)..(by + bh + 2).min(h) {
            for x in bx.saturating_sub(2)..(bx + bw + 2).min(w) {
                let (xi, yi) = (x as isize, y as isize);
                let inside = on(xi, yi);
                // Edge: within 2 px outside the mask.
                let edge = !inside
                    && (-2..=2)
                        .any(|dy| (-2..=2).any(|dx| (dx != 0 || dy != 0) && on(xi + dx, yi + dy)));
                let alpha = match (i == picked, inside, edge) {
                    (true, true, _) => 70,
                    (true, false, true) => 255,
                    (false, false, true) => 150,
                    _ => continue,
                };
                let p = (y * w + x) * 4;
                rgba[p..p + 4].copy_from_slice(&[PINK[0], PINK[1], PINK[2], alpha]);
            }
        }
    }
    let tmp = path.with_extension("png.tmp");
    {
        let file = std::fs::File::create(&tmp)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header()?.write_image_data(&rgba)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn write_state(state_dir: &Path, state: &State) -> Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(state)?)?;
    std::fs::rename(&tmp, state_dir.join(FILE))?;
    Ok(())
}

pub fn clear(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(FILE));
    let _ = std::fs::remove_file(state_dir.join(IMAGE));
}

/// Do `action` at a shown candidate.
pub fn act(compositor: Compositor, output: &Output, target: &Shown, action: Action) -> Result<()> {
    let (x, y) = (output.x + target.point[0], output.y + target.point[1]);
    pointer::move_to(compositor, x, y)?;
    // Let the app see the pointer arrive before the press (hover states).
    std::thread::sleep(Duration::from_millis(80));
    match action {
        Action::Outline | Action::Move => {}
        Action::Click(b) => pointer::click(compositor, b.into())?,
        Action::DoubleClick => {
            pointer::click(compositor, Button::Left)?;
            std::thread::sleep(Duration::from_millis(60));
            pointer::click(compositor, Button::Left)?;
        }
        Action::Scroll(n) => pointer::scroll(compositor, n)?,
    }
    Ok(())
}

/// A look's working files.
pub fn image_path(state_dir: &Path) -> PathBuf {
    state_dir.join(IMAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        let r = Request::parse("Right click the blue folder.");
        assert_eq!(r.action, Action::Click(ButtonName::Right));
        assert_eq!(r.target, "blue folder");
        let r = Request::parse("the second send button");
        assert_eq!((r.action, r.which), (Action::Outline, Some(Which::Nth(2))));
        assert_eq!(r.target, "send button");
        let r = Request::parse("click the button on the left");
        assert_eq!(r.which, Some(Which::Left));
        assert_eq!(r.target, "button");
        let r = Request::parse("the left arrow icon");
        assert_eq!(r.which, Some(Which::Left));
        let r = Request::parse("close icon number three");
        assert_eq!(
            (r.which, r.target.as_str()),
            (Some(Which::Nth(3)), "close icon")
        );
        let r = Request::parse("search field");
        assert_eq!(
            (r.action, r.which, r.target.as_str()),
            (Action::Outline, None, "search field")
        );
    }

    #[test]
    fn parses_scrolls() {
        assert_eq!(parse_scroll("up twenty"), Some(-20));
        assert_eq!(parse_scroll("Down five."), Some(5));
        assert_eq!(parse_scroll("scroll down 3"), Some(3));
        assert_eq!(parse_scroll("down"), Some(5));
        assert_eq!(parse_scroll("download button"), None);
        assert_eq!(parse_scroll("up the hill"), None);
    }

    #[test]
    fn decodes_coco_rle() {
        // A 3x4 mask with a 2x2 block at rows 1-2, cols 1-2. Column-major
        // runs: 3+1 zeros, 2 ones, 1 zero, 2 ones, 3 zeros.
        let m = rle_decode(&encode(&[4, 2, 1, 2, 3]), 3, 4).unwrap();
        let rows: Vec<String> = (0..3)
            .map(|y| (0..4).map(|x| if m.at(x, y) { '#' } else { '.' }).collect())
            .collect();
        assert_eq!(rows, ["....", ".##.", ".##."]);
        let (bbox, point) = shape(&m).unwrap();
        assert_eq!(bbox, [1, 1, 2, 2]);
        assert!(m.at(point.0, point.1));
    }

    /// pycocotools `rleToString`, for the test.
    fn encode(cnts: &[i64]) -> String {
        let mut s = String::new();
        for (i, &c) in cnts.iter().enumerate() {
            let mut x = if i > 2 { c - cnts[i - 2] } else { c };
            loop {
                let mut c = x & 0x1f;
                x >>= 5;
                let more = if c & 0x10 != 0 { x != -1 } else { x != 0 };
                if more {
                    c |= 0x20;
                }
                s.push((c as u8 + 48) as char);
                if !more {
                    break;
                }
            }
        }
        s
    }

    #[test]
    fn deepest_point_of_an_l_is_inside() {
        let (w, h) = (20, 20);
        let mut px = vec![false; w * h];
        for y in 0..20 {
            for x in 0..20 {
                px[y * w + x] = x < 5 || y >= 15;
            }
        }
        let m = Mask { w, h, px };
        let (_, (x, y)) = shape(&m).unwrap();
        assert!(m.at(x, y));
    }
}

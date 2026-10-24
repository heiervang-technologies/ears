//! Command mode: speech routed to actions by embedding, not transcription.
//!
//! Saying "command" on its own switches VAD into command mode (the ears
//! light turns violet). From then on an utterance is not transcribed: its raw
//! audio is embedded with EmbeddingGemma 2 (the cluster's `embeddinggemma2`,
//! a llama-server with the audio encoder) and compared with the anchors of
//! every action. Anchors are examples of what an action means: the text
//! descriptions in `actions.toml`, and recordings of your own voice, added by
//! `ears teach ACTION` or automatically each time an action fires. The k
//! nearest anchors vote; the winner fires only if its nearest anchor is
//! close enough (`floor`) and clearly closer than any other action's
//! (`margin`). Similarities are z-scored per anchor kind first (see
//! [`Router`]). Everything else is the null action: nothing happens.
//!
//! Design note: wiki "Embedding-Driven Voice Control".
//!
//! Built-in actions, always present: `leave` (back to dictation; "command"
//! again), `none` (ordinary chatter, so it has somewhere to land) and
//! `wrong` (forget the voice anchor the last firing added).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Marker in the runtime dir while command mode is on (read by the light).
pub const MARKER: &str = "vad.command";
/// `ears teach ACTION` writes the action's name here; the next command-mode
/// utterance becomes one of its voice anchors.
pub const TEACH: &str = "command.teach";
/// Last routing decision, for displays.
pub const LAST: &str = "command-last.json";

/// Spoken alone, in dictation or standby, these switch command mode on.
/// "commands": how the ASR model often hears a lone "command".
pub const ENTER_PHRASES: &[&str] = &["command", "commands", "command mode"];

/// Voice anchors kept per action; the oldest automatic ones go first.
const MAX_VOICE_ANCHORS: usize = 24;
/// Text inputs get the same task prefix on both sides (symmetric task).
const TEXT_PREFIX: &str = "task: sentence similarity | query: ";

const DEFAULT_ACTIONS: &str = include_str!("../contrib/actions.toml");

#[derive(Debug, thiserror::Error)]
pub enum CommandModeError {
    #[error("embedding server: {0}")]
    Embed(String),
    #[error("actions file {0}: {1}")]
    Actions(PathBuf, String),
}

type Result<T> = std::result::Result<T, CommandModeError>;

/// `actions.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ActionsFile {
    /// EmbeddingGemma 2 llama-server.
    #[serde(default = "default_server")]
    pub server: String,
    /// The winner's nearest anchor must score at least this (z-score: how
    /// much closer than the anchors of its kind usually are).
    #[serde(default = "default_floor")]
    pub floor: f32,
    /// ...and beat every other action's nearest anchor by this much.
    #[serde(default = "default_margin")]
    pub margin: f32,
    /// Neighbours that vote.
    #[serde(default = "default_k")]
    pub k: usize,
    #[serde(default, rename = "action")]
    pub actions: Vec<ActionSpec>,
}

fn default_server() -> String {
    "http://localhost:30201".into()
}
fn default_floor() -> f32 {
    2.0
}
fn default_margin() -> f32 {
    0.4
}
fn default_k() -> usize {
    5
}

/// One action: what it means (`text`) and what it does (one of `keys`,
/// `run`).
#[derive(Debug, Clone, Deserialize)]
pub struct ActionSpec {
    pub name: String,
    /// Descriptions and phrasings; each is a text anchor.
    #[serde(default)]
    pub text: Vec<String>,
    /// Key combination, as in `[commands.keys]`: "enter", "ctrl+c".
    pub keys: Option<String>,
    /// Program and arguments to start.
    pub run: Option<Vec<String>>,
}

/// What firing an action does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Keys(String),
    Run(Vec<String>),
    /// Back to dictation.
    Leave,
    /// The null action: chatter.
    Nothing,
    /// Forget the voice anchor the last firing added.
    Wrong,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub effect: Effect,
    pub text: Vec<String>,
}

fn builtins() -> Vec<Action> {
    let a = |name: &str, effect, text: &[&str]| Action {
        name: name.into(),
        effect,
        text: text.iter().map(|t| t.to_string()).collect(),
    };
    vec![
        a(
            "leave",
            Effect::Leave,
            &["command", "command mode", "exit command mode"],
        ),
        a(
            "none",
            Effect::Nothing,
            &[
                "what a nice day today",
                "hmm let me think",
                "okay so anyway",
                "I was just talking to someone else",
                "can you hear me",
                "yeah I know what you mean",
            ],
        ),
        a(
            "wrong",
            Effect::Wrong,
            &[
                "wrong",
                "no that was wrong",
                "not that one",
                "that's not what I meant",
            ],
        ),
    ]
}

/// Load `path`, writing the shipped defaults there first if it is missing.
pub fn load_actions(path: &Path) -> Result<(ActionsFile, Vec<Action>)> {
    let err = |e: String| CommandModeError::Actions(path.to_path_buf(), e);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err(e.to_string()))?;
        }
        std::fs::write(path, DEFAULT_ACTIONS).map_err(|e| err(e.to_string()))?;
    }
    let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
    parse_actions(&text).map_err(err)
}

pub fn parse_actions(text: &str) -> std::result::Result<(ActionsFile, Vec<Action>), String> {
    let file: ActionsFile = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut actions = builtins();
    for spec in &file.actions {
        let effect = match (&spec.keys, &spec.run) {
            (Some(keys), None) => {
                if crate::commands::keycodes(keys).is_none() {
                    return Err(format!("action {}: unknown keys {keys:?}", spec.name));
                }
                Effect::Keys(keys.clone())
            }
            (None, Some(run)) if !run.is_empty() => Effect::Run(run.clone()),
            _ => {
                return Err(format!(
                    "action {}: needs exactly one of `keys` or `run`",
                    spec.name
                ))
            }
        };
        let action = Action {
            name: spec.name.clone(),
            effect,
            text: spec.text.clone(),
        };
        // A file action replaces a built-in of the same name.
        match actions.iter_mut().find(|a| a.name == spec.name) {
            Some(existing) => *existing = action,
            None => actions.push(action),
        }
    }
    Ok((file, actions))
}

/// A voice anchor on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceAnchor {
    pub action: String,
    /// "teach" (labelled by you) or "auto" (an action that fired).
    pub source: String,
    pub vec: Vec<f32>,
}

/// All voice anchors, in `command-anchors.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VoiceAnchors {
    pub anchors: Vec<VoiceAnchor>,
}

impl VoiceAnchors {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Add one, dropping the action's oldest automatic anchor (else its
    /// oldest) beyond the cap.
    pub fn add(&mut self, anchor: VoiceAnchor) {
        let action = anchor.action.clone();
        self.anchors.push(anchor);
        let count = self.anchors.iter().filter(|a| a.action == action).count();
        if count > MAX_VOICE_ANCHORS {
            let victim = self
                .anchors
                .iter()
                .position(|a| a.action == action && a.source == "auto")
                .or_else(|| self.anchors.iter().position(|a| a.action == action));
            if let Some(i) = victim {
                self.anchors.remove(i);
            }
        }
    }
}

/// Unit-length copy.
pub fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Anchors of one kind below this count are not scored: their baseline
/// cannot be estimated.
const MIN_KIND: usize = 4;

/// Anchors and the nearest-neighbour vote.
///
/// Raw cosine values are not comparable across anchor kinds: speech against
/// text sits in one band, speech against speech in a higher one, and
/// against recordings of the same voice everything looks close (who speaks
/// outweighs what is said, so one recording became a hub that attracted
/// every utterance). Each kind is therefore scored as a z-score against its
/// own anchors (how much closer than usual), and only then compared.
pub struct Router {
    pub actions: Vec<Action>,
    /// (action index, unit vector, is a voice anchor).
    anchors: Vec<(usize, Vec<f32>, bool)>,
    /// Fire only if the winner's nearest anchor scores at least this (z).
    pub floor: f32,
    /// ...and beats every other action's nearest anchor by this much (z).
    pub margin: f32,
    /// Nearest anchors that vote.
    pub k: usize,
}

/// One routing decision.
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    /// Each action's share of the k-nearest vote, highest first.
    pub votes: Vec<(String, f32)>,
    /// The winner's nearest anchor (z-score).
    pub nearest: f32,
    /// How far it beats the runner-up's nearest anchor (z).
    pub margin: f32,
    /// The action to fire, if any (never the null action).
    #[serde(skip)]
    pub fire: Option<usize>,
    pub fired: Option<String>,
}

impl Router {
    pub fn new(actions: Vec<Action>, floor: f32, margin: f32, k: usize) -> Self {
        Self {
            actions,
            anchors: Vec::new(),
            floor,
            margin,
            k: k.max(1),
        }
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.actions.iter().position(|a| a.name == name)
    }

    pub fn add_anchor(&mut self, action: usize, vec: Vec<f32>, voice: bool) {
        self.anchors.push((action, normalized(vec), voice));
    }

    pub fn remove_anchor(&mut self, action: usize, vec: &[f32]) {
        if let Some(i) = self
            .anchors
            .iter()
            .position(|(a, v, voice)| *a == action && *voice && v.as_slice() == vec)
        {
            self.anchors.remove(i);
        }
    }

    /// (z-score, action) of every scorable anchor.
    fn scored(&self, utterance: &[f32]) -> Vec<(f32, usize)> {
        let mut out = Vec::with_capacity(self.anchors.len());
        for kind in [false, true] {
            let sims: Vec<(f32, usize)> = self
                .anchors
                .iter()
                .filter(|(_, _, voice)| *voice == kind)
                .map(|(a, v, _)| (dot(utterance, v), *a))
                .collect();
            if sims.len() < MIN_KIND {
                continue;
            }
            let n = sims.len() as f32;
            let mean = sims.iter().map(|s| s.0).sum::<f32>() / n;
            let sd = (sims.iter().map(|s| (s.0 - mean).powi(2)).sum::<f32>() / n).sqrt();
            let sd = if sd > 1e-6 { sd } else { 1.0 };
            out.extend(sims.into_iter().map(|(s, a)| ((s - mean) / sd, a)));
        }
        out
    }

    /// The k nearest anchors vote (weighted by score); the winner fires if
    /// its nearest anchor clears `floor` and beats the runner-up by `margin`.
    pub fn decide(&self, utterance: &[f32]) -> Decision {
        let mut scored = self.scored(utterance);
        scored.sort_by(|x, y| y.0.total_cmp(&x.0));
        let mut best = vec![f32::NEG_INFINITY; self.actions.len()];
        for &(z, a) in &scored {
            best[a] = best[a].max(z);
        }
        let mut votes = vec![0.0f32; self.actions.len()];
        for &(z, a) in scored.iter().take(self.k) {
            votes[a] += z.max(0.0);
        }
        let total: f32 = votes.iter().sum();
        let mut ranked: Vec<(usize, f32)> = votes
            .iter()
            .enumerate()
            .map(|(i, v)| (i, if total > 0.0 { v / total } else { 0.0 }))
            .collect();
        ranked.sort_by(|x, y| y.1.total_cmp(&x.1));
        let winner = ranked.first().filter(|r| r.1 > 0.0).map(|r| r.0);
        let (nearest, margin) = winner.map_or((0.0, 0.0), |w| {
            let runner_up = best
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != w)
                .map(|(_, z)| *z)
                .fold(f32::NEG_INFINITY, f32::max);
            let margin = if runner_up.is_finite() {
                best[w] - runner_up
            } else {
                best[w]
            };
            (best[w], margin)
        });
        let fire = winner.filter(|&w| {
            nearest >= self.floor
                && margin >= self.margin
                && self.actions[w].effect != Effect::Nothing
        });
        Decision {
            votes: ranked
                .iter()
                .filter(|(_, s)| *s > 0.0)
                .map(|&(i, s)| (self.actions[i].name.clone(), s))
                .collect(),
            nearest,
            margin,
            fire,
            fired: fire.map(|i| self.actions[i].name.clone()),
        }
    }
}

/// EmbeddingGemma 2 on llama-server: `/embeddings`, audio as a media part.
#[derive(Clone)]
pub struct Embedder {
    http: reqwest::Client,
    url: String,
    marker: std::sync::Arc<tokio::sync::OnceCell<String>>,
}

impl Embedder {
    pub fn new(server: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            url: server.trim_end_matches('/').to_string(),
            marker: Default::default(),
        }
    }

    async fn marker(&self) -> Result<&str> {
        self.marker
            .get_or_try_init(|| async {
                let props: serde_json::Value = self
                    .http
                    .get(format!("{}/props", self.url))
                    .send()
                    .await
                    .map_err(|e| CommandModeError::Embed(e.to_string()))?
                    .json()
                    .await
                    .map_err(|e| CommandModeError::Embed(e.to_string()))?;
                Ok(props["media_marker"]
                    .as_str()
                    .unwrap_or("<__media__>")
                    .to_string())
            })
            .await
            .map(String::as_str)
    }

    async fn embed(&self, content: serde_json::Value) -> Result<Vec<f32>> {
        let response = self
            .http
            .post(format!("{}/embeddings", self.url))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| CommandModeError::Embed(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CommandModeError::Embed(format!(
                "{status}: {}",
                crate::continuous::truncate(&body, 200)
            )));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| CommandModeError::Embed(e.to_string()))?;
        parse_embedding(&value)
            .map(normalized)
            .ok_or_else(|| CommandModeError::Embed("no embedding in the reply".into()))
    }

    pub async fn text(&self, text: &str) -> Result<Vec<f32>> {
        self.embed(serde_json::Value::String(format!("{TEXT_PREFIX}{text}")))
            .await
    }

    /// A 16 kHz mono WAV file's bytes.
    pub async fn audio(&self, wav: &[u8]) -> Result<Vec<f32>> {
        use base64::Engine;
        let marker = self.marker().await?.to_string();
        let data = base64::engine::general_purpose::STANDARD.encode(wav);
        self.embed(serde_json::json!({"prompt_string": marker, "multimodal_data": [data]}))
            .await
    }
}

/// `[{"embedding": [..]}]` or `[{"embedding": [[..]]}]` (llama-server), or
/// the OpenAI `{"data": [{"embedding": [..]}]}`.
fn parse_embedding(value: &serde_json::Value) -> Option<Vec<f32>> {
    let first = value
        .get(0)
        .or_else(|| value.get("data").and_then(|d| d.get(0)))?;
    let mut e = first.get("embedding")?;
    if e.get(0).is_some_and(|x| x.is_array()) {
        e = e.get(0)?;
    }
    e.as_array()?
        .iter()
        .map(|x| x.as_f64().map(|f| f as f32))
        .collect()
}

/// Text anchor vectors, cached on disk by text so entering the mode costs
/// one embedding call per new phrasing, not per anchor.
#[derive(Default, Serialize, Deserialize)]
struct TextCache {
    server: String,
    vectors: std::collections::HashMap<String, Vec<f32>>,
}

/// Where command mode keeps things.
#[derive(Debug, Clone)]
pub struct Paths {
    pub actions: PathBuf,
    pub voice: PathBuf,
    pub text_cache: PathBuf,
    pub state_dir: PathBuf,
}

impl Paths {
    pub fn new(state_dir: &Path) -> Self {
        let dirs = directories::ProjectDirs::from("com", "heiervang", "ears");
        let config = dirs
            .as_ref()
            .map_or_else(|| state_dir.to_path_buf(), |d| d.config_dir().to_path_buf());
        let data = dirs
            .as_ref()
            .map_or_else(|| state_dir.to_path_buf(), |d| d.data_dir().to_path_buf());
        let cache = dirs
            .as_ref()
            .map_or_else(|| state_dir.to_path_buf(), |d| d.cache_dir().to_path_buf());
        Self {
            actions: config.join("actions.toml"),
            voice: data.join("command-anchors.json"),
            text_cache: cache.join("command-text-anchors.json"),
            state_dir: state_dir.to_path_buf(),
        }
    }
}

/// Command mode while it is on.
pub struct CommandMode {
    pub router: Router,
    pub embedder: Embedder,
    pub paths: Paths,
    voice: VoiceAnchors,
    /// The voice anchor the last firing added, for `wrong`.
    last_auto: Option<VoiceAnchor>,
}

/// What an utterance in command mode came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Fire this action.
    Fire { name: String, effect: Effect },
    /// Labelled by `ears teach`: stored, not fired.
    Taught(String),
    /// The last automatic anchor was forgotten.
    Forgot(Option<String>),
    /// Nothing confident enough (or chatter).
    Nothing,
}

impl CommandMode {
    /// Load actions and anchors, embedding text anchors not in the cache.
    pub async fn start(paths: Paths) -> Result<Self> {
        let (file, actions) = load_actions(&paths.actions)?;
        let embedder = Embedder::new(&file.server);
        let mut router = Router::new(actions, file.floor, file.margin, file.k);
        let mut cache: TextCache = std::fs::read_to_string(&paths.text_cache)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .filter(|c: &TextCache| c.server == file.server)
            .unwrap_or_else(|| TextCache {
                server: file.server.clone(),
                ..Default::default()
            });
        let mut dirty = false;
        for i in 0..router.actions.len() {
            for text in router.actions[i].text.clone() {
                let vec = match cache.vectors.get(&text) {
                    Some(v) => v.clone(),
                    None => {
                        let v = embedder.text(&text).await?;
                        cache.vectors.insert(text.clone(), v.clone());
                        dirty = true;
                        v
                    }
                };
                router.add_anchor(i, vec, false);
            }
        }
        if dirty {
            if let Some(dir) = paths.text_cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(bytes) = serde_json::to_vec(&cache) {
                let _ = std::fs::write(&paths.text_cache, bytes);
            }
        }
        let voice = VoiceAnchors::load(&paths.voice);
        for anchor in &voice.anchors {
            if let Some(i) = router.index(&anchor.action) {
                router.add_anchor(i, anchor.vec.clone(), true);
            }
        }
        Ok(Self {
            router,
            embedder,
            paths,
            voice,
            last_auto: None,
        })
    }

    /// Route one utterance (16 kHz mono WAV bytes).
    pub async fn hear(&mut self, wav: &[u8]) -> Result<(Outcome, Decision)> {
        let u = self.embedder.audio(wav).await?;
        let decision = self.router.decide(&u);
        let _ = std::fs::write(
            self.paths.state_dir.join(LAST),
            serde_json::to_vec(&decision).unwrap_or_default(),
        );
        // A label from `ears teach` wins over the vote.
        let teach = self.paths.state_dir.join(TEACH);
        if let Ok(name) = std::fs::read_to_string(&teach) {
            let _ = std::fs::remove_file(&teach);
            let name = name.trim().to_string();
            if let Some(i) = self.router.index(&name) {
                self.remember(i, u, "teach");
                return Ok((Outcome::Taught(name), decision));
            }
        }
        let Some(i) = decision.fire else {
            return Ok((Outcome::Nothing, decision));
        };
        let action = self.router.actions[i].clone();
        let outcome = match action.effect {
            Effect::Wrong => {
                let forgotten = self.last_auto.take().map(|anchor| {
                    if let Some(a) = self.router.index(&anchor.action) {
                        self.router.remove_anchor(a, &anchor.vec);
                    }
                    self.voice.anchors.retain(|x| x != &anchor);
                    self.save();
                    anchor.action
                });
                Outcome::Forgot(forgotten)
            }
            Effect::Nothing => Outcome::Nothing,
            effect => {
                // Few-shot calibration: a clear firing teaches its own
                // phrasing. A narrow one might be a misroute nobody undid.
                if decision.margin >= 2.0 * self.router.margin {
                    self.remember(i, u, "auto");
                }
                Outcome::Fire {
                    name: action.name,
                    effect,
                }
            }
        };
        Ok((outcome, decision))
    }

    fn remember(&mut self, action: usize, vec: Vec<f32>, source: &str) {
        let anchor = VoiceAnchor {
            action: self.router.actions[action].name.clone(),
            source: source.into(),
            vec: vec.clone(),
        };
        self.router.add_anchor(action, vec, true);
        if source == "auto" {
            self.last_auto = Some(anchor.clone());
        }
        self.voice.add(anchor);
        self.save();
    }

    fn save(&self) {
        if let Err(e) = self.voice.save(&self.paths.voice) {
            tracing::warn!("Cannot save command anchors: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(x: &[f32]) -> Vec<f32> {
        normalized(x.to_vec())
    }

    fn router() -> Router {
        let (_, actions) = parse_actions(
            r#"
[[action]]
name = "enter"
keys = "enter"
text = ["press enter"]
[[action]]
name = "next"
run = ["playerctl", "next"]
"#,
        )
        .unwrap();
        let mut r = Router::new(actions, 1.0, 0.4, 3);
        let id = |r: &Router, n: &str| r.index(n).unwrap();
        let (enter, next, none, leave) = (
            id(&r, "enter"),
            id(&r, "next"),
            id(&r, "none"),
            id(&r, "leave"),
        );
        // Text anchors on distinct axes.
        r.add_anchor(enter, unit(&[1.0, 0.0, 0.0, 0.0, 0.0]), false);
        r.add_anchor(next, unit(&[0.0, 1.0, 0.0, 0.0, 0.0]), false);
        r.add_anchor(none, unit(&[0.0, 0.0, 1.0, 0.0, 0.0]), false);
        r.add_anchor(leave, unit(&[0.0, 0.0, 0.0, 1.0, 0.0]), false);
        r
    }

    #[test]
    fn shipped_actions_parse() {
        let (file, actions) = parse_actions(DEFAULT_ACTIONS).unwrap();
        assert!(file.floor > 0.0 && file.margin > 0.0 && file.k > 0);
        for builtin in ["leave", "none", "wrong"] {
            assert!(actions.iter().any(|a| a.name == builtin));
        }
        assert!(actions.len() > 8);
        assert!(actions.iter().all(|a| !a.text.is_empty()));
    }

    #[test]
    fn actions_need_one_effect_and_known_keys() {
        assert!(parse_actions("[[action]]\nname = \"x\"\n").is_err());
        assert!(parse_actions("[[action]]\nname = \"x\"\nkeys = \"hyperspace\"\n").is_err());
        let both = "[[action]]\nname = \"x\"\nkeys = \"enter\"\nrun = [\"true\"]\n";
        assert!(parse_actions(both).is_err());
    }

    #[test]
    fn nearest_anchor_fires_when_clear() {
        let r = router();
        let d = r.decide(&unit(&[1.0, 0.1, 0.0, 0.0, 0.0]));
        assert_eq!(d.fired.as_deref(), Some("enter"));
        assert_eq!(d.votes[0].0, "enter");
        assert_eq!(
            r.decide(&unit(&[0.1, 1.0, 0.0, 0.0, 0.0])).fired.as_deref(),
            Some("next")
        );
    }

    #[test]
    fn chatter_and_coin_tosses_fire_nothing() {
        let r = router();
        // Nearest is the null action.
        assert_eq!(r.decide(&unit(&[0.0, 0.1, 1.0, 0.0, 0.0])).fired, None);
        // Equally close to two actions: no margin.
        let toss = r.decide(&unit(&[1.0, 1.0, 0.0, 0.0, 0.0]));
        assert!(toss.margin < 0.4);
        assert_eq!(toss.fired, None);
        // Close to nothing: every anchor alike, nothing stands out.
        assert_eq!(r.decide(&unit(&[0.0, 0.0, 0.0, 0.0, 1.0])).fired, None);
    }

    #[test]
    fn a_voice_that_sounds_like_every_recording_is_no_hub() {
        let mut r = router();
        let enter = r.index("enter").unwrap();
        let next = r.index("next").unwrap();
        let leave = r.index("leave").unwrap();
        // Recordings share a strong "same speaker" component (axis 4) and
        // differ a little in content.
        r.add_anchor(leave, unit(&[0.0, 0.0, 0.0, 0.3, 1.0]), true);
        r.add_anchor(enter, unit(&[0.3, 0.0, 0.0, 0.0, 1.0]), true);
        r.add_anchor(next, unit(&[0.0, 0.3, 0.0, 0.0, 1.0]), true);
        r.add_anchor(enter, unit(&[0.35, 0.0, 0.0, 0.0, 1.0]), true);
        // The same voice saying "next": raw cosine is high to every
        // recording, but only the content difference counts.
        let d = r.decide(&unit(&[0.0, 0.5, 0.0, 0.0, 1.0]));
        assert_eq!(d.fired.as_deref(), Some("next"), "{d:?}");
    }

    #[test]
    fn voice_anchors_are_capped_per_action_oldest_auto_first() {
        let mut v = VoiceAnchors::default();
        v.add(VoiceAnchor {
            action: "enter".into(),
            source: "teach".into(),
            vec: vec![9.0],
        });
        for i in 0..MAX_VOICE_ANCHORS + 3 {
            v.add(VoiceAnchor {
                action: "enter".into(),
                source: "auto".into(),
                vec: vec![i as f32],
            });
        }
        assert_eq!(v.anchors.len(), MAX_VOICE_ANCHORS);
        assert!(v.anchors.iter().any(|a| a.source == "teach"));
        assert_eq!(v.anchors[1].vec, vec![4.0]);
    }

    #[test]
    fn embeddings_parse_in_every_server_shape() {
        let flat = serde_json::json!([{"index": 0, "embedding": [3.0, 4.0]}]);
        let nested = serde_json::json!([{"index": 0, "embedding": [[3.0, 4.0]]}]);
        let openai = serde_json::json!({"data": [{"embedding": [3.0, 4.0]}]});
        for v in [flat, nested, openai] {
            assert_eq!(parse_embedding(&v), Some(vec![3.0, 4.0]));
        }
        assert_eq!(parse_embedding(&serde_json::json!({})), None);
    }
}

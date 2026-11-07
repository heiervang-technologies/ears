//! Command mode: speech routed to actions by its words and by embedding.
//!
//! Saying "command" on its own switches VAD into command mode (the ears
//! light turns violet). From then on each utterance is heard two ways at
//! once: transcribed, and its raw audio embedded with EmbeddingGemma 2 (the
//! cluster's `embeddinggemma2`, a llama-server with the audio encoder).
//! Words that are an action's name or one of its phrasings fire it outright.
//! Otherwise the audio, and the transcript's text embedding, are compared
//! with the anchors of every action (see [`CommandMode::hear_with`] for how
//! the two votes combine). The transcript is also the feedback: the subtitle
//! line shows what was heard, and the nearest action when nothing fired. Anchors are examples of what an action means: the text
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
//! again), `none` (ordinary chatter, so it has somewhere to land), `wrong`
//! (forget the voice anchor the last firing added), `chant` (chanting on or
//! off, [`crate::chant`]) and `agent`, the escape hatch for a missing
//! command: "agent ..." hands what follows, or else the last words that
//! fired nothing, to the `agent` program of `actions.toml`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Marker in the runtime dir while command mode is on (read by the light).
pub const MARKER: &str = "vad.command";
/// `ears teach ACTION` writes the action's name here; the next command-mode
/// utterance becomes one of its voice anchors.
pub const TEACH: &str = "command.teach";
/// Marker beside [`MARKER`] while the text anchors are still being embedded:
/// the mode is on, but only an action's exact words fire it.
pub const LOADING: &str = "vad.command.loading";
/// Last routing decision, for displays.
pub const LAST: &str = "command-last.json";

/// Spoken alone, in dictation or standby, these switch command mode on.
/// "commands": how the ASR model often hears a lone "command".
pub const ENTER_PHRASES: &[&str] = &["command", "commands", "command mode"];

/// Voice anchors kept per action; the oldest automatic ones go first.
const MAX_VOICE_ANCHORS: usize = 24;
/// Text inputs get the same task prefix on both sides (symmetric task).
const TEXT_PREFIX: &str = "task: sentence similarity | query: ";
/// Texts embedded per request when filling the text anchor cache.
const EMBED_BATCH: usize = 32;
/// Requests in flight when the server takes one text at a time.
const EMBED_CONCURRENCY: usize = 4;
/// New vectors between saves of the text anchor cache while it fills.
const CACHE_SAVE_EVERY: usize = 96;

const DEFAULT_ACTIONS: &str = include_str!("../contrib/actions.toml");

#[derive(Debug, thiserror::Error)]
pub enum CommandModeError {
    #[error("embedding server: {0}")]
    Embed(String),
    #[error("actions file {0}: {1}")]
    Actions(PathBuf, String),
}

pub type Result<T> = std::result::Result<T, CommandModeError>;

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
    /// The `agent` action runs this with what you meant as one more
    /// argument.
    #[serde(default = "default_agent")]
    pub agent: Vec<String>,
}

fn default_agent() -> Vec<String> {
    vec!["ears-chant-agent".into()]
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
    /// Fire only on its exact words, never on how it sounds or what it
    /// means: for actions a misfire would quietly leave changed (keyboard
    /// layout, touchpad, notifications off).
    #[serde(default)]
    pub exact: bool,
    /// Fire when the words start with one of its phrasings, and pass what
    /// follows to `run` as one more argument ("call moon" -> `... "moon"`,
    /// "" when nothing follows). By words only, like `exact`.
    #[serde(default)]
    pub rest: bool,
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
    /// Switch to the mode of a colour word ([`crate::colour_mode`]).
    Colour(crate::colour_mode::Colour),
    /// Chanting on or off ([`crate::chant`]).
    Chant,
    /// Hand what was meant to an agent: the words after "agent", else the
    /// last words that fired nothing ("" when there are none).
    Agent(String),
}

#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub effect: Effect,
    pub text: Vec<String>,
    /// Fires only by its exact words (see [`ActionSpec::exact`]).
    pub exact: bool,
    /// Takes the words after its phrasing as an argument (see [`ActionSpec::rest`]).
    pub rest: bool,
}

fn builtins() -> Vec<Action> {
    let a = |name: &str, effect, text: &[&str]| Action {
        exact: false,
        rest: false,
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
        // Colour words, as outside command mode (purple is this mode).
        a(
            "orange",
            Effect::Colour(crate::colour_mode::Colour::Orange),
            // "out": out of command mode, into standby
            &["orange", "orange mode", "out"],
        ),
        a(
            "blue",
            Effect::Colour(crate::colour_mode::Colour::Blue),
            &["blue", "blue mode", "cyan"],
        ),
        a(
            "yellow",
            Effect::Colour(crate::colour_mode::Colour::Yellow),
            &["yellow", "yellow mode"],
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
        Action {
            exact: true,
            ..a("chant", Effect::Chant, crate::chant::PHRASES)
        },
        Action {
            exact: true,
            rest: true,
            ..a(
                "agent",
                Effect::Agent(String::new()),
                &[
                    "agent",
                    "send to agent",
                    "ask the agent",
                    "tell the agent",
                    "ask agent",
                ],
            )
        },
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
                if crate::commands::key_sequence(keys).is_none() {
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
        if spec.rest && spec.run.is_none() {
            return Err(format!("action {}: `rest` needs `run`", spec.name));
        }
        let action = Action {
            exact: spec.exact || spec.rest,
            rest: spec.rest,
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

pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
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
    /// The text anchors are not in yet: nothing is scored, so only an
    /// action's exact words fire it.
    pub words_only: bool,
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
            words_only: false,
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
        if self.words_only {
            return Vec::new();
        }
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
                && !self.actions[w].exact
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

    /// The model the server runs (the first id in `/v1/models`), if it says.
    pub async fn model(&self) -> Option<String> {
        let reply: serde_json::Value = self
            .http
            .get(format!("{}/v1/models", self.url))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        reply["data"][0]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    async fn embed(&self, content: serde_json::Value) -> Result<Vec<f32>> {
        let value = self.post(content).await?;
        parse_embedding(&value)
            .map(normalized)
            .ok_or_else(|| CommandModeError::Embed("no embedding in the reply".into()))
    }

    async fn post(&self, content: serde_json::Value) -> Result<serde_json::Value> {
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
        response
            .json()
            .await
            .map_err(|e| CommandModeError::Embed(e.to_string()))
    }

    /// Several texts in one request (llama-server takes a list as
    /// `content`), in the order given.
    pub async fn texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let content: Vec<String> = texts.iter().map(|t| format!("{TEXT_PREFIX}{t}")).collect();
        let value = self.post(serde_json::json!(content)).await?;
        parse_embeddings(&value, texts.len())
            .map(|all| all.into_iter().map(normalized).collect())
            .ok_or_else(|| {
                CommandModeError::Embed("not one embedding per text in the reply".into())
            })
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
    parse_item(first)
}

fn parse_item(item: &serde_json::Value) -> Option<Vec<f32>> {
    let mut e = item.get("embedding")?;
    if e.get(0).is_some_and(|x| x.is_array()) {
        e = e.get(0)?;
    }
    e.as_array()?
        .iter()
        .map(|x| x.as_f64().map(|f| f as f32))
        .collect()
}

/// The reply to a list of `n` inputs, in input order (by each item's
/// `index` when it has one). None unless there is one embedding per input.
fn parse_embeddings(value: &serde_json::Value, n: usize) -> Option<Vec<Vec<f32>>> {
    let items = value
        .as_array()
        .or_else(|| value.get("data").and_then(|d| d.as_array()))?;
    if items.len() != n {
        return None;
    }
    let mut out = vec![None; n];
    for (at, item) in items.iter().enumerate() {
        let i = item
            .get("index")
            .and_then(|i| i.as_u64())
            .map_or(at, |i| i as usize);
        *out.get_mut(i)? = Some(parse_item(item)?);
    }
    out.into_iter().collect()
}

/// Text anchor vectors, cached on disk by text so entering the mode costs
/// one embedding call per new phrasing, not per anchor.
#[derive(Default, Serialize, Deserialize)]
struct TextCache {
    /// The server the vectors came from, as `actions.toml` spells it.
    server: String,
    /// Its model (`/v1/models`): what the vectors depend on, so the same
    /// model reached at another address reuses them. Absent in caches
    /// written before it was recorded; those are matched by `server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    vectors: std::collections::HashMap<String, Vec<f32>>,
}

impl TextCache {
    fn load(path: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    fn save(&self, path: &Path) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(self)?)?;
            std::fs::rename(tmp, path)
        };
        if let Err(e) = write() {
            tracing::warn!("Cannot save the text anchor cache: {}", e);
        }
    }
}

/// Text anchors as the router takes them: (action index, vector).
type TextAnchors = Vec<(usize, Vec<f32>)>;

/// The text anchors still to get: from the cache, else the server.
struct AnchorLoad {
    embedder: Embedder,
    server: String,
    path: PathBuf,
    /// (action index, phrasing) of every text anchor.
    texts: Vec<(usize, String)>,
    cache: Option<TextCache>,
}

impl AnchorLoad {
    fn anchors(&self, cache: &TextCache) -> Option<TextAnchors> {
        self.texts
            .iter()
            .map(|(i, text)| cache.vectors.get(text).map(|v| (*i, v.clone())))
            .collect()
    }

    /// Every anchor, if the cache written for this server address has them
    /// all: nothing to ask the server, which need not even be up.
    fn cached(&self) -> Option<TextAnchors> {
        let cache = self.cache.as_ref().filter(|c| c.server == self.server)?;
        self.anchors(cache)
    }

    /// Embed what the cache lacks, saving it as it fills (and on failure),
    /// so an interrupted load is resumed rather than started over.
    async fn run(mut self) -> Result<TextAnchors> {
        // The cache is kept if it is this model's; by server address, as
        // before, when either side does not name its model.
        let model = self.embedder.model().await;
        let mut cache = match self.cache.take() {
            Some(c)
                if match (&model, &c.model) {
                    (Some(now), Some(then)) => now == then,
                    _ => c.server == self.server,
                } =>
            {
                c
            }
            Some(_) => {
                tracing::info!(
                    "Command mode: the text anchor cache is of another model or server, starting a new one"
                );
                TextCache::default()
            }
            None => TextCache::default(),
        };
        let model = model.or(cache.model.take());
        // Kept under another address, or newly known to be this model's:
        // written back even if nothing is missing.
        let mut unsaved = usize::from(cache.server != self.server || cache.model != model);
        cache.server = self.server.clone();
        cache.model = model;
        let missing: Vec<String> = self
            .texts
            .iter()
            .map(|(_, text)| text)
            .filter(|text| !cache.vectors.contains_key(*text))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect();
        let mut batched = true;
        let mut failed = None;
        for chunk in missing.chunks(EMBED_BATCH) {
            let (vectors, error) = self.embed(chunk, &mut batched).await;
            unsaved += vectors.len();
            cache.vectors.extend(chunk.iter().cloned().zip(vectors));
            if error.is_some() {
                failed = error;
                break;
            }
            if unsaved >= CACHE_SAVE_EVERY {
                cache.save(&self.path);
                unsaved = 0;
            }
        }
        if unsaved > 0 {
            cache.save(&self.path);
        }
        match failed {
            Some(e) => Err(e),
            None => self
                .anchors(&cache)
                .ok_or_else(|| CommandModeError::Embed("a text anchor was not embedded".into())),
        }
    }

    /// The vectors of the first of `texts` that could be embedded, and the
    /// error that stopped the rest. One request for all of them; a server
    /// that does not take a list is asked one text at a time from then on,
    /// a few at once.
    async fn embed(
        &self,
        texts: &[String],
        batched: &mut bool,
    ) -> (Vec<Vec<f32>>, Option<CommandModeError>) {
        use futures_util::StreamExt;
        if *batched {
            match self.embedder.texts(texts).await {
                Ok(vectors) => return (vectors, None),
                Err(e) => {
                    tracing::debug!("Command mode: no batch embedding ({}), one by one", e);
                    *batched = false;
                }
            }
        }
        let mut vectors = Vec::with_capacity(texts.len());
        let mut replies = futures_util::stream::iter(texts.to_vec())
            .map(|text| {
                let embedder = self.embedder.clone();
                async move { embedder.text(&text).await }
            })
            .buffered(EMBED_CONCURRENCY);
        while let Some(reply) = replies.next().await {
            match reply {
                Ok(v) => vectors.push(v),
                Err(e) => return (vectors, Some(e)),
            }
        }
        (vectors, None)
    }
}

/// The task embedding the text anchors, stopped when command mode is left.
struct LoadTask {
    task: tokio::task::JoinHandle<()>,
    marker: PathBuf,
}

impl Drop for LoadTask {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.marker);
    }
}

/// Where the text anchors are.
enum Anchors {
    /// In the router.
    Ready,
    /// Not asked for yet.
    Pending(Box<AnchorLoad>),
    /// A task is getting them ([`CommandMode::start_loading`]).
    Loading {
        done: tokio::sync::oneshot::Receiver<Result<TextAnchors>>,
        _task: LoadTask,
    },
    /// The server did not give them: words only until the mode is entered
    /// again.
    Failed,
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
    anchors: Anchors,
    pub router: Router,
    pub embedder: Embedder,
    pub paths: Paths,
    voice: VoiceAnchors,
    /// The voice anchor the last firing added, for `wrong`.
    last_auto: Option<VoiceAnchor>,
    /// The `agent` program ([`ActionsFile::agent`]).
    pub agent: Vec<String>,
    /// The last words that fired nothing, for `agent`.
    pub last_miss: Option<String>,
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

/// How one command-mode utterance was heard: written to [`LAST`] and logged.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Hearing {
    /// What the ASR heard ("" if it failed or was too slow).
    pub words: String,
    /// The vote on the raw audio, if it could be embedded.
    pub audio: Option<Decision>,
    /// The vote on the transcript's text embedding.
    pub text: Option<Decision>,
    /// What fired it: "words" (an exact phrasing), "both" (audio and text
    /// agree), "voice" (the audio alone) or "meaning" (the text alone).
    pub by: Option<&'static str>,
    /// Nothing fired: the action it came closest to, if not chatter.
    pub nearest: Option<String>,
    /// Which mode heard it: "command" here; a voice command heard in
    /// another mode says that one ([`DictationCommand`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
    /// When each stage was done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
}

/// When each stage of hearing a command was done, in milliseconds after
/// VAD called the utterance ended (which was `silence_ms` after the last
/// voiced frame). A stage that did not run is absent.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Timing {
    /// The silence VAD waited for before calling the utterance ended.
    pub silence_ms: u64,
    /// The transcript was in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_ms: Option<f64>,
    /// The audio embedding was in (command mode; it may come in after the
    /// decision when the words alone decided).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_ms: Option<f64>,
    /// What to do was decided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_ms: Option<f64>,
    /// The action was done: the key pressed, the program started, the mode
    /// switched. Includes the accept colour hold (`[commands] accept_ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_ms: Option<f64>,
}

/// The zero of a [`Timing`]: when VAD called the utterance ended.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub ended: std::time::Instant,
    pub silence_ms: u64,
}

impl Clock {
    /// Milliseconds from the end to `at`, to a tenth.
    pub fn ms(&self, at: std::time::Instant) -> f64 {
        (at.saturating_duration_since(self.ended).as_secs_f64() * 10_000.0).round() / 10.0
    }

    pub fn timing(&self) -> Timing {
        Timing {
            silence_ms: self.silence_ms,
            ..Default::default()
        }
    }
}

impl std::fmt::Display for Timing {
    /// "asr 110 ms, embedding 300 ms, decision 112 ms, action 395 ms after
    /// 400 ms of silence".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stages = [
            ("asr", self.asr_ms),
            ("embedding", self.embedding_ms),
            ("decision", self.decision_ms),
            ("action", self.action_ms),
        ];
        let parts: Vec<String> = (stages.iter())
            .filter_map(|(name, ms)| ms.map(|ms| format!("{name} {ms:.0} ms")))
            .collect();
        write!(
            f,
            "{} after {} ms of silence",
            if parts.is_empty() {
                "nothing".to_string()
            } else {
                parts.join(", ")
            },
            self.silence_ms
        )
    }
}

/// A voice command heard outside command mode (dictating, or a wake
/// phrase in standby), as written to [`LAST`].
#[derive(Debug, Clone, Serialize)]
pub struct DictationCommand {
    /// The mode it was heard in.
    pub mode: crate::vad_mode::ModeKind,
    /// The command's label ("enter", "keys:ctrl+c", a run command's name).
    pub command: String,
    /// The words, as already logged.
    pub words: String,
    pub timing: Timing,
}

/// Write `record` to [`LAST`] in `state_dir`.
pub fn write_last(state_dir: &Path, record: &impl Serialize) {
    let _ = std::fs::write(
        state_dir.join(LAST),
        serde_json::to_vec(record).unwrap_or_default(),
    );
}

/// The chant so far, as [`CommandMode::chant_hear`] heard it.
#[derive(Debug, Clone, Default)]
pub struct ChantHearing {
    /// What it repeats (folded words), "" without words.
    pub heard: String,
    /// The action those words name, and a `rest` action's argument.
    pub named: Option<(usize, Option<String>)>,
    pub audio: Option<Decision>,
    pub meaning: Option<Decision>,
}

/// Lowercase words, punctuation and filler gone: "Press Enter." -> "press enter".
pub fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !matches!(*w, "please" | "uh" | "um" | "erm"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Router {
    /// The action whose name or one of whose phrasings the words are.
    pub fn by_words(&self, words: &str) -> Option<usize> {
        let said = plain(words);
        if said.is_empty() {
            return None;
        }
        self.actions
            .iter()
            .position(|a| plain(&a.name) == said || a.text.iter().any(|t| plain(t) == said))
    }

    /// An action taking the rest of the words, and that rest: the longest
    /// phrasing the words start with wins ("call clanker moon" over "call").
    pub fn by_rest(&self, words: &str) -> Option<(usize, String)> {
        let said = plain(words);
        let mut best: Option<(usize, usize, String)> = None;
        for (i, a) in self.actions.iter().enumerate().filter(|(_, a)| a.rest) {
            for p in std::iter::once(&a.name)
                .chain(a.text.iter())
                .map(|t| plain(t))
            {
                if p.is_empty() || best.as_ref().is_some_and(|b| b.1 >= p.len()) {
                    continue;
                }
                let rest = if said == p {
                    Some(String::new())
                } else {
                    said.strip_prefix(&format!("{p} ")).map(str::to_string)
                };
                if let Some(rest) = rest {
                    best = Some((i, p.len(), rest));
                }
            }
        }
        best.map(|(i, _, rest)| (i, rest))
    }
}

/// Combine the three ways an utterance is heard into the action to fire.
/// Exact words win. Then audio and text: agreeing, or either alone. If they
/// fire different actions, nothing does: a wrong key is worse than a retry.
fn combine(
    words: Option<usize>,
    audio: Option<&Decision>,
    text: Option<&Decision>,
) -> (Option<usize>, Option<&'static str>) {
    if words.is_some() {
        return (words, Some("words"));
    }
    match (audio.and_then(|d| d.fire), text.and_then(|d| d.fire)) {
        (Some(a), Some(t)) if a == t => (Some(a), Some("both")),
        (Some(_), Some(_)) => (None, None),
        (Some(a), None) => (Some(a), Some("voice")),
        (None, Some(t)) => (Some(t), Some("meaning")),
        (None, None) => (None, None),
    }
}

/// The action the votes leaned to when nothing fired (not chatter).
fn nearest(hearing: &Hearing) -> Option<String> {
    [hearing.text.as_ref(), hearing.audio.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|d| d.votes.first().map(|v| v.0.clone()))
        .filter(|name| name != "none")
}

impl CommandMode {
    /// Load actions and anchors, embedding text anchors not in the cache.
    pub async fn start(paths: Paths) -> Result<Self> {
        let mut mode = Self::open(paths)?;
        if let Anchors::Pending(load) = std::mem::replace(&mut mode.anchors, Anchors::Ready) {
            let anchors = load.run().await?;
            mode.add_text_anchors(anchors);
        }
        Ok(mode)
    }

    /// As [`CommandMode::start`], without waiting for the embedding server:
    /// text anchors missing from the cache are embedded by a task, and until
    /// they are in ([`CommandMode::ready`]; the [`LOADING`] marker is up
    /// meanwhile) only an action's exact words fire it.
    pub fn start_loading(paths: Paths) -> Result<Self> {
        let mut mode = Self::open(paths)?;
        if let Anchors::Pending(load) = std::mem::replace(&mut mode.anchors, Anchors::Ready) {
            let marker = mode.paths.state_dir.join(LOADING);
            if let Err(e) = std::fs::write(&marker, "") {
                tracing::warn!("Cannot write the command mode loading marker: {}", e);
            }
            let (tx, done) = tokio::sync::oneshot::channel();
            let finished = marker.clone();
            let task = tokio::spawn(async move {
                let started = std::time::Instant::now();
                let result = load.run().await;
                match &result {
                    Ok(anchors) => tracing::info!(
                        "Command mode: {} text anchors ready in {:?}",
                        anchors.len(),
                        started.elapsed()
                    ),
                    Err(e) => tracing::warn!(
                        "Command mode: no text anchors ({}), exact words only until it is entered again",
                        e
                    ),
                }
                let _ = std::fs::remove_file(finished);
                let _ = tx.send(result);
            });
            mode.anchors = Anchors::Loading {
                done,
                _task: LoadTask { task, marker },
            };
        }
        Ok(mode)
    }

    /// Whether the text anchors are in, taking them from the loading task
    /// if it has finished. Until then nothing fires by embedding.
    pub fn ready(&mut self) -> bool {
        if let Anchors::Loading { done, .. } = &mut self.anchors {
            use tokio::sync::oneshot::error::TryRecvError;
            match done.try_recv() {
                Ok(Ok(anchors)) => {
                    self.add_text_anchors(anchors);
                    self.anchors = Anchors::Ready;
                }
                Ok(Err(_)) | Err(TryRecvError::Closed) => self.anchors = Anchors::Failed,
                Err(TryRecvError::Empty) => {}
            }
        }
        matches!(self.anchors, Anchors::Ready)
    }

    fn add_text_anchors(&mut self, anchors: TextAnchors) {
        for (action, vec) in anchors {
            self.router.add_anchor(action, vec, false);
        }
        self.router.words_only = false;
    }

    /// Actions, voice anchors, and the text anchors the cache already has
    /// (all of them, or none until the rest are embedded).
    fn open(paths: Paths) -> Result<Self> {
        let (file, mut actions) = load_actions(&paths.actions)?;
        // Known mishearings of the phrasings (`action:<name>`, see `crate::cs`).
        crate::cs::apply_actions(&mut actions, &crate::cs::active());
        let embedder = Embedder::new(&file.server);
        let mut router = Router::new(actions, file.floor, file.margin, file.k);
        let load = AnchorLoad {
            embedder: embedder.clone(),
            server: file.server.clone(),
            path: paths.text_cache.clone(),
            texts: (router.actions.iter().enumerate())
                .flat_map(|(i, a)| a.text.iter().map(move |text| (i, text.clone())))
                .collect(),
            cache: TextCache::load(&paths.text_cache),
        };
        let anchors = match load.cached() {
            Some(anchors) => {
                for (action, vec) in anchors {
                    router.add_anchor(action, vec, false);
                }
                Anchors::Ready
            }
            None => {
                router.words_only = true;
                Anchors::Pending(Box::new(load))
            }
        };
        let voice = VoiceAnchors::load(&paths.voice);
        for anchor in &voice.anchors {
            if let Some(i) = router.index(&anchor.action) {
                router.add_anchor(i, anchor.vec.clone(), true);
            }
        }
        Ok(Self {
            anchors,
            router,
            embedder,
            paths,
            voice,
            last_auto: None,
            agent: file.agent,
            last_miss: None,
        })
    }

    /// Route one utterance by its audio alone (16 kHz mono WAV bytes).
    pub async fn hear(&mut self, wav: &[u8]) -> Result<(Outcome, Hearing)> {
        let u = self.embedder.audio(wav).await?;
        Ok(self.hear_with(Some(u), "").await)
    }

    /// Whether the words alone fire an action (its name or a phrasing,
    /// cs variants included), so the audio embedding need not be waited
    /// for. Not while `ears teach` waits to label the voice.
    pub fn fires_by_words(&self, words: &str) -> bool {
        self.by_words(words).0.is_some() && !self.paths.state_dir.join(TEACH).exists()
    }

    /// The action the words name, and for a `rest` action what follows.
    fn by_words(&self, words: &str) -> (Option<usize>, Option<(usize, String)>) {
        let rest = self.router.by_rest(words);
        let by_words = self
            .router
            .by_words(words)
            .filter(|&i| !self.router.actions[i].rest)
            .or(rest.as_ref().map(|r| r.0));
        (by_words, rest)
    }

    /// The phrasings that fire an action as said, for ending an utterance
    /// early ([`crate::endpoint::Ending::Exact`]). A `rest` action's are
    /// open: more words may follow them.
    pub fn phrases(&self) -> crate::endpoint::Phrases {
        let said = |a: &Action| {
            std::iter::once(a.name.clone())
                .chain(a.text.iter().cloned())
                .collect::<Vec<_>>()
        };
        let (open, complete): (Vec<&Action>, Vec<&Action>) =
            self.router.actions.iter().partition(|a| a.rest);
        crate::endpoint::Phrases::new(
            complete.into_iter().flat_map(said),
            open.into_iter().flat_map(said),
            plain,
        )
    }

    /// The audio embedding of an utterance that already fired by its words
    /// ([`CommandMode::fires_by_words`]), in late: the vote goes on the
    /// record, and a firing teaches the voice its phrasing as it would
    /// have had the embedding been in time.
    pub fn heard_audio(&mut self, hearing: &mut Hearing, fired: Option<&str>, u: Vec<f32>) {
        hearing.audio = Some(self.router.decide(&u));
        let Some(i) = fired.and_then(|name| self.router.index(name)) else {
            return;
        };
        if hearing.by == Some("words") && !self.router.actions[i].rest {
            self.remember(i, u, "auto");
        }
    }

    /// As [`CommandMode::heard_audio`] once command mode is off (the action
    /// left it): the voice anchor goes straight to the file.
    pub fn remember_after(paths: &Paths, action: &str, u: Vec<f32>) {
        let mut voice = VoiceAnchors::load(&paths.voice);
        voice.add(VoiceAnchor {
            action: action.to_string(),
            source: "auto".into(),
            vec: u,
        });
        if let Err(e) = voice.save(&paths.voice) {
            tracing::warn!("Cannot save command anchors: {}", e);
        }
    }

    /// Route one utterance from its audio embedding (`audio`, if the
    /// embedding worked) and its transcript (`words`, "" if the ASR did not
    /// answer in time). See [`combine`].
    pub async fn hear_with(&mut self, audio: Option<Vec<f32>>, words: &str) -> (Outcome, Hearing) {
        let ready = self.ready();
        let (by_words, rest) = self.by_words(words);
        let mut hearing = Hearing {
            words: words.trim().to_string(),
            audio: audio.as_ref().map(|u| self.router.decide(u)),
            mode: Some("command"),
            ..Default::default()
        };
        // What the words mean, unless they already name an action.
        if ready && by_words.is_none() && !plain(words).is_empty() {
            match self.embedder.text(words.trim()).await {
                Ok(v) => hearing.text = Some(self.router.decide(&v)),
                Err(e) => tracing::warn!("Command mode: cannot embed the transcript: {}", e),
            }
        }
        // A label from `ears teach` wins over the vote (it needs the voice).
        let teach = self.paths.state_dir.join(TEACH);
        if let (Some(u), Ok(name)) = (audio.as_ref(), std::fs::read_to_string(&teach)) {
            let _ = std::fs::remove_file(&teach);
            let name = name.trim().to_string();
            if let Some(i) = self.router.index(&name) {
                self.remember(i, u.clone(), "teach");
                self.write_last(&hearing);
                return (Outcome::Taught(name), hearing);
            }
        }
        let (fire, by) = combine(by_words, hearing.audio.as_ref(), hearing.text.as_ref());
        hearing.by = by;
        let Some(i) = fire else {
            hearing.nearest = nearest(&hearing);
            self.missed(words);
            self.write_last(&hearing);
            return (Outcome::Nothing, hearing);
        };
        self.write_last(&hearing);
        let action = self.router.actions[i].clone();
        let arg = rest.filter(|r| r.0 == i).map(|r| r.1);
        let effect = self.effect(i, arg);
        let outcome = match effect {
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
                // Few-shot calibration: a clear firing teaches the voice its
                // own phrasing. A narrow one might be a misroute nobody undid.
                let clear =
                    |d: Option<&Decision>| d.is_some_and(|d| d.margin >= 2.0 * self.router.margin);
                let teaches = match by {
                    Some("words") | Some("both") => true,
                    Some("voice") => clear(hearing.audio.as_ref()),
                    _ => clear(hearing.text.as_ref()),
                };
                // "call moon" says little about how "call doc" sounds.
                if let (true, false, Some(u)) = (teaches, action.rest, audio) {
                    self.remember(i, u, "auto");
                }
                Outcome::Fire {
                    name: action.name,
                    effect,
                }
            }
        };
        (outcome, hearing)
    }

    /// What firing action `i` does, given the words after its phrasing
    /// (`rest` actions): a `run` gets them as one more argument; `agent`
    /// takes them as what was meant, else the last miss.
    pub fn effect(&self, i: usize, rest: Option<String>) -> Effect {
        let action = &self.router.actions[i];
        match (action.effect.clone(), rest) {
            (Effect::Run(mut argv), Some(arg)) if action.rest => {
                argv.push(arg);
                Effect::Run(argv)
            }
            (Effect::Agent(_), rest) => Effect::Agent(
                rest.filter(|r| !r.trim().is_empty())
                    .or_else(|| self.last_miss.clone())
                    .unwrap_or_default(),
            ),
            (effect, _) => effect,
        }
    }

    /// Keep words that fired nothing for the `agent` action.
    pub fn missed(&mut self, words: &str) {
        let words = words.trim();
        if !plain(words).is_empty() {
            self.last_miss = Some(words.to_string());
        }
    }

    /// Hear the chant so far, without acting or learning: its folded words,
    /// the action they name, and the votes on the audio and on their
    /// meaning (skipped when the words name an action). Until the text
    /// anchors are in ([`CommandMode::ready`]) it goes by the words alone:
    /// no meaning is asked of the embedding server, busy with the load.
    pub async fn chant_hear(&mut self, audio: Option<&[f32]>, words: &str) -> ChantHearing {
        let ready = self.ready();
        let units = crate::chant::units(words);
        let named = crate::chant::by_words(&self.router, &units);
        let meaning = match (&named, units.first()) {
            (None, Some(unit)) if ready => match self.embedder.text(unit).await {
                Ok(v) => Some(self.router.decide(&v)),
                Err(e) => {
                    tracing::debug!("Chant: cannot embed {:?}: {}", unit, e);
                    None
                }
            },
            _ => None,
        };
        ChantHearing {
            heard: units.first().cloned().unwrap_or_default(),
            named,
            audio: audio.map(|u| self.router.decide(u)),
            meaning,
        }
    }

    fn write_last(&self, hearing: &Hearing) {
        write_last(&self.paths.state_dir, hearing);
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
    fn out_goes_to_standby() {
        let (_, actions) = parse_actions(DEFAULT_ACTIONS).unwrap();
        let orange = actions.iter().find(|a| a.name == "orange").unwrap();
        assert_eq!(
            orange.effect,
            Effect::Colour(crate::colour_mode::Colour::Orange)
        );
        assert!(orange.text.iter().any(|t| t == "out"));
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
    fn a_rest_action_takes_the_words_after_it() {
        let toml = "[[action]]\nname = \"call\"\nrun = [\"ears-call\"]\nrest = true\n\
                    text = [\"call\", \"call clanker\", \"call agent\"]\n";
        let (_, actions) = parse_actions(toml).unwrap();
        let call = actions.iter().position(|a| a.name == "call").unwrap();
        assert!(actions[call].exact && actions[call].rest);
        let r = Router::new(actions, 1.0, 0.1, 3);
        assert_eq!(r.by_rest("Call moon."), Some((call, "moon".into())));
        assert_eq!(
            r.by_rest("call clanker the TTS one"),
            Some((call, "the tts one".into()))
        );
        assert_eq!(r.by_rest("Call agent"), Some((call, String::new())));
        assert_eq!(r.by_rest("recall moon"), None);
        assert!(
            parse_actions("[[action]]\nname = \"x\"\nkeys = \"enter\"\nrest = true\n").is_err()
        );
    }

    #[test]
    fn an_exact_action_fires_on_its_words_only() {
        let mut r = router();
        let next = r.index("next").unwrap();
        r.actions[next].exact = true;
        let d = r.decide(&unit(&[0.1, 1.0, 0.0, 0.0, 0.0]));
        assert_eq!(d.fired, None, "never by sound or meaning");
        assert_eq!(d.votes[0].0, "next", "still shown as the nearest");
        assert_eq!(r.by_words("Next."), Some(next));
        let (_, actions) =
            parse_actions("[[action]]\nname = \"layout\"\nrun = [\"true\"]\nexact = true\n")
                .unwrap();
        assert!(actions.iter().any(|a| a.name == "layout" && a.exact));
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

    fn decision(r: &Router, v: &[f32]) -> Decision {
        r.decide(&unit(v))
    }

    #[test]
    fn words_name_an_action_by_its_name_or_a_phrasing() {
        let r = router();
        assert_eq!(r.by_words("Press enter."), r.index("enter"));
        assert_eq!(r.by_words("NEXT!"), r.index("next"));
        assert_eq!(r.by_words("Please, press enter"), r.index("enter"));
        assert_eq!(r.by_words("press enter twice"), None);
        assert_eq!(r.by_words(""), None);
    }

    #[test]
    fn exact_words_win_and_disagreement_fires_nothing() {
        let r = router();
        let enter = decision(&r, &[1.0, 0.0, 0.0, 0.0, 0.0]);
        let next = decision(&r, &[0.0, 1.0, 0.0, 0.0, 0.0]);
        let unsure = decision(&r, &[0.5, 0.5, 0.5, 0.5, 0.0]);
        let (e, n) = (r.index("enter"), r.index("next"));
        assert_eq!(combine(n, Some(&enter), Some(&enter)), (n, Some("words")));
        assert_eq!(combine(None, Some(&enter), Some(&enter)), (e, Some("both")));
        assert_eq!(combine(None, Some(&enter), Some(&next)), (None, None));
        assert_eq!(
            combine(None, Some(&enter), Some(&unsure)),
            (e, Some("voice"))
        );
        assert_eq!(combine(None, None, Some(&next)), (n, Some("meaning")));
        assert_eq!(combine(None, Some(&unsure), None), (None, None));
    }

    /// An embedding server: `/v1/models` names `model` (404 without one),
    /// `/embeddings` answers a text or a list of texts after `delay`, and
    /// fails once `fail_after` requests have been answered.
    async fn server(
        model: Option<&str>,
        delay: Duration,
        fail_after: usize,
    ) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, ResponseTemplate};
        let server = MockServer::start().await;
        if let Some(model) = model {
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"data": [{"id": model}]})),
                )
                .mount(&server)
                .await;
        }
        let answered = std::sync::atomic::AtomicUsize::new(0);
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(move |request: &Request| {
                if answered.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= fail_after {
                    return ResponseTemplate::new(500);
                }
                let body: serde_json::Value = request.body_json().unwrap();
                let n = body["content"].as_array().map_or(1, Vec::len);
                let items: Vec<_> = (0..n)
                    .map(|i| serde_json::json!({"index": i, "embedding": [[1.0, i as f32]]}))
                    .collect();
                ResponseTemplate::new(200)
                    .set_body_json(items)
                    .set_delay(delay)
            })
            .mount(&server)
            .await;
        server
    }

    /// Paths in `dir`, with an actions file of `phrases` phrasings on `server`.
    fn paths(dir: &Path, server: &str, phrases: usize) -> Paths {
        let text: Vec<String> = (0..phrases).map(|i| format!("\"phrase {i}\"")).collect();
        let actions = format!(
            "server = \"{server}\"\n[[action]]\nname = \"enter\"\nkeys = \"enter\"\ntext = [{}]\n",
            text.join(", ")
        );
        std::fs::write(dir.join("actions.toml"), actions).unwrap();
        Paths {
            actions: dir.join("actions.toml"),
            voice: dir.join("command-anchors.json"),
            text_cache: dir.join("command-text-anchors.json"),
            state_dir: dir.to_path_buf(),
        }
    }

    fn text_anchors(paths: &Paths) -> usize {
        let (_, actions) = load_actions(&paths.actions).unwrap();
        actions.iter().map(|a| a.text.len()).sum()
    }

    /// Requests it takes to embed `texts` texts.
    fn batches(texts: usize) -> usize {
        texts.div_ceil(EMBED_BATCH)
    }

    fn cache(paths: &Paths) -> TextCache {
        TextCache::load(&paths.text_cache).expect("a cache file that parses")
    }

    /// POSTs to `/embeddings` so far, and the texts they asked for.
    async fn embedded(server: &wiremock::MockServer) -> (usize, usize) {
        let requests = server.received_requests().await.unwrap();
        let posts: Vec<_> = (requests.iter())
            .filter(|r| r.url.path() == "/embeddings")
            .collect();
        let texts = (posts.iter())
            .map(|r| {
                let body: serde_json::Value = r.body_json().unwrap();
                body["content"].as_array().map_or(1, Vec::len)
            })
            .sum();
        (posts.len(), texts)
    }

    #[tokio::test]
    async fn entering_does_not_wait_for_the_embedding_server() {
        let delay = Duration::from_millis(400);
        let server = server(Some("gemma"), delay, usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path(), &server.uri(), 3);
        let loading = dir.path().join(LOADING);

        let started = std::time::Instant::now();
        let mut mode = CommandMode::start_loading(paths.clone()).unwrap();
        // Exact words fire while the anchors load, without the server.
        let (outcome, hearing) = mode.hear_with(Some(unit(&[1.0, 0.0])), "Enter.").await;
        assert!(started.elapsed() < delay, "{:?}", started.elapsed());
        assert!(matches!(outcome, Outcome::Fire { ref name, .. } if name == "enter"));
        assert_eq!(hearing.by, Some("words"));
        // Nothing else does: no vote, and no request for the transcript.
        let (outcome, hearing) = mode.hear_with(Some(unit(&[1.0, 0.0])), "phrase").await;
        assert_eq!(outcome, Outcome::Nothing);
        assert!(hearing.text.is_none() && hearing.audio.unwrap().votes.is_empty());
        assert!(started.elapsed() < delay, "{:?}", started.elapsed());
        assert!(!mode.ready() && loading.exists());

        for _ in 0..100 {
            if mode.ready() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(mode.ready() && !loading.exists());
        assert!(!mode.router.words_only);
        let (_, hearing) = mode.hear_with(Some(unit(&[1.0, 0.0])), "").await;
        assert!(!hearing.audio.unwrap().votes.is_empty());
        // A request per batch of text anchors, not one each.
        let total = text_anchors(&paths);
        assert_eq!(embedded(&server).await, (batches(total), total));
        assert_eq!(cache(&paths).model.as_deref(), Some("gemma"));

        // With the cache full, entering needs no server and no loading.
        drop(server);
        let mut again = CommandMode::start_loading(paths).unwrap();
        assert!(again.ready() && !loading.exists());
    }

    #[tokio::test]
    async fn leaving_stops_the_load_and_takes_the_marker_down() {
        let server = server(Some("gemma"), Duration::from_secs(5), usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let mode = CommandMode::start_loading(paths(dir.path(), &server.uri(), 3)).unwrap();
        assert!(dir.path().join(LOADING).exists());
        drop(mode);
        assert!(!dir.path().join(LOADING).exists());
    }

    #[tokio::test]
    async fn an_interrupted_load_keeps_what_it_embedded() {
        let dir = tempfile::tempdir().unwrap();
        // Three batches' worth; the server answers two, then fails.
        let failing = server(Some("gemma"), Duration::ZERO, 2).await;
        let paths = paths(dir.path(), &failing.uri(), 2 * EMBED_BATCH + 5);
        let total = text_anchors(&paths);
        assert!(CommandMode::start(paths.clone()).await.is_err());
        assert_eq!(cache(&paths).vectors.len(), 2 * EMBED_BATCH);
        assert!(!paths.text_cache.with_extension("json.tmp").exists());

        // The next attempt embeds only the rest (same model, new address).
        let working = server(Some("gemma"), Duration::ZERO, usize::MAX).await;
        let paths = self::paths(dir.path(), &working.uri(), 2 * EMBED_BATCH + 5);
        let mode = CommandMode::start(paths.clone()).await.unwrap();
        assert!(matches!(mode.anchors, Anchors::Ready));
        let rest = total - 2 * EMBED_BATCH;
        assert_eq!(embedded(&working).await, (batches(rest), rest));
        assert_eq!(cache(&paths).vectors.len(), total);
    }

    #[tokio::test]
    async fn a_filling_cache_is_saved_before_the_load_ends() {
        let server = server(Some("gemma"), Duration::from_millis(300), usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(
            dir.path(),
            &server.uri(),
            CACHE_SAVE_EVERY + 2 * EMBED_BATCH,
        );
        let mut mode = CommandMode::start_loading(paths.clone()).unwrap();
        for _ in 0..100 {
            if paths.text_cache.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let saved = cache(&paths).vectors.len();
        assert!(!mode.ready(), "saved while still loading");
        assert!(saved >= CACHE_SAVE_EVERY && saved < text_anchors(&paths));
        // Left mid-load: the file stays, whole.
        drop(mode);
        assert!(cache(&paths).vectors.len() >= saved);
    }

    #[tokio::test]
    async fn a_server_without_batches_is_asked_one_text_at_a_time() {
        use wiremock::matchers::{method, path};
        let server = server(Some("gemma"), Duration::ZERO, usize::MAX).await;
        // A list comes back as a single embedding.
        wiremock::Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(|r: &wiremock::Request| {
                r.body_json::<serde_json::Value>()
                    .is_ok_and(|b| b["content"].is_array())
            })
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"index": 0, "embedding": [[1.0, 0.0]]}])),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path(), &server.uri(), EMBED_BATCH + 3);
        let total = text_anchors(&paths);
        CommandMode::start(paths.clone()).await.unwrap();
        // One refused batch, then every text on its own.
        assert_eq!(embedded(&server).await, (1 + total, EMBED_BATCH + total));
        assert_eq!(cache(&paths).vectors.len(), total);
    }

    #[tokio::test]
    async fn the_cache_is_keyed_by_model_and_an_old_one_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let write = |paths: &Paths, server: &str, model: Option<&str>, skip: &str| {
            let (_, actions) = load_actions(&paths.actions).unwrap();
            let mut json = serde_json::json!({
                "server": server,
                "vectors": (actions.iter().flat_map(|a| &a.text))
                    .filter(|t| *t != skip)
                    .map(|t| (t.clone(), vec![0.0, 1.0]))
                    .collect::<std::collections::HashMap<_, _>>(),
            });
            if let Some(model) = model {
                json["model"] = model.into();
            }
            std::fs::write(&paths.text_cache, json.to_string()).unwrap();
        };

        // Old format (no model), this address: kept, and given the model.
        let gemma = server(Some("gemma"), Duration::ZERO, usize::MAX).await;
        let paths = paths(dir.path(), &gemma.uri(), 3);
        let total = text_anchors(&paths);
        write(&paths, &gemma.uri(), None, "phrase 1");
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&gemma).await, (1, 1));
        let migrated = cache(&paths);
        assert_eq!(migrated.model.as_deref(), Some("gemma"));
        assert_eq!(migrated.vectors.len(), total);
        assert_eq!(migrated.vectors["phrase 0"], vec![0.0, 1.0]);

        // The same model at another address: reused whole, and adopted.
        let elsewhere = server(Some("gemma"), Duration::ZERO, usize::MAX).await;
        let paths = self::paths(dir.path(), &elsewhere.uri(), 3);
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&elsewhere).await, (0, 0));
        assert_eq!(cache(&paths).server, elsewhere.uri());

        // Another model at the same address, with a phrasing to embed: not this cache.
        let other = server(Some("other"), Duration::ZERO, usize::MAX).await;
        let paths = self::paths(dir.path(), &other.uri(), 3);
        write(&paths, &other.uri(), Some("gemma"), "phrase 1");
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&other).await, (batches(total), total));
        assert_eq!(cache(&paths).model.as_deref(), Some("other"));

        // A server that names no model: by address, as before.
        let nameless = server(None, Duration::ZERO, usize::MAX).await;
        let paths = self::paths(dir.path(), &nameless.uri(), 3);
        write(&paths, &nameless.uri(), None, "phrase 1");
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&nameless).await, (1, 1));
        write(&paths, "http://elsewhere:1", None, "phrase 1");
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&nameless).await, (1 + batches(total), 1 + total));
        // An old cache of another address is of an unknown model: not used.
        write(&paths, "http://elsewhere:1", None, "phrase 1");
        let paths = self::paths(dir.path(), &gemma.uri(), 3);
        CommandMode::start(paths.clone()).await.unwrap();
        assert_eq!(embedded(&gemma).await, (1 + batches(total), 1 + total));
    }

    #[test]
    fn a_batch_reply_parses_in_input_order() {
        let reply = serde_json::json!([
            {"index": 1, "embedding": [[0.0, 2.0]]},
            {"index": 0, "embedding": [[1.0, 0.0]]},
        ]);
        assert_eq!(
            parse_embeddings(&reply, 2),
            Some(vec![vec![1.0, 0.0], vec![0.0, 2.0]])
        );
        assert_eq!(parse_embeddings(&reply, 3), None);
        let openai = serde_json::json!({"data": [{"embedding": [3.0, 4.0]}]});
        assert_eq!(parse_embeddings(&openai, 1), Some(vec![vec![3.0, 4.0]]));
    }

    #[test]
    fn a_miss_names_the_nearest_action_unless_it_was_chatter() {
        let r = router();
        let mut hearing = Hearing {
            text: Some(decision(&r, &[0.6, 0.3, 0.0, 0.0, 0.5])),
            ..Default::default()
        };
        assert_eq!(nearest(&hearing).as_deref(), Some("enter"));
        hearing.text = Some(decision(&r, &[0.0, 0.0, 1.0, 0.0, 0.0]));
        assert_eq!(nearest(&hearing), None);
    }
}

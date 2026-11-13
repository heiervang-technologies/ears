//! Deterministic, English-oriented risk estimates for extra ASR trigger phrases.
//!
//! # Phonetics
//! A small, original grapheme-to-phoneme approximation retains vowels, folds
//! common digraphs and silent letters, and handles a few irregular common words.
//! Keys concatenate word sounds without boundary tokens; adjacent single-letter
//! tokens are joined before coding (`v a d` = `vad`). This models ASR spelling,
//! not the pronunciation of alphabet names. Keys are diagnostic tokens, not IPA.
//! Voicing and vowel differences are retained so the distance can weight them.
//! Unsupported non-ASCII words and digits remain literal, rather than disappearing
//! and accidentally becoming homophones. English accents, stress and heteronyms
//! are not fully modeled; this is a review heuristic, not a pronunciation oracle.
//!
//! Weighted Levenshtein uses integer twentieths: insertion/deletion 20, vowel
//! substitution 5, voiced/unvoiced partners (and th/t/d) 4, other substitutions within a
//! broad articulation class 12, unrelated substitution 40. These nested classes
//! give metric edit costs. For raw edit cost `d` and lengths `n`, `m`, distance is
//! `2*d / (20*(n+m) + d)` (zero for two empty sequences). This is the Steinhaus
//! normalization about the empty sequence: a bounded pseudometric, including
//! triangle inequality, on text after phonetic coding. Unlike division by the
//! maximum length, it preserves that property. Unrelated sequences cost deleting
//! and reinserting everything, hence distance 1. DP takes O(n*m) time and
//! O(min(n,m)) scratch space, with no allocation inside the DP loop.
//!
//! # Risk model
//! Normalization matches `commands::normalize`: lowercase alphanumerics, other
//! characters become word separators, whitespace collapsed. An exact normalized
//! other-target match always rejects at 1. Empty triggers also reject at 1.
//! Same-target phrases are excluded from *all* collision evidence. Ties select
//! the lexicographically smallest (normalized phrase, target, original phrase),
//! making reasons independent of input ordering.
//!
//! Otherwise combine independent heuristic evidence with noisy OR:
//! `score = 1 - (1-C)*(1-L)*(1-S)` (not a calibrated probability).
//! * `C = .94 * max(0, 1 - nearest_other_distance/.30)`; sound-identical
//!   other-target phrases reject, and close phonetic neighbors flag.
//! * A rank `r` has frequency prior `f = 1500/(1500+r)`. `L` is the maximum
//!   of `.88*f` for a known whole word, `.86*f` for a known whole bigram,
//!   and `.18*mean(word_f)/sqrt(word_count)` for a multiword variant. An unknown
//!   word has `f=0`. Common component words alone do not imply a common phrase:
//!   this deliberately keeps `years off` below the flag threshold.
//!   Normalization splits contractions ("here's" -> `here s`), so a
//!   contraction fragment (`s t re ll ve d m`) is rejoined with the word
//!   before it into one unit first: a unit that is a known bigram (`it s`,
//!   `don t`) counts with the bigram's frequency, any other with its head
//!   word's (`here s` as `here`). Words, counts and the mean above are over
//!   units, so a stray `s` no longer dilutes `here s that`.
//! * `S` is .64/.38/.18/.05/0 for <=2/3/4/5/>5 phonemes, plus .06 for at
//!   most one estimated syllable, capped at .75. Syllables are vowel runs in
//!   the key (minimum one for nonempty text). Thus very short novel triggers
//!   still flag; long unknown phrases are not penalized just for being unknown.
//!
//! Verdict thresholds: `Ok < .60`, `Flag >= .60`, `Reject >= .90`. Reasons
//! identify the largest evidence term and any nearest phonetic collision inside
//! the .30 neighborhood. Scores measure accidental-trigger risk, not whether
//! the variant is a plausible transcription of the intended canonical phrase.
//!
//! # Lexicon provenance and limitations
//! The word and bigram lists below were authored for this module (2026-11-13),
//! under the repository's MIT license. No dictionary or corpus was copied, no
//! network or runtime data files are needed. Word order supplies approximate
//! frequency ranks: function words, conversational vocabulary, then everyday
//! topics. These are *editorial priors*, not measured corpus frequencies or a
//! claim to the English top 5000. Bigrams likewise have editorial ranks. The
//! compact vocabulary is intentionally conservative; absent words/idioms can
//! be common in speech and should still receive human review. Tests preserve
//! a snapshot of the supplied live command lists without requiring that private
//! untracked fixture to exist in another checkout.

use std::collections::BTreeMap;

/// A configured phrase and the action/list it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct Phrase<'a> {
    pub text: &'a str,
    pub target: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Flag,
    Reject,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Risk {
    pub score: f32,
    pub verdict: Verdict,
    pub reason: String,
}

/// Immutable editorial frequency ranks; construction has no I/O.
#[derive(Debug, Clone)]
pub struct Lexicon {
    words: BTreeMap<&'static str, usize>,
    bigrams: BTreeMap<&'static str, usize>,
}

impl Lexicon {
    pub fn builtin() -> Lexicon {
        let mut words = BTreeMap::new();
        for (index, word) in COMMON_WORDS.split_whitespace().enumerate() {
            // First occurrence wins if future edits accidentally repeat a word.
            words.entry(word).or_insert(index + 1);
        }
        let bigrams = COMMON_BIGRAMS
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .enumerate()
            .map(|(index, phrase)| (phrase, index + 1))
            .collect();
        Lexicon { words, bigrams }
    }

    /// Whether `word` (normalized, one word) is in the lexicon.
    pub fn contains(&self, word: &str) -> bool {
        self.words.contains_key(word)
    }

    fn frequency(&self, word: &str) -> f32 {
        self.words.get(word).map_or(0.0, |&r| rank_frequency(r))
    }

    fn lexical_risk(&self, text: &str) -> f32 {
        let units = contraction_units(text);
        let whole = self
            .bigrams
            .get(text)
            .map_or(0.0, |&r| 0.86 * rank_frequency(r));
        if let [(_, head)] = units.as_slice() {
            return (0.88 * self.frequency(head)).max(whole);
        }
        let count = units.len() as f32;
        let sum: f32 = units
            .iter()
            .map(|(unit, head)| match self.bigrams.get(unit.as_str()) {
                Some(&r) if unit != head => rank_frequency(r),
                _ => self.frequency(head),
            })
            .sum();
        let components = 0.18 * sum / (count * count.sqrt());
        whole.max(components)
    }
}

/// What `commands::normalize` leaves of a contraction's tail: "here's" is
/// `here s`, "don't" `don t`, "we'll" `we ll`.
const CONTRACTION_FRAGMENTS: &[&str] = &["s", "t", "re", "ll", "ve", "d", "m"];

/// Words with contraction fragments rejoined to the word before them, each
/// as (unit, head word): `here s that` is [(`here s`, `here`), (`that`,
/// `that`)]. A fragment with no word before it stays a unit of its own.
fn contraction_units(text: &str) -> Vec<(String, &str)> {
    let mut units: Vec<(String, &str)> = Vec::new();
    for word in text.split_whitespace() {
        match units.last_mut() {
            Some((unit, _)) if CONTRACTION_FRAGMENTS.contains(&word) => {
                unit.push(' ');
                unit.push_str(word);
            }
            _ => units.push((word.to_string(), word)),
        }
    }
    units
}

fn rank_frequency(rank: usize) -> f32 {
    1500.0 / (1500.0 + rank as f32)
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut separated = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if separated && !out.is_empty() {
                out.push(' ');
            }
            out.push(c.to_lowercase().next().unwrap_or(c));
            separated = false;
        } else {
            separated = true;
        }
    }
    out
}

/// Return a diagnostic phoneme key; equality denotes approximate homophony.
pub fn phonetic_key(text: &str) -> String {
    key_normalized(&normalize(text))
}

fn key_normalized(text: &str) -> String {
    let mut key = String::with_capacity(text.len());
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if is_letter(word) && words.peek().is_some_and(|next| is_letter(next)) {
            let mut spelled = String::from(word);
            while words.peek().is_some_and(|next| is_letter(next)) {
                spelled.push_str(words.next().unwrap());
            }
            encode_word(&spelled, &mut key);
        } else {
            encode_word(word, &mut key);
        }
    }
    key
}

fn is_letter(word: &str) -> bool {
    word.len() == 1 && word.as_bytes()[0].is_ascii_alphabetic()
}

fn vowel(c: u8) -> bool {
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

fn encode_word(word: &str, out: &mut String) {
    // Keep unsupported scripts distinct: transliteration would need a lexicon.
    if !word.is_ascii() {
        out.push_str(word);
        return;
    }
    let irregular = match word {
        "ear" | "year" => Some("Ir"),
        "ears" | "years" => Some("Irz"),
        "hear" | "here" => Some("hIr"),
        "heard" | "herd" => Some("hErd"),
        "one" | "won" => Some("wUn"),
        "two" | "to" | "too" => Some("tU"),
        "you" => Some("yU"),
        "your" => Some("yOr"),
        "yourself" => Some("yOrsElf"),
        "are" | "our" => Some("Ar"),
        "of" => Some("Ov"),
        "have" => Some("hAv"),
        "give" => Some("gIv"),
        "said" => Some("sEd"),
        "does" => Some("dUz"),
        "was" => Some("wOz"),
        "is" => Some("Iz"),
        "the" => Some("TE"),
        "through" | "threw" => Some("TrU"),
        "though" => Some("TO"),
        "thought" => Some("TOt"),
        "rough" | "ruff" => Some("rUf"),
        "tough" | "tuff" => Some("tUf"),
        "enough" => Some("EnUf"),
        "cough" => Some("kOf"),
        "laugh" => Some("lAf"),
        "eight" | "ate" => Some("Et"),
        "eye" | "i" => Some("I"),
        _ => None,
    };
    if let Some(key) = irregular {
        out.push_str(key);
        return;
    }
    let bytes = word.as_bytes();
    let mut i = usize::from(
        ["kn", "gn", "pn", "wr", "ps"]
            .iter()
            .any(|s| word.starts_with(s)),
    );
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        let tail = &word[i..];
        let (sound, consumed) = if tail.starts_with("tion") || tail.starts_with("sion") {
            ("SEn", 4)
        } else if tail.starts_with("tch") {
            ("C", 3)
        } else if tail.starts_with("dge") {
            ("j", 3)
        } else if tail.starts_with("igh") {
            ("I", 3)
        } else if tail.starts_with("sch") {
            ("sk", 3)
        } else {
            match (c, next) {
                (b'c', b'h') => ("C", 2),
                (b's', b'h') => ("S", 2),
                (b't', b'h') => ("T", 2),
                (b'p', b'h') => ("f", 2),
                (b'c', b'k') => ("k", 2),
                (b'n', b'g') => ("N", 2),
                (b'q', b'u') => ("kw", 2),
                (b'w', b'h') => ("w", 2),
                (b'e', b'e' | b'a') | (b'i', b'e') => ("I", 2),
                (b'a', b'i' | b'y') | (b'e', b'i' | b'y') => ("E", 2),
                (b'o', b'o' | b'u') => ("U", 2),
                (b'o', b'a' | b'w') | (b'a', b'u' | b'w') => ("O", 2),
                (b'e', 0) if i > 1 && !vowel(bytes[i - 1]) => ("", 1),
                (b'b', 0) if i > 0 && bytes[i - 1] == b'm' => ("", 1),
                (b'g', b'h') if i > 0 && vowel(bytes[i - 1]) => ("", 2),
                (b'c', b'e' | b'i' | b'y') => ("s", 1),
                (b'g', b'e' | b'i' | b'y') => ("j", 1),
                (b'c' | b'q', _) => ("k", 1),
                (b'x', _) => ("ks", 1),
                (b'a', _) => ("A", 1),
                (b'e', _) => ("E", 1),
                (b'i', _) => ("I", 1),
                (b'o', _) => ("O", 1),
                (b'u', _) => ("U", 1),
                (b'y', _) if i > 0 => ("I", 1),
                _ => {
                    out.push(c as char);
                    // Geminate consonants normally have only one sound.
                    i += if next == c && !vowel(c) { 2 } else { 1 };
                    continue;
                }
            }
        };
        out.push_str(sound);
        i += consumed;
    }
}

fn sound_class(c: char) -> u8 {
    match c {
        'A' | 'E' | 'I' | 'O' | 'U' => 1,
        'p' | 'b' | 't' | 'd' | 'T' | 'k' | 'g' => 2,
        'f' | 'v' | 's' | 'z' | 'S' | 'h' => 3,
        'C' | 'j' => 4,
        'm' | 'n' | 'N' => 5,
        'r' | 'l' | 'w' | 'y' => 6,
        _ => 0,
    }
}

fn substitution(a: char, b: char) -> u32 {
    if a == b {
        return 0;
    }
    if matches!(
        (a, b),
        ('T', 'd')
            | ('d', 'T')
            | ('T', 't')
            | ('t', 'T')
            | ('p', 'b')
            | ('b', 'p')
            | ('t', 'd')
            | ('d', 't')
            | ('k', 'g')
            | ('g', 'k')
            | ('f', 'v')
            | ('v', 'f')
            | ('s', 'z')
            | ('z', 's')
    ) {
        return 4;
    }
    let class = sound_class(a);
    if class != 0 && class == sound_class(b) {
        if class == 1 {
            5
        } else {
            12
        }
    } else {
        40
    }
}

/// Symmetric normalized weighted edit distance: 0 is homophony, 1 unrelated.
pub fn phonetic_distance(a: &str, b: &str) -> f32 {
    if a == b {
        return 0.0;
    }
    let a: Vec<_> = phonetic_key(a).chars().collect();
    let b: Vec<_> = phonetic_key(b).chars().collect();
    distance(&a, &b)
}

fn distance(a: &[char], b: &[char]) -> f32 {
    if a == b {
        return 0.0;
    }
    if a.is_empty() || b.is_empty() {
        return 1.0;
    }
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    // Integer costs make swapped argument results bit-for-bit identical.
    let mut row: Vec<u64> = (0..=short.len()).map(|i| i as u64 * 20).collect();
    for (i, &x) in long.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = (i as u64 + 1) * 20;
        for (j, &y) in short.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 20)
                .min(row[j] + 20)
                .min(diagonal + u64::from(substitution(x, y)));
            diagonal = above;
        }
    }
    let d = row[short.len()] as f64;
    (2.0 * d / (20.0 * (a.len() as f64 + b.len() as f64) + d)) as f32
}

/// Assess accidental activation, excluding every phrase of the same target.
pub fn assess(heard: &str, target: &str, others: &[Phrase<'_>], lexicon: &Lexicon) -> Risk {
    let normalized = normalize(heard);
    if normalized.is_empty() {
        return Risk {
            score: 1.0,
            verdict: Verdict::Reject,
            reason: "empty trigger after normalization".into(),
        };
    }
    let key: Vec<_> = key_normalized(&normalized).chars().collect();
    // Store the nearest phrase, with a stable textual tie-break, not input order.
    let mut nearest: Option<(f32, String, &Phrase<'_>)> = None;
    let mut exact: Option<&Phrase<'_>> = None;
    for phrase in others.iter().filter(|p| p.target != target) {
        let other = normalize(phrase.text);
        if other.is_empty() {
            continue;
        }
        if other == normalized {
            if exact.is_none_or(|old| (phrase.target, phrase.text) < (old.target, old.text)) {
                exact = Some(phrase);
            }
            continue;
        }
        let other_key: Vec<_> = key_normalized(&other).chars().collect();
        let d = distance(&key, &other_key);
        if nearest.as_ref().is_none_or(|(best, text, old)| {
            d < *best
                || (d == *best
                    && (other.as_str(), phrase.target, phrase.text)
                        < (text.as_str(), old.target, old.text))
        }) {
            nearest = Some((d, other, phrase));
        }
    }
    if let Some(phrase) = exact {
        return Risk {
            score: 1.0,
            verdict: Verdict::Reject,
            reason: format!(
                "exact collision with {:?} (target {:?})",
                phrase.text, phrase.target
            ),
        };
    }
    let collision = nearest
        .as_ref()
        .map_or(0.0, |(d, _, _)| 0.94 * (1.0 - d / 0.30).max(0.0));
    let lexical = lexicon.lexical_risk(&normalized);
    let syllables = key
        .iter()
        .enumerate()
        .filter(|&(i, &c)| sound_class(c) == 1 && (i == 0 || sound_class(key[i - 1]) != 1))
        .count()
        .max(1);
    let short: f32 = match key.len() {
        0..=2 => 0.64,
        3 => 0.38,
        4 => 0.18,
        5 => 0.05,
        _ => 0.0,
    };
    let short = (short + if syllables <= 1 { 0.06 } else { 0.0 }).min(0.75);
    let score = (1.0 - (1.0 - collision) * (1.0 - lexical) * (1.0 - short)).clamp(0.0, 1.0);
    let verdict = if score >= 0.90 {
        Verdict::Reject
    } else if score >= 0.60 {
        Verdict::Flag
    } else {
        Verdict::Ok
    };
    let mut reason = if collision > 0.0 && collision >= lexical && collision >= short {
        format!("phonetic collision dominates ({collision:.3})")
    } else if lexical > 0.0 && lexical >= short {
        format!("common speech dominates ({lexical:.3} lexical evidence)")
    } else if short > 0.0 {
        format!(
            "short trigger dominates ({} phonemes, {syllables} estimated syllables)",
            key.len()
        )
    } else {
        "low evidence: distinctive length and no common whole phrase".to_string()
    };
    if let Some((d, _, phrase)) = nearest.filter(|(d, _, _)| *d < 0.30) {
        reason.push_str(&format!(
            "; nearest {:?} (target {:?}, distance {d:.3})",
            phrase.text, phrase.target
        ));
    }
    Risk {
        score,
        verdict,
        reason,
    }
}

// Original, approximate descending-frequency vocabulary, MIT; see module docs.
const COMMON_WORDS: &str = "
the of and to a in is it you that i for on with was as this be are not have at or he we
they but his from by she an will my one all would there their what so up out if about
who get which go me when make can like time no just him know take people into year your
good some could them see other than then now look only come its over think also back
after use two how our work first well way even new want because any these give day most
us yourself myself himself herself itself themselves ourselves yourselves each much very
here where those down should may being been do does did done had has having said say
says something anything nothing everything someone anyone everyone nobody everybody
somewhere anywhere everywhere never always often sometimes again once still already yet
really right left before off through under between while last long little own same old
great too more many such another why must might need try feel call put tell ask help
talk turn start show hear play run move live believe bring happen write read sit stand
lose pay meet include continue set learn change lead understand watch follow stop create
speak allow add spend grow open walk win offer remember love consider appear buy wait
serve die send expect build stay fall cut reach remain suggest raise pass sell require
report decide pull return explain hope develop carry break receive agree support hit
produce eat cover catch draw choose cause point listen thank leave keep hold let begin
end find seem become mean saw went got made thought knew took came gave found told felt
heard tried used wanted needed worked liked called asked looked going getting doing
making coming looking taking talking working trying saying thinking seeing waiting
please thanks yes yeah okay ok sure hello hi hey goodbye sorry welcome fine true false
maybe perhaps probably actually basically especially exactly almost enough quite rather
pretty simply both either neither few less least every whole together apart around
across along above below behind near far away home house room door window floor wall bed
table chair kitchen bathroom water food bread milk tea coffee sugar salt rice meat
chicken fish egg eggs fruit apple orange banana vegetable potato tomato onion cheese
butter soup breakfast lunch dinner drink taste hungry thirsty hot cold warm cool man
woman child children boy girl baby family friend mother father parent brother sister son
daughter wife husband person name life world country city town village street road car
bus train plane bike boat travel trip place school student teacher class book paper pen
pencil story word words sentence language question answer idea problem number part side
hand eye head face hair body arm leg foot feet mouth nose ear ears heart mind voice
sound music song movie game picture photo news phone computer screen light night morning
evening today tomorrow yesterday week month years hour minute second moment early late
soon later since until during without within toward against among beyond although unless
whether however therefore instead otherwise indeed anyway else elsewise whatever whoever
whenever wherever whose whom alone happy sad angry tired afraid worried sick healthy
strong weak young small large big short tall high low full empty hard easy simple
difficult important different certain possible available ready free busy clear dark
bright white black red blue green yellow brown pink purple gray grey beautiful nice bad
better best worse worst correct wrong real special common public private local national
international personal social human political economic natural physical general
particular similar recent present future past next previous current final usual normal
entire likely able unable safe dangerous necessary useful interesting serious funny
lovely wonderful terrible perfect poor rich expensive cheap clean dirty fresh dry wet
soft heavy deep wide narrow close fast slow quick quiet loud careful comfortable kind
honest fair unfair lucky friendly strange weird amazing surprised excited boring bored
calm nervous asleep awake sleep dream rest wake lie fell slept ran sat stood business
company office job money price cost value dollar pound euro cent bank cash credit card
bill tax budget market shop store sale product service customer client order purchase
stock supply demand trade deal account project plan meeting team group member leader
manager employee worker owner boss colleague partner organization community society
government president police law court judge lawyer crime war peace army soldier power
force control rule policy decision choice reason result effect case fact truth evidence
example information knowledge experience education history science research study test
experiment method system process program code software hardware network internet website
email message letter text list file folder document data database server device machine
engine tool equipment model design version update issue error bug fix command action
option mode key button keyboard mouse cursor input output audio video recording record
volume microphone speaker camera connection signal address port channel radio television
broadcast online password security access permission user login logout restart shutdown
cancel accept reject confirm delete remove copy paste save load search select check edit
undo redo enter exit quit line literal space tab shift escape backtrack track trace
direction position location map path north south east west front rear top bottom middle
center centre corner inside outside surface edge ground sky air earth sun moon star
cloud rain snow ice wind storm weather summer winter spring autumn season fire smoke
heat energy electric electricity gas oil coal metal wood stone glass plastic rubber
cloth cotton leather wool material gold silver iron steel copper color colour shape size
weight height width length depth distance speed area level rate amount total sum half
quarter third double pair single million thousand hundred ten twenty thirty forty fifty
sixty seventy eighty ninety zero three four five six seven eight nine eleven twelve
thirteen fourteen fifteen sixteen seventeen eighteen nineteen twice daily weekly monthly
annual date calendar schedule appointment event party holiday birthday wedding
celebration vacation weekend monday tuesday wednesday thursday friday saturday sunday
january february march april june july august september october november december
century age adult teenager infant youth generation neighbor neighbour visitor guest host
crowd audience population citizen resident stranger gentleman lady male female married
relationship friendship conversation discussion argument debate agreement promise trust
respect care concern attention interest feeling emotion happiness sadness anger fear
pain pleasure joy surprise shame pride regret fault mistake trouble danger risk chance
luck success failure progress effort practice skill ability talent quality detail
feature purpose goal task duty responsibility opportunity challenge solution benefit
advantage disadvantage loss gain profit growth improvement increase decrease difference
comparison balance average standard measure measurement limit range scale degree
temperature pressure movement activity exercise sport ball football soccer tennis
basketball baseball swimming running walking dancing singing reading writing drawing
cooking cleaning shopping driving riding flying fishing gardening camping hiking health
doctor nurse hospital medicine treatment patient disease illness injury infection fever
cough breath breathing blood bone skin muscle stomach tooth teeth throat neck shoulder
chest finger thumb knee ankle brain lung liver surgery operation emergency ambulance
pharmacy pill tablet drug dose vaccine virus bacteria symptom recovery fit ill hurt dead
alive born birth death funeral grave church religion faith belief god spirit soul heaven
hell prayer tradition culture art artist painting sculpture theatre theater performance
concert instrument piano guitar drum violin rhythm melody note tune dance film actor
actress stage scene character plot chapter novel poem poetry magazine newspaper article
page title author reader writer editor publisher library museum exhibition gallery
ticket seat row queue entrance admission apartment building roof ceiling stairs elevator
lift basement garden yard fence gate garage driveway porch balcony bedroom living dining
hallway sink tap faucet bath shower toilet towel soap brush comb mirror blanket pillow
sheet mattress carpet curtain cushion sofa couch desk shelf cabinet cupboard drawer
wardrobe closet lamp clock vase dish plate bowl cup mug bottle jar pot pan oven stove
cooker fridge freezer dishwasher laundry washing dryer vacuum broom mop bucket bin trash
rubbish waste bag box package parcel envelope stamp post mail delivery shipping
transport truck van taxi cab motorcycle bicycle wheel tire tyre brake gear pedal
seatbelt petrol fuel battery charger cable wire socket plug switch circuit voltage motor
repair maintenance accident traffic route journey station airport terminal platform
passenger driver pilot captain crew flight departure arrival destination hotel motel
hostel reservation reception luggage suitcase backpack passport visa border customs
tourist tour guide beach coast ocean sea lake river stream pond waterfall island
mountain hill valley forest woods tree leaf branch root flower grass plant seed soil
sand rock pebble mud clay field farm farmer crop wheat corn barley flour grain oat nut
peanut almond walnut bean pea lentil lettuce cabbage carrot broccoli pepper cucumber
mushroom spinach garlic ginger cinnamon spice herb basil parsley mint lemon lime peach
pear plum cherry grape melon berry strawberry raspberry blueberry pineapple coconut
mango juice soda beer wine cider alcohol honey jam jelly cake pie biscuit cookie
chocolate candy sweet dessert pudding cream yogurt yoghurt sandwich toast pasta noodle
pizza burger sausage bacon beef pork lamb turkey duck seafood shrimp crab lobster salmon
tuna trout oyster knife fork spoon chopstick recipe ingredient portion meal snack menu
restaurant cafe bar pub waiter waitress chef baker butcher brunch supper delicious
bitter sour salty spicy mild raw cooked fried baked boiled grilled roasted frozen pet
dog cat horse cow sheep pig goat bird goose hen rooster rabbit hamster rat squirrel deer
fox wolf bear lion tiger elephant giraffe zebra monkey ape gorilla snake frog turtle
lizard insect ant bee wasp fly mosquito butterfly spider worm beetle snail whale dolphin
shark seal penguin eagle owl crow pigeon sparrow parrot feather fur tail wing paw claw
nest cage wild domestic animal nature environment habitat climate pollution recycle
conservation resource renewable sustainable solar turbine planet universe galaxy orbit
satellite rocket astronaut gravity atom molecule chemical reaction oxygen hydrogen
carbon nitrogen acid base biology chemistry physics mathematics algebra geometry
calculation equation fraction decimal percent percentage ratio quantity density mass
solid liquid vapor steam fluid wave frequency vibration spectrum radiation reflection
shadow angle curve circle square triangle rectangle polygon sphere cube cylinder cone
pattern texture smooth rough flat round straight bent crooked horizontal vertical
parallel opposite equal unequal positive negative neutral exact approximate precise
accurate random regular unusual rare frequent occasional constant continuous temporary
permanent stable flexible rigid loose tight thin thick dense sparse sharp blunt pointed
broken damaged intact complete unfinished finished prepared organized planned unexpected
sudden gradual rapid steady gentle violent peaceful active passive polite rude generous
selfish humble proud brave shy timid confident doubtful uncertain curious creative
intelligent clever stupid wise foolish sensible practical logical reasonable responsible
reliable impatient tolerant strict firm stubborn willing reluctant eager lazy energetic
cheerful miserable lonely popular famous unknown familiar foreign native official
informal formal casual professional technical scientific medical legal financial
educational cultural religious moral ethical dishonest loyal grateful thankful helpful
harmful innocent guilty valid invalid illegal useless meaningful meaningless visible
invisible obvious hidden secret closed locked unlocked connected disconnected attached
detached separate united divided split joined mixed combined sorted ordered numbered
labeled named saved lost missing absent included excluded enabled disabled installed
uninstalled started stopped paused resumed completed failed passed approved denied
allowed forbidden required optional recommended preferred chosen selected unavailable
supported unsupported recognized unrecognized understood misunderstood remembered
forgotten repeated spoken written printed typed displayed recorded played listened
watched visited traveled arrived departed entered exited returned moved placed picked
dropped thrown caught pushed pulled lifted lowered raised carried sent received brought
bought sold paid earned spent borrowed lent owned rented shared exchanged replaced
restored recovered removed added changed adjusted corrected improved reduced expanded
extended shortened lengthened widened narrowed increased decreased counted measured
weighed compared tested checked verified validated reviewed assessed evaluated analyzed
examined explored discovered invented developed designed built created generated
produced manufactured assembled configured managed maintained monitored controlled
operated handled processed stored transferred transmitted uploaded downloaded published
announced reported informed notified warned advised instructed taught trained learned
studied practiced rehearsed attempted succeeded achieved accomplished concluded ended
settled solved resolved decided agreed accepted refused rejected declined offered
suggested proposed requested demanded answered replied responded explained described
expressed mentioned stated claimed argued discussed debated negotiated persuaded
convinced encouraged helped assisted served provided supplied delivered fulfilled
satisfied pleased delighted amused entertained interested shocked frightened scared
concerned bothered annoyed irritated frustrated disappointed confused embarrassed
ashamed exhausted relaxed rested healed cured protected defended rescued escaped
survived lived died existed appeared disappeared occurred happened lasted continued
remained stayed waited delayed postponed canceled cancelled scheduled arranged hosted
attended participated contributed cooperated collaborated competed won defeated missed
reached approached followed led guided directed accompanied met greeted welcomed
introduced invited thanked apologized forgave blamed praised criticized judged respected
trusted believed doubted suspected hoped wished expected anticipated imagined dreamed
wondered realized noticed observed perceived sensed touched tasted smelled viewed
focused concentrated considered reasoned calculated estimated predicted forecast guessed
assumed supposed inferred identified distinguished classified categorized grouped listed
summarized outlined detailed illustrated demonstrated represented symbolized meant
implied indicated signaled reflected revealed showed proved confirmed contradicted
opposed resisted prevented avoided blocked limited restricted constrained permitted
caused triggered initiated launched activated deactivated switched toggled pressed
released clicked tapped scrolled dragged zoomed resized minimized maximized
";

const COMMON_BIGRAMS: &str = "
of the
in the
to the
on the
and the
for the
at the
with the
from the
it is
i am
you are
that is
this is
there is
we are
they are
i think
i know
i mean
you know
do you
are you
can you
will you
would you
could you
thank you
thanks for
of course
all right
right now
as well
so much
very much
a lot
a little
kind of
sort of
some of
one of
a few
each other
no problem
no worries
not sure
i guess
i see
i understand
i want
i need
we need
you can
you have
it was
it has
it will
we have
they have
have been
has been
will be
would be
could be
should be
going to
want to
need to
used to
have to
had to
able to
out of
up to
because of
instead of
rather than
more than
less than
as soon
such as
even though
as if
what if
what about
how about
how much
how many
so that
in fact
in case
in order
for example
for instance
at least
at all
at home
at work
last night
last time
this morning
this time
next time
next week
last week
every day
all day
all night
good morning
good night
good evening
good luck
take care
see you
excuse me
sorry about
come on
hang on
hold on
look at
look like
looks like
sounds like
feel like
feels like
make sure
let me
let us
let s
don t
can t
won t
i m
you re
it s
that s
we re
they re
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contraction_fragments_rejoin_their_word() {
        assert_eq!(
            contraction_units("here s that"),
            vec![("here s".to_string(), "here"), ("that".to_string(), "that")]
        );
        assert_eq!(
            contraction_units("don t"),
            vec![("don t".to_string(), "don")]
        );
        assert_eq!(
            contraction_units("s on"),
            vec![("s".to_string(), "s"), ("on".to_string(), "on")]
        );
        let lexicon = Lexicon::builtin();
        // What the stray fragment used to leave: every word counted alone.
        let diluted = |text: &str| {
            let words: Vec<&str> = text.split_whitespace().collect();
            let n = words.len() as f32;
            0.18 * words.iter().map(|w| lexicon.frequency(w)).sum::<f32>() / (n * n.sqrt())
        };
        for text in ["here s that", "it s on"] {
            let now = lexicon.lexical_risk(text);
            assert!(
                now > diluted(text) * 1.3,
                "{text}: {now} vs {}",
                diluted(text)
            );
        }
        // As common as the same words without the contraction.
        let here = lexicon.lexical_risk("here s that");
        assert!(
            (here - lexicon.lexical_risk("here that")).abs() < 1e-6,
            "{here}"
        );
        assert!(lexicon.lexical_risk("don t") > 0.5);
        assert_eq!(
            lexicon.lexical_risk("don t"),
            0.86 * rank_frequency(lexicon.bigrams["don t"])
        );
        // No fragment: unchanged.
        assert_eq!(lexicon.lexical_risk("years off"), {
            let f = lexicon.frequency("years") + lexicon.frequency("off");
            (0.18 * f / (2.0 * 2f32.sqrt())).max(0.0)
        });
    }

    // Snapshot of .lists-fixture.toml [commands], supplied 2026-11-13.
    // Keep this self-contained: the private fixture is deliberately not committed.
    const LIVE_LISTS: &[(&str, &[&str])] = &[
        ("enter", &["enter", "over"]),
        (
            "sign_off",
            &[
                "over and out",
                "peace",
                "peace out",
                "piece out",
                "ears out",
                "years out",
            ],
        ),
        ("new_line", &["new line"]),
        ("undo", &["scratch that"]),
        ("back", &["back"]),
        ("backtrack", &["backtrack", "back track"]),
        ("standby", &["ears vad", "ears v a d"]),
        (
            "wake",
            &[
                "ears on",
                "listen clanker",
                "hey clanker",
                "hay clanker",
                "h e y clanker",
                "hey klanker",
                "clanker",
                "klanker",
                "hey glinker",
                "hey clinker",
                "hey clunker",
                "hey klinker",
                "adsense",
                "ad sense",
                "ads on",
            ],
        ),
        (
            "sleep",
            &[
                "ears off",
                "years off",
                "fuck off",
                "f off",
                "fuck of",
                "yourself",
                "fuck off clanker",
                "fuck off klanker",
                "fuck off clunker",
                "f off clanker",
                "fuck off kleiner",
                "fuck off klecker",
                "fuck off klinker",
            ],
        ),
        ("literal", &["literal"]),
    ];

    fn live_phrases() -> Vec<Phrase<'static>> {
        LIVE_LISTS
            .iter()
            .flat_map(|(target, texts)| texts.iter().map(|text| Phrase { text, target }))
            .collect()
    }

    #[test]
    fn live_list_calibration() {
        let phrases = live_phrases();
        assert_eq!(phrases.len(), 44);
        let lexicon = Lexicon::builtin();
        for (heard, target, expected) in [
            ("yourself", "sleep", Verdict::Flag),
            ("years off", "sleep", Verdict::Ok),
            ("adsense", "wake", Verdict::Ok),
            ("back track", "backtrack", Verdict::Ok),
            ("enter", "sleep", Verdict::Reject),
        ] {
            let risk = assess(heard, target, &phrases, &lexicon);
            println!(
                "{heard:?} | {target} | {:.6} | {:?} | {}",
                risk.score, risk.verdict, risk.reason
            );
            assert_eq!(risk.verdict, expected, "{heard}: {risk:?}");
            if heard == "yourself" {
                assert!(risk.reason.contains("common speech"));
            }
        }
    }

    #[test]
    fn every_other_target_exact_match_rejects() {
        let phrases = live_phrases();
        let lexicon = Lexicon::builtin();
        for phrase in &phrases {
            for (target, _) in LIVE_LISTS.iter().filter(|(t, _)| *t != phrase.target) {
                let decorated = format!("  {}!!!  ", phrase.text.to_uppercase());
                let risk = assess(&decorated, target, &phrases, &lexicon);
                assert_eq!(risk.score, 1.0, "{decorated} -> {target}");
                assert_eq!(risk.verdict, Verdict::Reject);
                assert!(risk.reason.contains("exact collision"));
                assert!(risk.reason.contains(phrase.text));
            }
        }
    }

    #[test]
    fn all_same_target_phrases_are_ignored() {
        let lexicon = Lexicon::builtin();
        for (target, texts) in LIVE_LISTS {
            let others: Vec<_> = texts.iter().map(|text| Phrase { text, target }).collect();
            for heard in texts.iter().copied().chain(["scratch dat", "hello", "x"]) {
                assert_eq!(
                    assess(heard, target, &others, &lexicon),
                    assess(heard, target, &[], &lexicon)
                );
            }
        }
    }

    #[test]
    fn same_target_does_not_mask_another_targets_collision() {
        let lexicon = Lexicon::builtin();
        let phrases = [
            Phrase {
                text: "ears on",
                target: "wake",
            },
            Phrase {
                text: "ears on",
                target: "sleep",
            },
        ];
        let risk = assess("ears on", "wake", &phrases, &lexicon);
        assert_eq!(risk.verdict, Verdict::Reject);
        assert!(risk.reason.contains("sleep"));
    }

    #[test]
    fn near_collision_flags_and_homophone_rejects() {
        let lexicon = Lexicon::builtin();
        let others = [Phrase {
            text: "scratch that",
            target: "undo",
        }];
        let risk = assess("scratch dat", "sleep", &others, &lexicon);
        assert_eq!(risk.verdict, Verdict::Flag, "{risk:?}");
        assert!(risk.reason.contains("phonetic collision"));
        assert!(risk.reason.contains("scratch that"));
        let others = [Phrase {
            text: "peace out",
            target: "sign_off",
        }];
        let risk = assess("piece out", "sleep", &others, &lexicon);
        assert_eq!(risk.verdict, Verdict::Reject, "{risk:?}");
        assert!(risk.reason.contains("peace out"));
    }

    #[test]
    fn common_words_and_bigrams_flag_without_other_targets() {
        let lexicon = Lexicon::builtin();
        for heard in [
            "yourself",
            "hello",
            "the",
            "please",
            "of course",
            "thank you",
            "you know",
            "it is",
            "don't",
        ] {
            let risk = assess(heard, "sleep", &[], &lexicon);
            assert_ne!(risk.verdict, Verdict::Ok, "{heard}: {risk:?}");
            assert!(risk.reason.contains("common speech"), "{risk:?}");
        }
        assert_eq!(
            assess("years off", "sleep", &[], &lexicon).verdict,
            Verdict::Ok
        );
        assert!(
            assess("of course", "sleep", &[], &lexicon).score
                > assess("course of", "sleep", &[], &lexicon).score
        );
    }

    #[test]
    fn short_unknown_triggers_are_riskier() {
        let lexicon = Lexicon::builtin();
        let short = assess("xz", "run:test", &[], &lexicon);
        let tiny = assess("z", "run:test", &[], &lexicon);
        let long = assess("zorbax flindor", "run:test", &[], &lexicon);
        assert!(short.score > long.score);
        assert_eq!(tiny.verdict, Verdict::Flag);
        assert!(tiny.reason.contains("short trigger"));
        assert_eq!(long.verdict, Verdict::Ok);
    }

    #[test]
    fn empty_variants_reject_but_empty_other_phrases_are_ignored() {
        let lexicon = Lexicon::builtin();
        for heard in ["", " ", "!!!", "💬"] {
            let risk = assess(heard, "wake", &[], &lexicon);
            assert_eq!(risk.score, 1.0);
            assert_eq!(risk.verdict, Verdict::Reject);
            assert!(risk.reason.contains("empty"));
        }
        let others = [Phrase {
            text: "...",
            target: "sleep",
        }];
        assert_eq!(
            assess("years off", "wake", &others, &lexicon),
            assess("years off", "wake", &[], &lexicon)
        );
    }

    #[test]
    fn phonetic_equivalences() {
        for (a, b) in [
            ("back track", "backtrack"),
            ("v a d", "vad"),
            ("ears v a d", "ears vad"),
            ("h e y clanker", "hey clanker"),
            ("hay clanker", "hey klanker"),
            ("ad sense", "adsense"),
            ("peace out", "piece out"),
            ("years off", "ears off"),
            ("night", "knight"),
            ("write", "right"),
            ("phone", "fone"),
            ("rough", "ruff"),
            ("one", "won"),
            ("eight", "ate"),
            (" EARS, ON! ", "ears on"),
        ] {
            assert_eq!(phonetic_key(a), phonetic_key(b), "{a} / {b}");
            assert_eq!(phonetic_distance(a, b), 0.0);
        }
    }

    #[test]
    fn confusable_sounds_cost_less_than_unrelated_sounds() {
        assert!(phonetic_distance("pat", "bat") < phonetic_distance("pat", "mat"));
        assert!(phonetic_distance("bet", "bit") < phonetic_distance("bet", "botch"));
        assert!(phonetic_distance("fan", "van") < phonetic_distance("fan", "man"));
        assert!(phonetic_distance("clanker", "klinker") < phonetic_distance("clanker", "yourself"));
        assert!(phonetic_distance("ears on", "ears off") > 0.0);
        assert_eq!(phonetic_distance("m", "s"), 1.0);
    }

    #[test]
    fn punctuation_and_unicode_are_normalized_without_data_loss() {
        for text in [
            "  EARS---ON!",
            "don't",
            "İSTANBUL",
            "éclair",
            "你好",
            "foo\nbar\t1",
            "💬 ",
        ] {
            assert_eq!(normalize(text), crate::commands::normalize(text));
        }
        assert_ne!(phonetic_key("你好"), phonetic_key("世界"));
        assert_ne!(phonetic_key("é"), phonetic_key("e"));
        assert_ne!(phonetic_key("1"), phonetic_key("2"));
        assert_eq!(phonetic_distance("", "你好"), 1.0);
    }

    #[test]
    fn distance_identity_symmetry_range_and_triangle() {
        let samples = [
            "",
            "a",
            "b",
            "ab",
            "ba",
            "aba",
            "baa",
            "ears on",
            "ears off",
            "years off",
            "yourself",
            "hey clanker",
            "hey glinker",
            "peace out",
            "piece out",
            "back track",
            "backtrack",
            "v a d",
            "vad",
            "scratch that",
            "scratch dat",
            "xyz",
            "123",
            "Été",
            "你好",
        ];
        let distances: Vec<Vec<f32>> = samples
            .iter()
            .map(|a| samples.iter().map(|b| phonetic_distance(a, b)).collect())
            .collect();
        for i in 0..samples.len() {
            assert_eq!(distances[i][i], 0.0);
            for j in 0..samples.len() {
                assert!((0.0..=1.0).contains(&distances[i][j]));
                assert_eq!(distances[i][j], distances[j][i]);
                for k in 0..samples.len() {
                    assert!(
                        distances[i][k] <= distances[i][j] + distances[j][k] + 1e-6,
                        "triangle: {:?}, {:?}, {:?}",
                        samples[i],
                        samples[j],
                        samples[k]
                    );
                }
            }
        }
    }

    #[test]
    fn substitution_costs_are_metric() {
        let alphabet: Vec<_> = "AEIOUpbtdkgfvszSThCjmnNrlwy012é".chars().collect();
        for &a in &alphabet {
            assert_eq!(substitution(a, a), 0);
            for &b in &alphabet {
                assert_eq!(substitution(a, b), substitution(b, a));
                assert!(substitution(a, b) <= 40); // Via deletion and insertion.
                for &c in &alphabet {
                    assert!(substitution(a, c) <= substitution(a, b) + substitution(b, c));
                }
            }
        }
    }

    #[test]
    fn assessment_is_bounded_deterministic_and_order_independent() {
        let lexicon = Lexicon::builtin();
        let phrases = live_phrases();
        let mut reversed = phrases.clone();
        reversed.reverse();
        for phrase in &phrases {
            let risk = assess(phrase.text, phrase.target, &phrases, &lexicon);
            assert!(risk.score.is_finite() && (0.0..=1.0).contains(&risk.score));
            assert_eq!(risk, assess(phrase.text, phrase.target, &phrases, &lexicon));
            assert_eq!(
                risk,
                assess(phrase.text, phrase.target, &reversed, &lexicon)
            );
            let expected = if risk.score >= 0.90 {
                Verdict::Reject
            } else if risk.score >= 0.60 {
                Verdict::Flag
            } else {
                Verdict::Ok
            };
            assert_eq!(risk.verdict, expected);
        }
        for heard in ["ears on", "EARS ON!", "earz on"] {
            let mut ties = [
                Phrase {
                    text: "ears on",
                    target: "z",
                },
                Phrase {
                    text: "EARS ON!",
                    target: "a",
                },
                Phrase {
                    text: "ears on",
                    target: "a",
                },
            ];
            let risk = assess(heard, "other", &ties, &lexicon);
            ties.reverse();
            assert_eq!(risk, assess(heard, "other", &ties, &lexicon));
        }
    }

    #[test]
    fn lexicon_has_unique_ranked_words_and_bigrams() {
        let lexicon = Lexicon::builtin();
        assert!(lexicon.words.len() >= 1500);
        assert_eq!(lexicon.words.len(), COMMON_WORDS.split_whitespace().count());
        assert!(lexicon.bigrams.len() >= 100);
        assert_eq!(
            lexicon.bigrams.len(),
            COMMON_BIGRAMS
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
        );
        assert!(lexicon.frequency("the") > lexicon.frequency("yourself"));
        assert!(lexicon.frequency("yourself") > lexicon.frequency("turbine"));
        assert_eq!(lexicon.frequency("zorbax"), 0.0);
        assert!(COMMON_WORDS.len() + COMMON_BIGRAMS.len() < 200_000);
        for (&word, &rank) in &lexicon.words {
            assert_eq!(normalize(word), word);
            assert!(rank > 0);
        }
        println!(
            "lexicon: {} words, {} bigrams, {} embedded bytes",
            lexicon.words.len(),
            lexicon.bigrams.len(),
            COMMON_WORDS.len() + COMMON_BIGRAMS.len()
        );
    }

    #[test]
    fn distance_microbenchmark_10k_calls() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};
        let pairs = [
            ("hey little clanker", "hay little glinker"),
            ("please scratch that", "please scratch dat"),
            ("turn ears off", "turn years on"),
        ];
        let start = Instant::now();
        for i in 0..10_000 {
            let (a, b) = pairs[i % pairs.len()];
            black_box(phonetic_distance(black_box(a), black_box(b)));
        }
        let elapsed = start.elapsed();
        println!(
            "10,000 three-word distances: {elapsed:?}, {:.0} calls/s (debug assertions: {})",
            10_000.0 / elapsed.as_secs_f64(),
            cfg!(debug_assertions)
        );
        // A regression guard, not a sub-second guarantee on shared CI hardware.
        assert!(
            elapsed < Duration::from_secs(30),
            "10k calls took {elapsed:?}"
        );
    }
}

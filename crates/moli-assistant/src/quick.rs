//! Quick orders: the house's everyday commands (« ferme les stores du salon »,
//! « mets la clim à 22 », « pause ») understood **without a model**.
//!
//! A sentence is cleaned, cut into clauses and matched against the house's own
//! inventory (device names, rooms, what each value can do). Nothing is guessed:
//! when the sentence is a question, a negation, a delayed or conditional order,
//! concerns the alarm or a door, or when a device is unknown or ambiguous, it is
//! **passed on** (to the model, or « je n'ai pas compris ») and nothing is done.
//!
//! The words (verbs, nouns, phrases) live in the catalogues
//! (`assistant.quick.*`), the house's rooms and their aliases in `home.json`:
//! this file knows no word of any language and no device of any house.

use std::collections::{BTreeMap, BTreeSet};

use moli_core::{DeviceId, Kind as PointKind, PointId, PointSpec, Semantic, Value};
use moli_runtime::DeviceView;

use crate::Config;
use crate::house::{Layout, name_of, no_room, norm};
use crate::word_list;

/// More than this many orders from one sentence is not a sentence.
const MAX_ORDERS: usize = 24;
/// A relative change of the volume, in points.
const VOLUME_STEP: f64 = 10.0;
/// A relative change of a light, in percent.
const LIGHT_STEP: f64 = 20.0;
/// A fuzzy correction needs words at least this long, and this similar.
const FUZZY_MIN_LEN: usize = 4;
const FUZZY_MIN_SIMILARITY: f64 = 0.8;

/// One order to give, under the guard.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Order {
    pub point: PointId,
    pub value: Value,
    pub device: String,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Understood: these orders.
    Run(Vec<Order>),
    /// The person ends the conversation.
    End,
    /// A question about the state of the house, answered from what Moli
    /// knows: the sentence to say and the devices it is about.
    Say {
        text: String,
        devices: Vec<DeviceId>,
    },
    /// Not sure: nothing is done. The reason is for the history.
    Pass(&'static str),
}

// ------------------------------------------------------------------ the words

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cat {
    Cover,
    Light,
    Clim,
    Speaker,
    Tv,
    /// A house helper (a switch that stands for a channel, the TV…).
    Helper,
    /// The alarm, a garage door: never by voice.
    Sensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    On,
    Off,
    Open,
    Close,
    Stop,
    Set,
    Play,
}

type Phrase = Vec<String>;

#[derive(Debug, Default)]
struct Lex {
    verbs: Vec<(Verb, Vec<Phrase>)>,
    step_up: Vec<Phrase>,
    step_down: Vec<Phrase>,
    nouns: Vec<(Cat, Vec<Phrase>)>,
    polite: Vec<Phrase>,
    filler: Vec<Phrase>,
    sensitive: Vec<(Phrase, Option<Phrase>)>,
    negation: Vec<Phrase>,
    deferred: Vec<Phrase>,
    time_units: BTreeSet<String>,
    question_start: Vec<Phrase>,
    question_any: Vec<Phrase>,
    end: Vec<Phrase>,
    everywhere: Vec<Phrase>,
    group_all: Vec<Phrase>,
    half: Vec<Phrase>,
    full: Vec<Phrase>,
    minimum: Vec<Phrase>,
    up: Vec<Phrase>,
    down: Vec<Phrase>,
    up_light: Vec<Phrase>,
    down_light: Vec<Phrase>,
    down_always: Vec<Phrase>,
    pause: Vec<Phrase>,
    resume: Vec<Phrase>,
    next: Vec<Phrase>,
    previous: Vec<Phrase>,
    mute: Vec<Phrase>,
    unmute: Vec<Phrase>,
    degree: BTreeSet<String>,
    apps: Vec<(Phrase, String)>,
    channels: Vec<(Phrase, String)>,
    mode_words: Vec<(Phrase, String)>,
    music_intro: Vec<Phrase>,
    music_end: BTreeSet<String>,
    clause_sep: BTreeSet<String>,
    stopwords: BTreeSet<String>,
    spec_alias: BTreeMap<String, String>,
    after_verb: BTreeSet<String>,
    person_prefix: Phrase,
    person_forms: Vec<String>,
    numbers: BTreeMap<String, i64>,
    join: BTreeSet<String>,
    one_after_join: BTreeSet<String>,
    x20: BTreeSet<String>,
    twenty: BTreeSet<String>,
    not_after: BTreeSet<String>,
    ask_temperature: Vec<Phrase>,
    ask_whats_on: Vec<Phrase>,
    garage_words: Vec<Phrase>,
    alarm_words: Vec<Phrase>,
    garage_states: Vec<(Phrase, String)>,
    alarm_states: Vec<(Phrase, String)>,
}

/// Case, accents, hyphens and apostrophes don't count; `,` and `?` stand alone.
fn tokens(text: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(text.len() + 8);
    for c in norm(text).chars() {
        match c {
            'a'..='z' | '0'..='9' | '+' => spaced.push(c),
            ',' | '?' => {
                spaced.push(' ');
                spaced.push(c);
                spaced.push(' ');
            }
            _ => spaced.push(' '),
        }
    }
    spaced.split_whitespace().map(str::to_owned).collect()
}

fn phrases(key: &str) -> Vec<Phrase> {
    let mut list: Vec<Phrase> = word_list(key)
        .iter()
        .map(|w| tokens(w))
        .filter(|t| !t.is_empty())
        .collect();
    // The longest first: « mets en pause » before « pause ».
    list.sort_by_key(|p| std::cmp::Reverse(p.len()));
    list
}

fn words(key: &str) -> BTreeSet<String> {
    phrases(key)
        .into_iter()
        .filter_map(|p| p.into_iter().next())
        .collect()
}

fn pairs(key: &str) -> Vec<(Phrase, String)> {
    let mut list: Vec<(Phrase, String)> = word_list(key)
        .iter()
        .filter_map(|w| w.split_once('='))
        .map(|(k, v)| (tokens(k), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    list.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    list
}

impl Lex {
    fn load() -> Self {
        let verb = |v, k: &str| (v, phrases(&format!("assistant.quick.verb_{k}")));
        let noun = |c, k: &str| (c, phrases(&format!("assistant.quick.noun_{k}")));
        let list = |k: &str| phrases(&format!("assistant.quick.{k}"));
        let set = |k: &str| words(&format!("assistant.quick.{k}"));
        let sensitive = word_list("assistant.quick.sensitive")
            .iter()
            .map(|w| match w.split_once('~') {
                Some((word, unless)) => (tokens(word), Some(tokens(unless))),
                None => (tokens(w), None),
            })
            .filter(|(w, _)| !w.is_empty())
            .collect();
        Self {
            verbs: vec![
                verb(Verb::On, "on"),
                verb(Verb::Off, "off"),
                verb(Verb::Open, "open"),
                verb(Verb::Close, "close"),
                verb(Verb::Stop, "stop"),
                verb(Verb::Set, "set"),
                verb(Verb::Play, "play"),
            ],
            step_up: list("step_up"),
            step_down: list("step_down"),
            nouns: vec![
                noun(Cat::Cover, "cover"),
                noun(Cat::Light, "light"),
                noun(Cat::Clim, "clim"),
                noun(Cat::Speaker, "speaker"),
                noun(Cat::Tv, "tv"),
            ],
            polite: list("polite"),
            filler: list("filler"),
            sensitive,
            negation: list("negation"),
            deferred: list("deferred"),
            time_units: set("time_units"),
            question_start: list("question_start"),
            question_any: list("question_any"),
            end: list("end"),
            everywhere: list("everywhere"),
            group_all: list("group_all"),
            half: list("half"),
            full: list("full"),
            minimum: list("minimum"),
            up: list("up"),
            down: list("down"),
            up_light: list("up_light"),
            down_light: list("down_light"),
            down_always: list("down_always"),
            pause: list("pause"),
            resume: list("resume"),
            next: list("next"),
            previous: list("previous"),
            mute: list("mute"),
            unmute: list("unmute"),
            degree: set("degree"),
            apps: pairs("assistant.quick.apps"),
            channels: pairs("assistant.quick.channels"),
            mode_words: pairs("assistant.quick.mode_words"),
            music_intro: list("music_intro"),
            music_end: set("music_end"),
            clause_sep: set("clause_sep"),
            stopwords: set("stopwords"),
            spec_alias: pairs("assistant.quick.spec_alias")
                .into_iter()
                .filter_map(|(k, v)| Some((k.into_iter().next()?, v)))
                .collect(),
            after_verb: set("after_verb"),
            person_prefix: tokens(&moli_i18n::tr("assistant.quick.room_person_prefix")),
            person_forms: word_list("assistant.quick.room_person_forms"),
            numbers: pairs("assistant.quick.numbers")
                .into_iter()
                .filter_map(|(k, v)| Some((k.into_iter().next()?, v.parse().ok()?)))
                .collect(),
            join: set("number_join"),
            one_after_join: set("number_one_after_join"),
            x20: set("number_x20"),
            twenty: set("number_20"),
            not_after: set("number_not_after"),
            ask_temperature: list("ask_temperature"),
            ask_whats_on: list("ask_whats_on"),
            garage_words: list("garage_words"),
            alarm_words: list("alarm_words"),
            garage_states: pairs("assistant.quick.garage_states"),
            alarm_states: pairs("assistant.quick.alarm_states"),
        }
    }

    /// Every word the catalogues use: a misheard word is never "corrected"
    /// into another, and these are never corrected at all.
    fn known_words(&self) -> BTreeSet<&String> {
        let lists: [&Vec<Phrase>; 22] = [
            &self.step_up,
            &self.step_down,
            &self.polite,
            &self.filler,
            &self.negation,
            &self.deferred,
            &self.question_start,
            &self.question_any,
            &self.end,
            &self.everywhere,
            &self.group_all,
            &self.half,
            &self.full,
            &self.minimum,
            &self.up,
            &self.down,
            &self.up_light,
            &self.down_light,
            &self.down_always,
            &self.pause,
            &self.resume,
            &self.music_intro,
        ];
        let more: [&Vec<Phrase>; 8] = [
            &self.next,
            &self.previous,
            &self.mute,
            &self.unmute,
            &self.ask_temperature,
            &self.ask_whats_on,
            &self.garage_words,
            &self.alarm_words,
        ];
        lists
            .into_iter()
            .chain(more)
            .chain(self.verbs.iter().map(|(_, l)| l))
            .chain(self.nouns.iter().map(|(_, l)| l))
            .flat_map(|l| l.iter().flatten())
            .chain(
                self.sensitive
                    .iter()
                    .flat_map(|(w, u)| w.iter().chain(u.iter().flatten())),
            )
            .chain(self.apps.iter().flat_map(|(k, _)| k))
            .chain(self.channels.iter().flat_map(|(k, _)| k))
            .chain(self.mode_words.iter().flat_map(|(k, _)| k))
            .chain(&self.time_units)
            .chain(&self.degree)
            .chain(&self.music_end)
            .chain(&self.clause_sep)
            .chain(&self.stopwords)
            .chain(&self.after_verb)
            .collect()
    }

    /// The category of the first noun said (a device noun: « store », « clim »).
    fn noun(&self, toks: &[String]) -> Option<Cat> {
        self.nouns
            .iter()
            .filter_map(|(cat, list)| first_at(toks, list).map(|at| (at, *cat)))
            .min_by_key(|(at, _)| *at)
            .map(|(_, cat)| cat)
    }

    /// The first verb said, with the words that said it.
    fn verb(&self, toks: &[String]) -> Option<(Verb, Phrase)> {
        self.verbs
            .iter()
            .flat_map(|(v, list)| list.iter().map(move |p| (*v, p)))
            .filter_map(|(v, p)| find(toks, p).map(|at| (at, v, p)))
            .min_by_key(|(at, _, p)| (*at, std::cmp::Reverse(p.len())))
            .map(|(_, v, p)| (v, p.clone()))
    }

    fn is_verb_word(&self, word: &str) -> bool {
        self.verbs
            .iter()
            .any(|(_, list)| list.iter().any(|p| p.last().is_some_and(|w| w == word)))
    }

    /// Words said as numbers become digits (« vingt et un » → 21).
    fn digitize(&self, toks: &[String]) -> Vec<String> {
        let mut out = Vec::with_capacity(toks.len());
        let mut i = 0;
        while i < toks.len() {
            let Some(first) = self.numbers.get(&toks[i]).filter(|_| {
                // « pour cent » is a percent, not a hundred.
                !out.last()
                    .is_some_and(|before| self.not_after.contains(before))
            }) else {
                out.push(toks[i].clone());
                i += 1;
                continue;
            };
            let mut total = *first;
            let mut j = i + 1;
            if self.x20.contains(&toks[i]) && toks.get(j).is_some_and(|w| self.twenty.contains(w)) {
                total = 80;
                j += 1; // the « vingt » of « quatre-vingt »
            }
            while j < toks.len() {
                let word = &toks[j];
                if let Some(v) = self.numbers.get(word) {
                    total += v;
                    j += 1;
                } else if self.join.contains(word)
                    && toks.get(j + 1).is_some_and(|n| {
                        self.one_after_join.contains(n) || self.numbers.contains_key(n)
                    })
                {
                    j += 1;
                    if self.one_after_join.contains(&toks[j]) {
                        total += 1;
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            out.push(total.to_string());
            i = j;
        }
        out
    }
}

/// Where `phrase` starts in `toks`, if it is there word for word.
fn find(toks: &[String], phrase: &[String]) -> Option<usize> {
    if phrase.is_empty() || phrase.len() > toks.len() {
        return None;
    }
    toks.windows(phrase.len()).position(|w| w == phrase)
}

fn first_at(toks: &[String], list: &[Phrase]) -> Option<usize> {
    list.iter().filter_map(|p| find(toks, p)).min()
}

fn has(toks: &[String], list: &[Phrase]) -> bool {
    list.iter().any(|p| find(toks, p).is_some())
}

fn remove_phrases(toks: &mut Vec<String>, list: &[Phrase]) {
    for p in list {
        while let Some(at) = find(toks, p) {
            toks.drain(at..at + p.len());
        }
    }
}

/// 1 for the same word, down to 0 for nothing in common.
#[allow(clippy::cast_precision_loss)]
fn similarity(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            row.push((prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    1.0 - prev[b.len()] as f64 / a.len().max(b.len()).max(1) as f64
}

// ---------------------------------------------------------------- the devices

struct Dev<'a> {
    view: &'a DeviceView,
    id: DeviceId,
    name: String,
    name_toks: Vec<String>,
    room: Option<String>,
    cat: Cat,
    /// A group of covers (« tous les stores », « la pièce de vie »).
    group: bool,
    online: bool,
    /// What tells this device from its neighbours: the name's words that are
    /// neither the room nor the noun of its own kind.
    spec: BTreeSet<String>,
}

impl Dev<'_> {
    fn point(&self, key: &str) -> Option<&PointSpec> {
        self.view.device.point(key).filter(|p| p.access.write)
    }

    /// A speaker that is playing right now.
    fn is_playing(&self) -> bool {
        matches!(
            self.view.state.get("playing").map(|s| &s.value),
            Some(Value::Bool(true))
        )
    }

    fn enum_has(&self, key: &str, value: &str) -> bool {
        self.point(key).is_some_and(|p| {
            matches!(&p.kind, PointKind::Enum { values } if values.iter().any(|v| &**v == value))
        })
    }

    #[allow(clippy::cast_precision_loss)]
    fn number(&self, key: &str) -> Option<f64> {
        match self.view.state.get(key)?.value {
            Value::Float(f) => Some(f),
            Value::Int(i) => Some(i as f64),
            _ => None,
        }
    }

    fn range(&self, key: &str) -> (Option<f64>, Option<f64>) {
        match self.point(key).map(|p| &p.kind) {
            Some(PointKind::Numeric { min, max, .. }) => (*min, *max),
            _ => (None, None),
        }
    }

    /// A dimmer's level point (`ep1.level`), and the one that brings it back.
    fn dimmer(&self) -> Option<(String, Option<String>)> {
        let level = self.view.device.points.iter().find(|p| {
            p.access.write
                && matches!(p.kind, PointKind::Numeric { .. })
                && (&*p.key == "level" || p.key.ends_with(".level"))
        })?;
        let restore = format!("{}_restore", level.key);
        let restore = self.point(&restore).map(|_| restore);
        Some((level.key.to_string(), restore))
    }
}

fn category(view: &DeviceView) -> Option<(Cat, bool)> {
    let d = &view.device;
    let w = |k: &str| d.point(k).filter(|p| p.access.write);
    let has_enum = |k: &str, v: &str| {
        w(k).is_some_and(
            |p| matches!(&p.kind, PointKind::Enum { values } if values.iter().any(|x| &**x == v)),
        )
    };
    if d.points.iter().any(|p| p.semantic == Semantic::GarageDoor)
        || has_enum("target", "open")
        || has_enum("mode", "disarmed")
    {
        return Some((Cat::Sensitive, false));
    }
    if has_enum("control", "open") {
        return Some((Cat::Cover, d.model.as_deref() == Some("CoverGroup")));
    }
    let dimmer = d.points.iter().any(|p| {
        p.access.write
            && matches!(p.kind, PointKind::Numeric { .. })
            && (&*p.key == "level" || p.key.ends_with(".level"))
    });
    let cat = if w("target_temperature").is_some() {
        Cat::Clim
    } else if w("playing").is_some() && w("volume").is_some() {
        Cat::Speaker
    } else if w("power").is_some() {
        Cat::Tv
    } else if dimmer || (w("on").is_some() && w("brightness").is_some()) {
        Cat::Light
    } else if d.model.as_deref() == Some("Aide") && w("on").is_some() {
        Cat::Helper
    } else {
        return None;
    };
    Some((cat, false))
}

/// A temperature read in a room.
struct Sensor {
    room: String,
    value: f64,
    /// Read by an air conditioner rather than by a sensor.
    clim: bool,
}

struct Catalog<'a> {
    lex: Lex,
    devs: Vec<Dev<'a>>,
    sensors: Vec<Sensor>,
    /// Every way to say a room → its name.
    rooms: Vec<(Phrase, String)>,
    room_words: BTreeSet<String>,
    modes: Vec<(Phrase, String)>,
    vocab: BTreeSet<String>,
}

impl<'a> Catalog<'a> {
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn new(devices: &'a [DeviceView], layout: &Layout, config: &Config) -> Self {
        let lex = Lex::load();
        let no_room = no_room();
        // The rooms that hold something one can order.
        let mut named: BTreeSet<String> = BTreeSet::new();
        for view in devices {
            if layout.hides(view.device.id.as_str()) || skipped(config, view) {
                continue;
            }
            if let Some((cat, group)) = category(view)
                && cat != Cat::Sensitive
                && !group
                && cat != Cat::Helper
            {
                let room = layout.room_of(view);
                if room != no_room {
                    named.insert(room);
                }
            }
        }
        // The temperatures read in a room (a room can be only read, never ordered).
        let mut readings: Vec<(&DeviceView, f64)> = Vec::new();
        for view in devices {
            if layout.hides(view.device.id.as_str()) || skipped(config, view) {
                continue;
            }
            for p in view
                .device
                .points
                .iter()
                .filter(|p| p.access.read && p.semantic == Semantic::Temperature)
            {
                let value = match view.state.get(&*p.key).map(|s| &s.value) {
                    Some(Value::Float(f)) => Some(*f),
                    Some(Value::Int(i)) => Some(*i as f64),
                    _ => None,
                };
                if let Some(v) = value {
                    readings.push((view, v));
                    let room = layout.room_of(view);
                    if room != no_room {
                        named.insert(room);
                    }
                }
            }
        }
        let mut rooms: Vec<(Phrase, String)> = Vec::new();
        for room in &named {
            rooms.push((tokens(room), room.clone()));
            rooms.extend(person_forms(&lex, room));
        }
        for def in &layout.rooms {
            let canonical = layout.room(&def.name);
            if named.contains(&canonical) {
                for alias in &def.aliases {
                    rooms.push((tokens(alias), canonical.clone()));
                }
            }
        }
        for (alias, room) in &config.room_aliases {
            let canonical = layout.room(room);
            if named.contains(&canonical) {
                rooms.push((tokens(alias), canonical));
            }
        }
        rooms.retain(|(p, _)| !p.is_empty());
        rooms.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        rooms.dedup();
        let room_words: BTreeSet<String> = rooms
            .iter()
            .flat_map(|(p, _)| p.iter().cloned())
            .filter(|w| !lex.stopwords.contains(w))
            .collect();
        let room_names: Vec<(Phrase, String)> =
            named.iter().map(|r| (tokens(r), r.clone())).collect();

        let mut devs = Vec::new();
        let mut modes: Vec<(Phrase, String)> = lex
            .mode_words
            .iter()
            .map(|(k, v)| (k.clone(), tokens(v).join(" ")))
            .collect();
        for view in devices {
            if layout.hides(view.device.id.as_str()) || skipped(config, view) {
                continue;
            }
            let Some((cat, group)) = category(view) else {
                continue;
            };
            let name = name_of(view).to_owned();
            let name_toks = tokens(&name);
            let room = Some(layout.room_of(view))
                .filter(|r| *r != no_room && named.contains(r))
                .or_else(|| {
                    room_names
                        .iter()
                        .find(|(p, _)| find(&name_toks, p).is_some())
                        .map(|(_, r)| r.clone())
                });
            let room_toks: Vec<String> = room.as_deref().map(tokens).unwrap_or_default();
            let own_nouns: BTreeSet<&String> = lex
                .nouns
                .iter()
                .filter(|(c, _)| *c == cat)
                .flat_map(|(_, l)| l.iter().flatten())
                .collect();
            let spec = name_toks
                .iter()
                .filter(|t| {
                    !lex.stopwords.contains(*t) && !room_toks.contains(t) && !own_nouns.contains(t)
                })
                .cloned()
                .collect();
            if cat == Cat::Clim
                && let Some(PointSpec {
                    kind: PointKind::Enum { values },
                    ..
                }) = view.device.point("mode")
            {
                for v in values {
                    let t = tokens(v);
                    modes.push((t.clone(), t.join(" ")));
                }
            }
            devs.push(Dev {
                view,
                id: view.device.id.clone(),
                name,
                name_toks,
                room,
                cat,
                group,
                online: view.online == Some(true),
                spec,
            });
        }
        modes.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        modes.dedup();

        // What a misheard word may be corrected to.
        let mut vocab = BTreeSet::new();
        for list in lex
            .verbs
            .iter()
            .map(|(_, l)| l)
            .chain(lex.nouns.iter().map(|(_, l)| l))
        {
            vocab.extend(list.iter().flatten().cloned());
        }
        vocab.extend(lex.apps.iter().flat_map(|(k, _)| k.iter().cloned()));
        vocab.extend(modes.iter().flat_map(|(k, _)| k.iter().cloned()));
        vocab.extend(room_words.iter().cloned());
        vocab.extend(devs.iter().flat_map(|d| d.spec.iter().cloned()));
        vocab.retain(|w| w.chars().count() >= FUZZY_MIN_LEN);
        let sensors = readings
            .iter()
            .filter_map(|(view, value)| {
                let room = Some(layout.room_of(view))
                    .filter(|r| *r != no_room)
                    .or_else(|| {
                        let t = tokens(name_of(view));
                        room_names
                            .iter()
                            .find(|(p, _)| find(&t, p).is_some())
                            .map(|(_, r)| r.clone())
                    })?;
                Some(Sensor {
                    room,
                    value: *value,
                    clim: category(view).is_some_and(|(c, _)| c == Cat::Clim),
                })
            })
            .collect();
        Self {
            lex,
            devs,
            sensors,
            rooms,
            room_words,
            modes,
            vocab,
        }
    }

    fn room_in(&self, toks: &[String]) -> Option<String> {
        self.rooms
            .iter()
            .find(|(p, _)| find(toks, p).is_some())
            .map(|(_, r)| r.clone())
    }
}

/// A device the house asked the quick path never to order.
fn skipped(config: &Config, view: &DeviceView) -> bool {
    config
        .quick_skip
        .iter()
        .any(|id| id == view.device.id.as_str())
}

/// « chambre Léa » → « chambre de Léa », « chez Léa », « Léa ».
fn person_forms(lex: &Lex, room: &str) -> Vec<(Phrase, String)> {
    let t = tokens(room);
    if lex.person_prefix.is_empty()
        || t.len() < 2
        || t[..lex.person_prefix.len()] != lex.person_prefix[..]
    {
        return Vec::new();
    }
    let who = t[lex.person_prefix.len()..].join(" ");
    lex.person_forms
        .iter()
        .map(|f| (tokens(&f.replace("{x}", &who)), room.to_owned()))
        .collect()
}

// -------------------------------------------------------------- what to do

#[derive(Clone, Debug, PartialEq)]
enum Op {
    On,
    Off,
    Open,
    Close,
    Stop,
    Half,
    Full,
    Min,
    Up,
    Down,
    Level(i64),
    Temp(i64),
    Mode(String),
    Mute,
    Unmute,
    Play,
    Pause,
    Next,
    Prev,
    Music(String),
    App(String),
    Channel(String),
}

#[derive(Default)]
struct Ctx {
    op: Option<Op>,
    cat: Option<Cat>,
}

type Fail = &'static str;

/// Understands `text`, said in `room` (the room of the microphone, if known).
pub(crate) fn interpret(
    text: &str,
    room: Option<&str>,
    devices: &[DeviceView],
    layout: &Layout,
    config: &Config,
) -> Outcome {
    let cat = Catalog::new(devices, layout, config);
    let room = room
        .filter(|r| !r.trim().is_empty())
        .map(|r| layout.room(r))
        .map(|r| cat.room_in(&tokens(&r)).unwrap_or(r));
    cat.interpret(text, room.as_deref())
}

impl Catalog<'_> {
    fn interpret(&self, text: &str, box_room: Option<&str>) -> Outcome {
        let lex = &self.lex;
        let raw = tokens(text);
        let said_something_to_do = raw.iter().any(|t| lex.is_verb_word(t));
        if has(&raw, &lex.end)
            && raw.len() <= 7
            && !raw
                .iter()
                .any(|t| lex.noun(std::slice::from_ref(t)).is_some())
            && !has_app(lex, &raw)
        {
            return Outcome::End;
        }
        let toks = self.clean(raw);
        if has_prefix(&toks, &lex.question_start)
            || has(&toks, &lex.question_any)
            || (toks.last().is_some_and(|t| t == "?") && !said_something_to_do)
        {
            return self
                .answer(&toks, box_room)
                .unwrap_or(Outcome::Pass("question"));
        }
        if lex
            .sensitive
            .iter()
            .any(|(word, unless)| sensitive_hit(&toks, word, unless.as_deref()))
        {
            return Outcome::Pass("sensitive");
        }
        if has(&toks, &lex.negation) {
            return Outcome::Pass("negation");
        }
        if has(&toks, &lex.deferred) || self.timed(&toks) {
            return Outcome::Pass("deferred");
        }
        let mut orders: Vec<Order> = Vec::new();
        let mut ctx = Ctx::default();
        for clause in toks.split(|t| t == "," || lex.clause_sep.contains(t)) {
            let clause: Vec<String> = clause.iter().filter(|t| *t != "?").cloned().collect();
            if clause.is_empty() {
                continue;
            }
            match self.clause(&clause, &mut ctx, box_room) {
                Ok(list) => {
                    for o in list {
                        if !orders.contains(&o) {
                            orders.push(o);
                        }
                    }
                }
                Err(why) => return Outcome::Pass(why),
            }
        }
        if orders.is_empty() {
            Outcome::Pass("nothing")
        } else if orders.len() > MAX_ORDERS {
            Outcome::Pass("too-many")
        } else {
            Outcome::Run(orders)
        }
    }

    /// A question about the state of the house, answered from the inventory:
    /// never a guess (nothing readable, no room, no device: `None`).
    fn answer(&self, toks: &[String], box_room: Option<&str>) -> Option<Outcome> {
        let lex = &self.lex;
        let said_room = self.room_in(toks);
        let room = said_room.clone().or_else(|| box_room.map(str::to_owned));
        if has(toks, &lex.ask_temperature) || toks.iter().any(|t| lex.degree.contains(t)) {
            return self.say_temperature(room.as_deref());
        }
        if has(toks, &lex.ask_whats_on) {
            return Some(self.say_whats_on());
        }
        // The garage door and the alarm: reading them is harmless.
        let sensitive = if has(toks, &lex.garage_words) {
            Some((true, &lex.garage_states))
        } else if has(toks, &lex.alarm_words) {
            Some((false, &lex.alarm_states))
        } else {
            None
        };
        if let Some((garage, states)) = sensitive {
            let (asked, read) = if garage {
                ("target", "state")
            } else {
                ("mode", "mode")
            };
            let lines: Vec<(&Dev, String)> = self
                .devs
                .iter()
                .filter(|d| d.cat == Cat::Sensitive && d.view.device.point(asked).is_some())
                .filter_map(|d| match d.view.state.get(read).map(|s| &s.value) {
                    Some(Value::Text(t)) => states
                        .iter()
                        .find(|(k, _)| *k == tokens(t))
                        .map(|(_, v)| (d, v.clone())),
                    _ => None,
                })
                .collect();
            return Self::say_lines(&lines);
        }
        let cat = lex
            .noun(toks)
            .filter(|c| matches!(c, Cat::Cover | Cat::Light | Cat::Clim | Cat::Speaker))?;
        let devs: Vec<&Dev> = if cat == Cat::Speaker {
            self.devs
                .iter()
                .filter(|d| d.cat == Cat::Speaker)
                .filter(|d| room.as_deref().is_none_or(|r| d.room.as_deref() == Some(r)))
                .collect()
        } else {
            let spec: BTreeSet<String> = toks
                .iter()
                .map(|t| lex.spec_alias.get(t).cloned().unwrap_or_else(|| t.clone()))
                .filter(|t| self.is_spec(t) && !self.room_words.contains(t))
                .collect();
            let everywhere = has(toks, &lex.everywhere);
            self.pick(
                &Op::On,
                Some(cat),
                said_room.as_deref(),
                &spec,
                toks,
                box_room,
                everywhere,
            )
            .ok()?
        };
        let lines: Vec<(&Dev, String)> = devs
            .into_iter()
            .filter_map(|d| self.state_text(d).map(|t| (d, t)))
            .collect();
        Self::say_lines(&lines)
    }

    fn say_lines(lines: &[(&Dev, String)]) -> Option<Outcome> {
        if lines.is_empty() {
            return None;
        }
        let text = lines
            .iter()
            .map(|(d, state)| {
                moli_i18n::tr!("assistant.quick.say_item", name = d.name, state = state)
            })
            .collect::<Vec<_>>()
            .join(" ");
        Some(Outcome::Say {
            text,
            devices: lines.iter().map(|(d, _)| d.id.clone()).collect(),
        })
    }

    /// The mean of what the room's sensors read (its air conditioner only
    /// when it has no sensor).
    #[allow(clippy::cast_precision_loss)]
    fn say_temperature(&self, room: Option<&str>) -> Option<Outcome> {
        let room = room?;
        let here = |clim: bool| -> Vec<f64> {
            self.sensors
                .iter()
                .filter(|s| s.room == room && s.clim == clim)
                .map(|s| s.value)
                .collect()
        };
        let mut values = here(false);
        if values.is_empty() {
            values = here(true);
        }
        if values.is_empty() {
            return None;
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        Some(Outcome::Say {
            text: moli_i18n::tr!(
                "assistant.quick.say_temperature",
                room = room,
                value = number_text(mean)
            ),
            devices: Vec::new(),
        })
    }

    fn say_whats_on(&self) -> Outcome {
        let on: Vec<(&Dev, f64)> = self
            .devs
            .iter()
            .filter(|d| d.cat == Cat::Light)
            .filter_map(|d| light_percent(d).filter(|p| *p > 0.0).map(|p| (d, p)))
            .collect();
        if on.is_empty() {
            return Outcome::Say {
                text: moli_i18n::tr!("assistant.quick.say_nothing_on"),
                devices: Vec::new(),
            };
        }
        let list = on
            .iter()
            .map(|(d, p)| format!("{} ({} %)", d.name, percent_text(*p)))
            .collect::<Vec<_>>()
            .join(", ");
        Outcome::Say {
            text: moli_i18n::tr!("assistant.quick.say_whats_on", list = list),
            devices: on.iter().map(|(d, _)| d.id.clone()).collect(),
        }
    }

    /// How one device is, in a word: `None` when its state is not known.
    #[allow(clippy::unused_self)]
    fn state_text(&self, d: &Dev<'_>) -> Option<String> {
        let t = |key: &str| moli_i18n::tr(key);
        match d.cat {
            Cat::Cover => {
                let p = d.number("percent_control")?;
                Some(if p >= 99.5 {
                    t("assistant.quick.state_open")
                } else if p <= 0.5 {
                    t("assistant.quick.state_closed")
                } else {
                    moli_i18n::tr!("assistant.quick.state_open_part", percent = percent_text(p))
                })
            }
            Cat::Light => {
                let p = light_percent(d)?;
                Some(if p <= 0.0 {
                    t("assistant.quick.state_light_off")
                } else if p >= 99.0 {
                    t("assistant.quick.state_light_on")
                } else {
                    moli_i18n::tr!(
                        "assistant.quick.state_light_level",
                        percent = percent_text(p)
                    )
                })
            }
            Cat::Clim => match d.view.state.get("on").map(|s| &s.value) {
                Some(Value::Bool(false)) => Some(t("assistant.quick.state_light_off")),
                Some(Value::Bool(true)) => {
                    let mode = match d.view.state.get("mode").map(|s| &s.value) {
                        Some(Value::Text(m)) => Some(m.to_string()),
                        _ => None,
                    };
                    match (mode, d.number("target_temperature")) {
                        (Some(mode), Some(target)) => Some(moli_i18n::tr!(
                            "assistant.quick.state_clim_on",
                            mode = mode,
                            target = number_text(target)
                        )),
                        _ => Some(t("assistant.quick.state_light_on")),
                    }
                }
                _ => None,
            },
            Cat::Speaker => match d.view.state.get("playing").map(|s| &s.value) {
                Some(Value::Bool(true)) => Some(t("assistant.quick.state_playing")),
                Some(Value::Bool(false)) => Some(t("assistant.quick.state_idle")),
                _ => None,
            },
            _ => None,
        }
    }

    /// A number followed by a unit of time (« dans 10 minutes », « à 18 h »).
    fn timed(&self, toks: &[String]) -> bool {
        toks.windows(2).any(|w| {
            w[0].chars().all(|c| c.is_ascii_digit()) && self.lex.time_units.contains(&w[1])
        })
    }

    /// What was heard, made fit to be matched.
    fn clean(&self, mut toks: Vec<String>) -> Vec<String> {
        let lex = &self.lex;
        remove_phrases(&mut toks, &lex.polite);
        remove_phrases(&mut toks, &lex.filler);
        // « allume-moi la lampe » = « allume la lampe ».
        let mut i = 1;
        while i < toks.len() {
            if lex.after_verb.contains(&toks[i]) && lex.is_verb_word(&toks[i - 1]) {
                toks.remove(i);
            } else {
                i += 1;
            }
        }
        let known_words = lex.known_words();
        for t in &mut toks {
            let known = self.vocab.contains(t) || known_words.contains(t);
            if !known
                && t.chars().count() >= FUZZY_MIN_LEN
                && !t.chars().any(|c| c.is_ascii_digit())
            {
                let mut scored: Vec<(f64, &String)> = self
                    .vocab
                    .iter()
                    .map(|v| (similarity(t, v), v))
                    .filter(|(s, _)| *s >= FUZZY_MIN_SIMILARITY)
                    .collect();
                scored.sort_by(|a, b| b.0.total_cmp(&a.0));
                // Two equally good guesses are no guess.
                if let Some((best, word)) = scored.first()
                    && scored.get(1).is_none_or(|(second, _)| second < best)
                {
                    t.clone_from(word);
                } else if scored.is_empty()
                    && let Some(word) = self
                        .vocab
                        .iter()
                        .filter(|w| {
                            w.chars().count() >= 5
                                && t.ends_with(w.as_str())
                                && t.chars().count() - w.chars().count() <= 3
                        })
                        .max_by_key(|w| w.len())
                {
                    // « bévitrée » for « baie vitrée »: it ends like a word we know.
                    t.clone_from(word);
                }
            }
        }
        lex.digitize(&toks)
    }

    #[allow(clippy::too_many_lines)]
    fn clause(
        &self,
        toks: &[String],
        ctx: &mut Ctx,
        box_room: Option<&str>,
    ) -> Result<Vec<Order>, Fail> {
        let lex = &self.lex;
        let verb = lex.verb(toks);
        let verb_kind = verb.as_ref().map(|(v, _)| *v);
        let number: Option<i64> = toks.iter().find_map(|t| t.parse().ok());
        let noun = lex.noun(toks);
        let (op, mut cat) = if let Some(found) = self.op_of(toks, verb.as_ref(), number, noun) {
            found
        } else {
            // « … et la salle à manger » (same order), or chatter.
            let refers = noun.is_some()
                || number.is_some()
                || toks
                    .iter()
                    .any(|t| self.room_words.contains(t) || self.is_spec(t));
            if !refers {
                return Ok(Vec::new());
            }
            (ctx.op.clone().ok_or("unclear")?, noun)
        };
        if cat.is_none() && verb_kind.is_none() {
            cat = ctx.cat;
        }
        if cat.is_none()
            && matches!(
                op,
                Op::Level(_) | Op::Half | Op::Min | Op::Full | Op::Up | Op::Down
            )
        {
            return Err("no-device");
        }
        let room = self.room_in(toks);
        let toks_spec: Vec<String> = toks
            .iter()
            .map(|t| lex.spec_alias.get(t).cloned().unwrap_or_else(|| t.clone()))
            .collect();
        let spec: BTreeSet<String> = toks_spec
            .iter()
            .filter(|t| self.is_spec(t) && !self.room_words.contains(*t))
            .cloned()
            .collect();
        let everywhere = has(toks, &lex.everywhere);

        let devs = self.pick(&op, cat, room.as_deref(), &spec, toks, box_room, everywhere)?;
        let mut orders = Vec::new();
        for d in devs {
            for (key, value) in Self::one(d, &op)? {
                orders.push(Order {
                    point: PointId::new(&d.id, &key),
                    value,
                    device: d.name.clone(),
                });
            }
        }
        ctx.op = Some(op);
        ctx.cat = cat.or(ctx.cat);
        Ok(orders)
    }

    fn is_spec(&self, token: &str) -> bool {
        let t = self.lex.spec_alias.get(token).map_or(token, String::as_str);
        self.devs.iter().any(|d| !d.group && d.spec.contains(t))
    }

    /// What is asked, and of what kind of device.
    #[allow(clippy::too_many_lines)]
    fn op_of(
        &self,
        toks: &[String],
        verb: Option<&(Verb, Phrase)>,
        number: Option<i64>,
        noun: Option<Cat>,
    ) -> Option<(Op, Option<Cat>)> {
        let lex = &self.lex;
        let verb_kind = verb.map(|(v, _)| *v);
        let said = |list: &[Phrase]| has(toks, list);
        let pair = |list: &[(Phrase, String)]| {
            list.iter()
                .find(|(k, _)| find(toks, k).is_some())
                .map(|(_, v)| v.clone())
        };
        if let Some(app) = pair(&lex.apps) {
            return Some((Op::App(app), Some(Cat::Tv)));
        }
        if !matches!(verb_kind, Some(Verb::Open | Verb::Close))
            && let Some(channel) = pair(&lex.channels)
        {
            return Some((Op::Channel(channel), Some(Cat::Tv)));
        }
        let media = |op: Op| Some((op, noun.or(Some(Cat::Speaker))));
        if said(&lex.pause) {
            return media(Op::Pause);
        }
        if said(&lex.resume) {
            return media(Op::Play);
        }
        if said(&lex.next) {
            return media(Op::Next);
        }
        if said(&lex.previous) {
            return media(Op::Prev);
        }
        if said(&lex.mute) {
            return media(Op::Mute);
        }
        if said(&lex.unmute) {
            return media(Op::Unmute);
        }
        if number.is_none() {
            if said(&lex.up_light) {
                return Some((Op::Up, noun.or(Some(Cat::Light))));
            }
            if said(&lex.down_light) {
                return Some((Op::Down, noun.or(Some(Cat::Light))));
            }
            if said(&lex.up) {
                return media(Op::Up);
            }
            let lowering = matches!(verb_kind, Some(Verb::Close | Verb::Off | Verb::Open));
            if said(&lex.down_always) || (said(&lex.down) && !lowering) {
                return media(Op::Down);
            }
            // « baisse la lumière » dims, « monte la lumière » brightens.
            if noun == Some(Cat::Light)
                && let Some((_, said_verb)) = verb
            {
                if lex.step_up.contains(said_verb) {
                    return Some((Op::Up, noun));
                }
                if lex.step_down.contains(said_verb) {
                    return Some((Op::Down, noun));
                }
            }
        }
        if said(&lex.half) {
            return Some((Op::Half, noun));
        }
        if said(&lex.full) && verb_kind != Some(Verb::Close) {
            let op = if verb_kind == Some(Verb::Open) {
                Op::Open
            } else {
                Op::Full
            };
            return Some((op, noun));
        }
        if said(&lex.minimum) {
            return Some((Op::Min, noun));
        }
        if let Some(n) = number {
            return if toks.iter().any(|t| lex.degree.contains(t)) {
                Some((Op::Temp(n), Some(Cat::Clim)))
            } else {
                Some((Op::Level(n), noun))
            };
        }
        if noun == Some(Cat::Clim)
            && let Some((_, mode)) = self.modes.iter().find(|(k, _)| find(toks, k).is_some())
        {
            return Some((Op::Mode(mode.clone()), noun));
        }
        let (kind, said_verb) = verb?;
        let simple = |op: Op| Some((op, noun));
        match kind {
            Verb::Open => simple(Op::Open),
            Verb::Close => simple(Op::Close),
            Verb::Off => simple(Op::Off),
            Verb::On => simple(Op::On),
            Verb::Stop => simple(Op::Stop),
            // « mets du jazz », « joue Taylor Swift » (but « mets la lumière du salon » is no song).
            Verb::Play => match self.music(toks, said_verb) {
                m @ Op::Music(_) => Some((m, noun.or(Some(Cat::Speaker)))),
                _ => Some((Op::Play, noun.or(Some(Cat::Speaker)))),
            },
            Verb::Set if noun.is_some_and(|c| c != Cat::Speaker) => simple(Op::On),
            Verb::Set => match self.music(toks, said_verb) {
                m @ Op::Music(_) => Some((m, noun.or(Some(Cat::Speaker)))),
                _ => simple(Op::On),
            },
        }
    }

    /// « mets du jazz », « joue Taylor Swift dans la chambre » → what to play.
    fn music(&self, toks: &[String], said_verb: &Phrase) -> Op {
        let lex = &self.lex;
        let after_verb = find(toks, said_verb).map_or(0, |at| at + said_verb.len());
        let mut from = after_verb;
        let rest = &toks[after_verb..];
        if let Some((at, intro)) = lex
            .music_intro
            .iter()
            .filter_map(|p| find(rest, p).map(|at| (at, p)))
            .min_by_key(|(at, p)| (*at, std::cmp::Reverse(p.len())))
        {
            from = after_verb + at + intro.len();
        } else if lex
            .verbs
            .iter()
            .any(|(v, l)| *v == Verb::Set && l.contains(said_verb))
        {
            // « mets pause » was handled; « mets le salon » is not music.
            return Op::On;
        }
        let what: Vec<&String> = toks[from..]
            .iter()
            .take_while(|t| !lex.music_end.contains(*t))
            .filter(|t| {
                lex.noun(std::slice::from_ref(*t))
                    .is_none_or(|c| c != Cat::Speaker)
            })
            .collect();
        if what.is_empty() {
            Op::On
        } else {
            Op::Music(
                what.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
    }

    /// The devices an order reaches.
    #[allow(clippy::too_many_arguments)]
    fn pick(
        &self,
        op: &Op,
        cat: Option<Cat>,
        room: Option<&str>,
        spec: &BTreeSet<String>,
        toks: &[String],
        box_room: Option<&str>,
        everywhere: bool,
    ) -> Result<Vec<&Dev<'_>>, Fail> {
        // The TV: the house's helpers (« TF1 », « la télé ») first, then the screens.
        if cat == Some(Cat::Tv) {
            return self.pick_tv(op, toks);
        }
        if cat == Some(Cat::Cover) {
            if let Some(g) = self
                .devs
                .iter()
                .find(|d| d.group && find(toks, &d.name_toks).is_some())
            {
                return Ok(vec![g]);
            }
            if everywhere
                && room.is_none()
                && spec.is_empty()
                && let Some(g) = self
                    .devs
                    .iter()
                    .find(|d| d.group && has_phrase(&self.lex.group_all, &d.name_toks))
            {
                return Ok(vec![g]);
            }
        }
        let kinds: &[Cat] = match cat {
            Some(Cat::Cover) => &[Cat::Cover],
            Some(Cat::Light) => &[Cat::Light],
            Some(Cat::Clim) => &[Cat::Clim],
            Some(Cat::Speaker) => &[Cat::Speaker],
            None if room.is_some() || !spec.is_empty() => &[Cat::Light, Cat::Cover, Cat::Clim],
            _ => return Err("no-device"),
        };
        let room = room.or(if spec.is_empty() { box_room } else { None });
        let mut cands: Vec<&Dev> = self
            .devs
            .iter()
            .filter(|d| !d.group && kinds.contains(&d.cat))
            .filter(|d| room.is_none_or(|r| d.room.as_deref() == Some(r)))
            .collect();
        if !spec.is_empty() {
            cands.retain(|d| spec.is_subset(&d.spec));
            if let Some(best) = cands.iter().map(|d| d.spec.len()).min() {
                cands.retain(|d| d.spec.len() == best);
            }
        }
        // « éteins le salon » is its lights.
        let mut cat = cat;
        if cat.is_none() && spec.is_empty() {
            cands.retain(|d| d.cat == Cat::Light);
            cat = Some(Cat::Light);
        }
        if cands.is_empty() {
            return Err("no-device");
        }
        // « pause », « plus fort » with no room said: what is playing now.
        if cat == Some(Cat::Speaker)
            && cands.len() > 1
            && room.is_none()
            && matches!(
                op,
                Op::Pause
                    | Op::Next
                    | Op::Prev
                    | Op::Mute
                    | Op::Unmute
                    | Op::Up
                    | Op::Down
                    | Op::Level(_)
                    | Op::Off
                    | Op::Stop
            )
        {
            let playing: Vec<&Dev> = cands.iter().copied().filter(|d| d.is_playing()).collect();
            if playing.len() == 1 {
                cands = playing;
            }
        }
        if cat == Some(Cat::Speaker) && cands.len() > 1 && room.is_some() {
            // The room's own speaker, named after it, over a portable one.
            let main: Vec<&Dev> = cands
                .iter()
                .copied()
                .filter(|d| {
                    d.room
                        .as_deref()
                        .map(tokens)
                        .is_some_and(|r| r == d.name_toks)
                })
                .collect();
            if !main.is_empty() {
                cands = main;
            }
        }
        if cands.len() > 1 && !matches!(cat, Some(Cat::Cover | Cat::Light)) && !everywhere {
            return Err("ambiguous");
        }
        if cands.len() > 1 && room.is_none() && !everywhere && spec.is_empty() {
            return Err("no-room");
        }
        Ok(cands)
    }

    fn pick_tv(&self, op: &Op, toks: &[String]) -> Result<Vec<&Dev<'_>>, Fail> {
        let helpers = self.devs.iter().filter(|d| d.cat == Cat::Helper);
        let screens: Vec<&Dev> = self.devs.iter().filter(|d| d.cat == Cat::Tv).collect();
        match op {
            Op::Channel(name) => {
                let wanted = tokens(name);
                if let Some(h) = helpers.clone().find(|d| d.name_toks == wanted) {
                    return Ok(vec![h]);
                }
                screens
                    .into_iter()
                    .find(|d| d.online && d.point("canal_channel").is_some())
                    .map(|d| vec![d])
                    .ok_or("no-device")
            }
            Op::On | Op::Off => {
                if let Some(h) = helpers.clone().find(|d| find(toks, &d.name_toks).is_some()) {
                    return Ok(vec![h]);
                }
                screens
                    .into_iter()
                    .find(|d| d.online)
                    .map(|d| vec![d])
                    .ok_or("no-device")
            }
            Op::App(_) => screens
                .into_iter()
                .find(|d| d.online && d.point("app").is_some())
                .map(|d| vec![d])
                .ok_or("no-device"),
            _ => screens
                .into_iter()
                .find(|d| d.online)
                .map(|d| vec![d])
                .ok_or("no-device"),
        }
    }

    /// What an order means for one device: the values to write.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn one(d: &Dev<'_>, op: &Op) -> Result<Vec<(String, Value)>, Fail> {
        let text = |k: &str, v: &str| (k.to_owned(), Value::Text(v.into()));
        let flag = |k: &str, v: bool| (k.to_owned(), Value::Bool(v));
        let num = |k: &str, v: f64| (k.to_owned(), Value::Float(v));
        match d.cat {
            Cat::Cover => {
                let command = |v: &str| {
                    if d.enum_has("control", v) {
                        Ok(vec![text("control", v)])
                    } else {
                        Err("unsupported")
                    }
                };
                match op {
                    Op::Open | Op::Full => command("open"),
                    Op::Close => command("close"),
                    Op::Stop => command("stop"),
                    Op::Half if d.point("percent_control").is_some() => {
                        Ok(vec![num("percent_control", 50.0)])
                    }
                    Op::Level(n)
                        if (0..=100).contains(n) && d.point("percent_control").is_some() =>
                    {
                        Ok(vec![num("percent_control", *n as f64)])
                    }
                    Op::Level(_) => Err("bad-value"),
                    _ => Err("unsupported"),
                }
            }
            Cat::Light => Self::light(d, op),
            Cat::Clim => match op {
                Op::On if d.point("on").is_some() => Ok(vec![flag("on", true)]),
                Op::Off if d.point("on").is_some() => Ok(vec![flag("on", false)]),
                Op::Mode(m) => {
                    let value = match d.point("mode").map(|p| &p.kind) {
                        Some(PointKind::Enum { values }) => {
                            values.iter().find(|v| tokens(v).join(" ") == *m)
                        }
                        _ => None,
                    };
                    value
                        .map(|v| vec![flag("on", true), text("mode", v)])
                        .ok_or("bad-value")
                }
                // Only the setpoint: the mode and the power stay as they are.
                Op::Temp(n) | Op::Level(n) => {
                    let (lo, hi) = d.range("target_temperature");
                    let n = *n as f64;
                    if d.point("target_temperature").is_some()
                        && lo.is_none_or(|l| n >= l)
                        && hi.is_none_or(|h| n <= h)
                    {
                        Ok(vec![num("target_temperature", n)])
                    } else {
                        Err("bad-value")
                    }
                }
                _ => Err("unsupported"),
            },
            Cat::Speaker => match op {
                Op::Pause | Op::Off | Op::Stop => Ok(vec![flag("playing", false)]),
                Op::Play | Op::On => Ok(vec![flag("playing", true)]),
                Op::Next if d.enum_has("skip", "next") => Ok(vec![text("skip", "next")]),
                Op::Prev if d.enum_has("skip", "previous") => Ok(vec![text("skip", "previous")]),
                Op::Mute if d.point("mute").is_some() => Ok(vec![flag("mute", true)]),
                Op::Unmute if d.point("mute").is_some() => Ok(vec![flag("mute", false)]),
                Op::Music(what) if d.point("play").is_some() => Ok(vec![text("play", what)]),
                Op::Level(n) => Ok(vec![num("volume", Self::bounded(d, "volume", *n as f64)?)]),
                Op::Up | Op::Down => {
                    let now = d.number("volume").ok_or("unknown-state")?;
                    let step = if *op == Op::Up {
                        VOLUME_STEP
                    } else {
                        -VOLUME_STEP
                    };
                    // « baisse » never silences: that is « coupe le son ».
                    Ok(vec![num("volume", (now + step).clamp(5.0, 100.0))])
                }
                _ => Err("unsupported"),
            },
            Cat::Tv => match op {
                Op::On => Ok(vec![flag("power", true)]),
                Op::Off => Ok(vec![flag("power", false)]),
                Op::App(app) if d.point("app").is_some() => Ok(vec![text("app", app)]),
                Op::Channel(name) => match d.point("canal_channel").map(|p| &p.kind) {
                    Some(PointKind::Enum { values }) => values
                        .iter()
                        .find(|v| tokens(v) == tokens(name))
                        .map(|v| vec![text("canal_channel", v)])
                        .ok_or("bad-value"),
                    _ => Err("unsupported"),
                },
                Op::Pause if d.enum_has("media_control", "pause") => {
                    Ok(vec![text("media_control", "pause")])
                }
                Op::Play if d.enum_has("media_control", "play") => {
                    Ok(vec![text("media_control", "play")])
                }
                Op::Mute if d.point("mute").is_some() => Ok(vec![flag("mute", true)]),
                Op::Unmute if d.point("mute").is_some() => Ok(vec![flag("mute", false)]),
                Op::Level(n) if d.point("volume").is_some() => {
                    Ok(vec![num("volume", Self::bounded(d, "volume", *n as f64)?)])
                }
                _ => Err("unsupported"),
            },
            Cat::Helper => match op {
                Op::On | Op::Channel(_) | Op::Level(_) => Ok(vec![flag("on", true)]),
                Op::Off => Ok(vec![flag("on", false)]),
                _ => Err("unsupported"),
            },
            Cat::Sensitive => Err("sensitive"),
        }
    }

    fn bounded(d: &Dev<'_>, key: &str, n: f64) -> Result<f64, Fail> {
        let (lo, hi) = d.range(key);
        if lo.is_none_or(|l| n >= l) && hi.is_none_or(|h| n <= h) {
            Ok(n)
        } else {
            Err("bad-value")
        }
    }

    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn light(d: &Dev<'_>, op: &Op) -> Result<Vec<(String, Value)>, Fail> {
        let flag = |k: &str, v: bool| (k.to_owned(), Value::Bool(v));
        let num = |k: &str, v: f64| (k.to_owned(), Value::Float(v));
        if let Some((level, restore)) = d.dimmer() {
            let (_, hi) = d.range(&level);
            let top = hi.unwrap_or(99.0);
            let to_level = |percent: f64| (percent * top / 100.0).round().clamp(1.0, top);
            return match op {
                Op::On | Op::Full => Ok(vec![match (op, restore) {
                    (Op::On, Some(r)) => flag(&r, true),
                    _ => num(&level, top),
                }]),
                Op::Off => Ok(vec![num(&level, 0.0)]),
                Op::Half => Ok(vec![num(&level, to_level(50.0))]),
                Op::Min => Ok(vec![num(&level, 1.0)]),
                Op::Level(n) if (1..=100).contains(n) => Ok(vec![num(&level, to_level(*n as f64))]),
                Op::Level(_) => Err("bad-value"),
                Op::Up | Op::Down => {
                    let now = d.number(&level).ok_or("unknown-state")? * 100.0 / top;
                    if *op == Op::Down && now <= 1.0 {
                        return Err("already-lowest");
                    }
                    let step = if *op == Op::Up {
                        LIGHT_STEP
                    } else {
                        -LIGHT_STEP
                    };
                    Ok(vec![num(&level, to_level((now + step).clamp(1.0, 100.0)))])
                }
                _ => Err("unsupported"),
            };
        }
        let on = || flag("on", true);
        match op {
            Op::On => Ok(vec![on()]),
            Op::Full => Ok(vec![on(), num("brightness", 100.0)]),
            Op::Off => Ok(vec![flag("on", false)]),
            Op::Half => Ok(vec![on(), num("brightness", 50.0)]),
            Op::Min => {
                let (lo, _) = d.range("brightness");
                Ok(vec![on(), num("brightness", lo.unwrap_or(1.0).max(1.0))])
            }
            Op::Level(n) if (1..=100).contains(n) && d.point("brightness").is_some() => {
                let (lo, _) = d.range("brightness");
                Ok(vec![
                    on(),
                    num("brightness", (*n as f64).max(lo.unwrap_or(0.0))),
                ])
            }
            Op::Level(_) => Err("bad-value"),
            Op::Up | Op::Down => {
                let now = d.number("brightness").ok_or("unknown-state")?;
                if *op == Op::Down && now <= 1.0 {
                    return Err("already-lowest");
                }
                let step = if *op == Op::Up {
                    LIGHT_STEP
                } else {
                    -LIGHT_STEP
                };
                let (lo, _) = d.range("brightness");
                Ok(vec![
                    on(),
                    num(
                        "brightness",
                        (now + step).clamp(lo.unwrap_or(1.0).max(1.0), 100.0),
                    ),
                ])
            }
            _ => Err("unsupported"),
        }
    }
}

/// A light's level in percent (0 when it is off); `None` when unknown.
fn light_percent(d: &Dev<'_>) -> Option<f64> {
    if let Some((level, _)) = d.dimmer() {
        let (_, hi) = d.range(&level);
        return d.number(&level).map(|v| v * 100.0 / hi.unwrap_or(99.0));
    }
    match d.view.state.get("on").map(|s| &s.value) {
        Some(Value::Bool(false)) => Some(0.0),
        Some(Value::Bool(true)) => Some(d.number("brightness").unwrap_or(100.0)),
        _ => None,
    }
}

/// A percentage as one says it: whole (« 61 % », never « 60.6 % »).
#[allow(clippy::cast_possible_truncation)]
fn percent_text(v: f64) -> String {
    format!("{}", v.round() as i64)
}

/// « 21,5 » → `21.5`, « 22,0 » → `22` (the voice says it right).
#[allow(clippy::cast_possible_truncation)]
fn number_text(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if (r - r.round()).abs() < 0.05 {
        format!("{}", r.round() as i64)
    } else {
        format!("{r:.1}")
    }
}

fn has_prefix(toks: &[String], list: &[Phrase]) -> bool {
    list.iter().any(|p| toks.starts_with(p))
}

fn has_phrase(list: &[Phrase], name: &[String]) -> bool {
    list.iter().any(|p| p == name)
}

fn has_app(lex: &Lex, toks: &[String]) -> bool {
    lex.apps.iter().any(|(k, _)| find(toks, k).is_some())
}

/// A sensitive word, unless it is followed by the word that makes it harmless
/// (« porte » alone is a door; « porte-fenêtre » is a window with a blind).
fn sensitive_hit(toks: &[String], word: &[String], unless: Option<&[String]>) -> bool {
    toks.windows(word.len().max(1))
        .enumerate()
        .any(|(at, w)| w == word && !unless.is_some_and(|u| toks[at + word.len()..].starts_with(u)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moli_core::{Access, Device, InstanceId, Label, Sample};
    use serde_json::json;

    use super::*;

    fn point(key: &str, kind: PointKind) -> PointSpec {
        PointSpec {
            key: key.into(),
            label: key.into(),
            kind,
            access: Access {
                read: true,
                write: true,
            },
            unit: None,
            semantic: Semantic::Other,
        }
    }

    fn numeric(min: f64, max: f64) -> PointKind {
        PointKind::Numeric {
            min: Some(min),
            max: Some(max),
            step: None,
        }
    }

    fn choice(values: &[&str]) -> PointKind {
        PointKind::Enum {
            values: values.iter().map(|v| Arc::<str>::from(*v)).collect(),
        }
    }

    fn view(
        id: &str,
        name: &str,
        room: Option<&str>,
        model: Option<&str>,
        points: Vec<PointSpec>,
        state: &[(&str, Value)],
    ) -> DeviceView {
        DeviceView {
            device: Arc::new(Device {
                id: DeviceId::from(id),
                instance: InstanceId::from("fake"),
                native_name: name.into(),
                manufacturer: None,
                model: model.map(Into::into),
                description: None,
                native_room: None,
                members: Vec::new(),
                points,
            }),
            label: Label {
                name: None,
                room: room.map(str::to_owned),
                voice: None,
            },
            online: Some(true),
            state: state
                .iter()
                .map(|(k, v)| {
                    (
                        Arc::<str>::from(*k),
                        Sample {
                            value: v.clone(),
                            ts: 0,
                        },
                    )
                })
                .collect(),
            camera: false,
            printer: false,
        }
    }

    fn blind(id: &str, name: &str, room: &str) -> DeviceView {
        blind_at(id, name, room, 100.0)
    }

    fn blind_at(id: &str, name: &str, room: &str, percent: f64) -> DeviceView {
        view(
            id,
            name,
            Some(room),
            None,
            vec![
                point("control", choice(&["open", "stop", "close"])),
                point("percent_control", numeric(0.0, 100.0)),
            ],
            &[("percent_control", Value::Float(percent))],
        )
    }

    /// A temperature read in a room, by a sensor.
    fn sensor(id: &str, name: &str, room: &str, value: f64) -> DeviceView {
        let mut v = view(
            id,
            name,
            Some(room),
            None,
            Vec::new(),
            &[("temperature", Value::Float(value))],
        );
        Arc::make_mut(&mut v.device).points.push(PointSpec {
            key: "temperature".into(),
            label: "temperature".into(),
            kind: PointKind::Numeric {
                min: None,
                max: None,
                step: None,
            },
            access: Access {
                read: true,
                write: false,
            },
            unit: None,
            semantic: Semantic::Temperature,
        });
        v
    }

    fn group(id: &str, name: &str) -> DeviceView {
        view(
            id,
            name,
            Some("Maison"),
            Some("CoverGroup"),
            vec![point("control", choice(&["open", "stop", "close"]))],
            &[],
        )
    }

    fn speaker(id: &str, name: &str, room: Option<&str>, playing: bool) -> DeviceView {
        view(
            id,
            name,
            room,
            None,
            vec![
                point("playing", PointKind::Binary),
                point("volume", numeric(0.0, 100.0)),
                point("mute", PointKind::Binary),
                point("skip", choice(&["next", "previous"])),
                point("play", PointKind::Text),
            ],
            &[
                ("volume", Value::Float(40.0)),
                ("playing", Value::Bool(playing)),
            ],
        )
    }

    /// A made-up house: nothing here is anyone's.
    fn house() -> Vec<DeviceView> {
        vec![
            blind("z:baie", "Store baie vitrée", "Salon"),
            blind_at("z:fenetre", "Store fenêtre", "Salon", 0.0),
            blind("z:cuisine", "Store Cuisine", "Cuisine"),
            blind("z:lea", "Store Chambre Léa", "Chambre Léa"),
            sensor("s:salon", "Sonde Séjour", "Salon", 21.8),
            sensor("s:lea", "Sonde Chambre Léa", "Chambre Léa", 21.5),
            group("g:tous", "Général"),
            group("g:vie", "Pièce de vie"),
            view(
                "zw:salon",
                "Lumière salon",
                Some("Salon"),
                None,
                vec![
                    point("ep1.level", numeric(0.0, 99.0)),
                    point("ep1.level_restore", PointKind::Binary),
                ],
                &[("ep1.level", Value::Float(0.0))],
            ),
            view(
                "hue:bureau",
                "Bandeau",
                Some("Bureau"),
                None,
                vec![
                    point("on", PointKind::Binary),
                    point("brightness", numeric(1.0, 100.0)),
                ],
                &[
                    ("brightness", Value::Float(80.0)),
                    ("on", Value::Bool(true)),
                ],
            ),
            view(
                "clim:cuisine",
                "Clim Cuisine",
                Some("Cuisine"),
                None,
                vec![
                    point("on", PointKind::Binary),
                    point("mode", choice(&["auto", "froid", "chaud"])),
                    point("target_temperature", numeric(16.0, 32.0)),
                ],
                &[
                    ("on", Value::Bool(true)),
                    ("mode", Value::Text("chaud".into())),
                    ("target_temperature", Value::Float(21.0)),
                ],
            ),
            speaker("sonos:salon", "Salon", None, true),
            speaker("sonos:move", "Move", Some("Salon"), false),
            view(
                "maison:tele",
                "Télé",
                None,
                Some("Aide"),
                vec![point("on", PointKind::Binary)],
                &[],
            ),
            view(
                "maison:tf1",
                "TF1",
                None,
                Some("Aide"),
                vec![point("on", PointKind::Binary)],
                &[],
            ),
            view(
                "portes:garage",
                "Porte de garage",
                None,
                None,
                vec![
                    point("target", choice(&["open", "closed"])),
                    point("state", PointKind::Text),
                ],
                &[("state", Value::Text("closed".into()))],
            ),
            view(
                "alarme:maison",
                "Alarme",
                None,
                None,
                vec![point("mode", choice(&["disarmed", "partial", "total"]))],
                &[],
            ),
        ]
    }

    fn layout() -> Layout {
        serde_json::from_value(json!({ "rooms": [{ "name": "Salon", "aliases": ["séjour"] }] }))
            .unwrap()
    }

    /// What was ordered, as `device/key=value`, sorted.
    fn said(text: &str, room: Option<&str>) -> Result<Vec<String>, String> {
        said_with(
            json!({ "room_aliases": { "chambre parentale": "Chambre Léa" } }),
            text,
            room,
        )
    }

    fn said_with(
        config: serde_json::Value,
        text: &str,
        room: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let config: Config = serde_json::from_value(config).unwrap();
        match interpret(text, room, &house(), &layout(), &config) {
            Outcome::Run(orders) => {
                let mut list: Vec<String> = orders
                    .iter()
                    .map(|o| {
                        let v = match &o.value {
                            Value::Text(t) => t.to_string(),
                            Value::Bool(b) => b.to_string(),
                            Value::Float(f) => format!("{f}"),
                            Value::Int(i) => i.to_string(),
                            Value::Null => "null".into(),
                        };
                        format!("{}={v}", o.point)
                    })
                    .collect();
                list.sort();
                Ok(list)
            }
            Outcome::End => Err("end".into()),
            Outcome::Say { .. } => Err("say".into()),
            Outcome::Pass(why) => Err(why.into()),
        }
    }

    fn ok(text: &str, room: Option<&str>, expected: &[&str]) {
        assert_eq!(
            said(text, room),
            Ok(expected.iter().map(|s| (*s).to_owned()).collect()),
            "{text}"
        );
    }

    fn passes(text: &str, room: Option<&str>, why: &str) {
        assert_eq!(said(text, room), Err(why.to_owned()), "{text}");
    }

    #[test]
    fn covers_by_room_group_and_name() {
        ok(
            "Ferme les stores du salon",
            None,
            &["z:baie/control=close", "z:fenetre/control=close"],
        );
        ok(
            "Ouvre les volets du séjour",
            None,
            &["z:baie/control=open", "z:fenetre/control=open"],
        );
        ok(
            "Ferme le store de la cuisine",
            None,
            &["z:cuisine/control=close"],
        );
        ok("Ferme le store fenêtre", None, &["z:fenetre/control=close"]);
        ok(
            "Mets le store de la cuisine à moitié",
            None,
            &["z:cuisine/percent_control=50"],
        );
        ok("Ferme tous les stores", None, &["g:tous/control=close"]);
        ok(
            "Ouvre les stores de la pièce de vie",
            None,
            &["g:vie/control=open"],
        );
        ok("Ferme le store chez Léa", None, &["z:lea/control=close"]);
        // A word the house uses and its devices don't, from `room_aliases`.
        ok(
            "Ferme le store de la chambre parentale",
            None,
            &["z:lea/control=close"],
        );
        ok(
            "Ferme les stores du salon et de la cuisine",
            None,
            &[
                "z:baie/control=close",
                "z:cuisine/control=close",
                "z:fenetre/control=close",
            ],
        );
    }

    #[test]
    fn the_room_of_the_microphone_is_the_default() {
        ok(
            "Ferme les stores",
            Some("Salon"),
            &["z:baie/control=close", "z:fenetre/control=close"],
        );
        passes("Ferme les stores", None, "no-room");
    }

    #[test]
    fn lights_and_dimmers() {
        ok(
            "Allume la lumière du salon",
            None,
            &["zw:salon/ep1.level_restore=true"],
        );
        ok(
            "Éteins la lumière du salon",
            None,
            &["zw:salon/ep1.level=0"],
        );
        ok(
            "Mets la lumière du salon à 40 pour cent",
            None,
            &["zw:salon/ep1.level=40"],
        );
        ok(
            "Mets la lumière du salon à quatre-vingt-dix pour cent",
            None,
            &["zw:salon/ep1.level=89"],
        );
        ok("Éteins le salon", None, &["zw:salon/ep1.level=0"]);
        ok(
            "Mets le bandeau du bureau à moitié",
            None,
            &["hue:bureau/brightness=50", "hue:bureau/on=true"],
        );
        ok(
            "Baisse la lumière du bureau",
            None,
            &["hue:bureau/brightness=60", "hue:bureau/on=true"],
        );
        // A light that is off is not turned on by « baisse ».
        passes("Baisse la lumière du salon", None, "already-lowest");
    }

    #[test]
    fn the_air_conditioner_changes_its_setpoint_only() {
        ok(
            "Mets la clim de la cuisine à vingt et un degrés",
            None,
            &["clim:cuisine/target_temperature=21"],
        );
        ok(
            "Mets la clim de la cuisine à 22",
            None,
            &["clim:cuisine/target_temperature=22"],
        );
        ok(
            "Mets la clim de la cuisine en chaud",
            None,
            &["clim:cuisine/mode=chaud", "clim:cuisine/on=true"],
        );
        passes("Mets la clim de la cuisine à 50", None, "bad-value");
        passes("Baisse la clim de la cuisine", None, "unsupported");
    }

    #[test]
    fn music_and_the_tv() {
        ok("Pause", Some("Salon"), &["sonos:salon/playing=false"]);
        ok(
            "Mets du jazz dans le salon",
            None,
            &["sonos:salon/play=jazz"],
        );
        ok(
            "Mets de la musique",
            Some("Salon"),
            &["sonos:salon/playing=true"],
        );
        ok("Monte le son", Some("Salon"), &["sonos:salon/volume=50"]);
        ok("Baisse le son", Some("Salon"), &["sonos:salon/volume=30"]);
        ok(
            "Mets le volume à 30",
            Some("Salon"),
            &["sonos:salon/volume=30"],
        );
        ok(
            "Chanson suivante",
            Some("Salon"),
            &["sonos:salon/skip=next"],
        );
        // No room said: what is playing now.
        ok("Pause", None, &["sonos:salon/playing=false"]);
        ok("Baisse le son", None, &["sonos:salon/volume=30"]);
        // Starting music needs a room: two speakers, none chosen.
        passes("Mets de la musique", None, "ambiguous");
        // What Whisper writes: « fermes », « coupe son ».
        ok(
            "Fermes les stores du salon",
            None,
            &["z:baie/control=close", "z:fenetre/control=close"],
        );
        ok("Coupe son", Some("Salon"), &["sonos:salon/mute=true"]);
        ok("Mets TF1", None, &["maison:tf1/on=true"]);
        ok("Allume la télé", None, &["maison:tele/on=true"]);
    }

    #[test]
    fn what_is_not_an_order_is_passed_on() {
        passes("Ferme les stores sauf celui du salon", None, "negation");
        passes("Ne ferme pas les stores du salon", None, "negation");
        passes(
            "Ferme les stores du salon dans dix minutes",
            None,
            "deferred",
        );
        passes("Ferme les stores ce soir", None, "deferred");
        passes("Ouvre la porte de la cuisine", None, "sensitive");
        passes("Ouvre le garage", None, "sensitive");
        passes("Désarme l'alarme", None, "sensitive");
        passes("Quel temps fait-il demain ?", None, "question");
        passes("Il fait chaud ici", None, "nothing");
        passes("Allume la lumière du grenier", None, "no-room");
        passes("Allume la lumière du garage", None, "sensitive");
        passes("Mets la lumière à 300", Some("Salon"), "bad-value");
        assert_eq!(
            interpret(
                "C'est bon, merci",
                None,
                &house(),
                &layout(),
                &Config::default()
            ),
            Outcome::End
        );
    }

    #[test]
    fn a_misheard_word_is_corrected_but_not_guessed_between_two() {
        ok(
            "Allume la lumière du sallon",
            None,
            &["zw:salon/ep1.level_restore=true"],
        );
        ok(
            "Éteint la lumière du salon",
            None,
            &["zw:salon/ep1.level=0"],
        );
        ok(
            "Ferme le stor de la cuisine",
            None,
            &["z:cuisine/control=close"],
        );
        // A word glued to the one before it: « bévitrée » for « baie vitrée ».
        ok(
            "Ferme le store de la bévitrée",
            None,
            &["z:baie/control=close"],
        );
    }

    /// A device in `quick_skip` is never ordered, by its room or by its name.
    #[test]
    fn a_skipped_device_is_never_ordered() {
        let skip = json!({ "quick_skip": ["zw:salon"] });
        let said = |text: &str| said_with(skip.clone(), text, None);
        assert_eq!(said("Éteins la lumière du salon"), Err("no-device".into()));
        assert_eq!(said("Éteins le salon"), Err("no-device".into()));
        // Everything else of the room still answers.
        assert_eq!(
            said("Ferme le store fenêtre"),
            Ok(vec!["z:fenetre/control=close".into()])
        );
        // Without the list, it is ordered.
        assert!(said_with(json!({}), "Éteins la lumière du salon", None).is_ok());
    }

    /// What a question about the house is answered, from the inventory alone.
    #[test]
    fn questions_about_the_state_are_answered() {
        let heard = |text: &str, room: Option<&str>| {
            let config = Config::default();
            match interpret(text, room, &house(), &layout(), &config) {
                Outcome::Say { text, .. } => Ok(text),
                Outcome::Pass(why) => Err(why),
                other => panic!("{other:?}"),
            }
        };
        let said = |s: &str| Ok(s.to_owned());
        assert_eq!(
            heard("Quelle est la température dans la chambre de Léa ?", None),
            said("Chambre Léa : 21.5 °C.")
        );
        assert_eq!(
            heard("Il fait combien dans le salon ?", None),
            said("Salon : 21.8 °C.")
        );
        assert_eq!(
            heard("Quelle est la température ?", Some("Salon")),
            said("Salon : 21.8 °C.")
        );
        assert_eq!(heard("Quelle est la température ?", None), Err("question"));
        assert_eq!(
            heard("Est-ce que les stores du salon sont fermés ?", None),
            said("Store baie vitrée : ouvert. Store fenêtre : fermé.")
        );
        assert_eq!(
            heard("La lumière du salon est allumée ?", None),
            said("Lumière salon : éteinte.")
        );
        assert_eq!(
            heard("La clim de la cuisine est allumée ?", None),
            said("Clim Cuisine : allumée, chaud, 21 °C.")
        );
        assert_eq!(
            heard("Est-ce que la porte de garage est ouverte ?", None),
            said("Porte de garage : fermée.")
        );
        assert_eq!(
            heard("Qu'est-ce qui est allumé ?", None),
            said("Allumé : Bandeau (80 %).")
        );
        assert_eq!(
            heard("Est-ce que la musique joue ?", Some("Salon")),
            said("Salon : joue. Move : à l'arrêt.")
        );
        // A question needs a room or a device it can name.
        assert_eq!(
            heard("Est-ce que les stores sont fermés ?", None),
            Err("question")
        );
    }

    #[test]
    fn numbers_said_in_words() {
        let lex = Lex::load();
        let digits = |s: &str| lex.digitize(&tokens(s)).join(" ");
        assert_eq!(digits("vingt et un"), "21");
        assert_eq!(digits("quatre vingt dix"), "90");
        assert_eq!(digits("soixante et onze"), "71");
        assert_eq!(digits("trente pour cent"), "30 pour cent");
        assert_eq!(digits("une lampe"), "une lampe");
    }
}

//! Météo-France: the weather vigilance of one department, from the official
//! API (portail-api.meteofrance.fr, « DonneesPubliquesVigilance », free with
//! an account).
//!
//! One device, « Vigilance », with the department's colour for today and
//! tomorrow, the colour of each phenomenon (wind, rain, storms…) and a
//! sentence that says it all, for messages. The coast of the department
//! (`dd10`, waves and submersion) counts with it. Nothing here sends anything:
//! an alert is an automation on `level` (it writes to the messengers).
//!
//! The API key lives in the vault (`moli-os secrets set <instance> api_key`),
//! never in `moli.toml`. A refused key stops the driver until it is changed:
//! asking again every few minutes would not make it better.

use std::time::Duration;

use anyhow::Context as _;
use http::{Method, Request, StatusCode};
use http_body_util::Full;
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::Deserialize;
use serde_json::Value as Json;

const HOST: &str = "public-api.meteofrance.fr";
const PATH: &str = "/public/DPVigilance/v1/cartevigilance/encours";
const API_KEY: &str = "api_key";
const TIMEOUT: Duration = Duration::from_secs(20);
/// The whole of France, two days: a few hundred kilobytes.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// The map changes twice a day, and at once when a warning starts.
const DEFAULT_POLL_SECS: u64 = 600;
const MIN_POLL_SECS: u64 = 300;

/// Météo-France's `color_id` 1 to 4.
pub const LEVELS: [&str; 4] = ["green", "yellow", "orange", "red"];
/// Point keys of the phenomena, by `phenomenon_id` 1 to 9.
const PHENOMENA: [&str; 9] = [
    "wind",
    "rain",
    "storms",
    "floods",
    "snow_ice",
    "heat",
    "cold",
    "avalanches",
    "waves",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The department (`"38"`, `"2A"`), mainland France.
    pub department: String,
    /// Seconds between two readings (5 minutes at least).
    #[serde(default = "default_poll")]
    pub poll_secs: u64,
}

fn default_poll() -> u64 {
    DEFAULT_POLL_SECS
}

#[derive(Debug)]
pub struct MeteoFrance {
    config: Config,
}

impl MeteoFrance {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        anyhow::ensure!(
            department_ok(&config.department),
            "department: a mainland department, two characters (\"38\", \"2A\")"
        );
        anyhow::ensure!(
            config.poll_secs >= MIN_POLL_SECS,
            "poll_secs: {MIN_POLL_SECS} at least"
        );
        Ok(Self { config })
    }
}

impl Driver for MeteoFrance {
    fn kind(&self) -> &'static str {
        "meteofrance"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(&self.config, ctx))
    }
}

/// `01` to `95` (no `20`), `2A`, `2B`: the vigilance map's departments.
fn department_ok(department: &str) -> bool {
    match department {
        "2A" | "2B" => true,
        d => {
            d.len() == 2
                && d.parse::<u8>()
                    .is_ok_and(|n| (1..=95).contains(&n) && n != 20)
        }
    }
}

/// One day of the department: colours 1 (green) to 4 (red).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Day {
    level: u8,
    phenomena: [u8; 9],
}

#[derive(Debug, PartialEq, Eq)]
struct Vigilance {
    updated: Option<String>,
    today: Day,
    /// Missing between midnight and 6 a.m.: the map has one day only then.
    tomorrow: Option<Day>,
}

/// A number given as a number or as text (`"3"`).
fn int(value: &Json) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn colour(value: &Json) -> u8 {
    int(value)
        .and_then(|c| u8::try_from(c).ok())
        .filter(|c| (1..=4).contains(c))
        .unwrap_or(1)
}

/// The department's day in one period of the map, with its coast.
fn day(period: &Json, department: &str) -> Option<Day> {
    let coast = format!("{department}10");
    let mut found = false;
    let mut day = Day {
        level: 1,
        phenomena: [1; 9],
    };
    for domain in period["timelaps"]["domain_ids"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let id = match &domain["domain_id"] {
            Json::String(s) => s.clone(),
            Json::Number(n) => n.to_string(),
            _ => continue,
        };
        if id != department && id != coast {
            continue;
        }
        found = true;
        day.level = day.level.max(colour(&domain["max_color_id"]));
        for item in domain["phenomenon_items"].as_array().into_iter().flatten() {
            let Some(index) = int(&item["phenomenon_id"])
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| (1..=PHENOMENA.len()).contains(i))
            else {
                continue;
            };
            let slot = &mut day.phenomena[index - 1];
            *slot = (*slot).max(colour(&item["phenomenon_max_color_id"]));
        }
    }
    let worst = day.phenomena.iter().copied().max().unwrap_or(1);
    day.level = day.level.max(worst);
    found.then_some(day)
}

/// What the map says of the department; `None` if it is not on it.
fn read(map: &Json, department: &str) -> Option<Vigilance> {
    let product = &map["product"];
    let mut today = None;
    let mut tomorrow = None;
    for period in product["periods"].as_array().into_iter().flatten() {
        match period["echeance"].as_str() {
            Some("J") => today = day(period, department),
            Some("J1") => tomorrow = day(period, department),
            _ => {}
        }
    }
    Some(Vigilance {
        updated: product["update_time"].as_str().map(str::to_owned),
        today: today?,
        tomorrow,
    })
}

fn level(colour: u8) -> &'static str {
    LEVELS[usize::from(colour.clamp(1, 4) - 1)]
}

fn colour_name(colour: u8) -> String {
    match colour {
        4 => moli_i18n::tr!("pilotes.meteofrance.rouge"),
        3 => moli_i18n::tr!("pilotes.meteofrance.orange"),
        2 => moli_i18n::tr!("pilotes.meteofrance.jaune"),
        _ => moli_i18n::tr!("pilotes.meteofrance.vert"),
    }
}

fn phenomenon_name(index: usize) -> String {
    match index {
        0 => moli_i18n::tr!("pilotes.meteofrance.vent"),
        1 => moli_i18n::tr!("pilotes.meteofrance.pluie"),
        2 => moli_i18n::tr!("pilotes.meteofrance.orages"),
        3 => moli_i18n::tr!("pilotes.meteofrance.crues"),
        4 => moli_i18n::tr!("pilotes.meteofrance.neige"),
        5 => moli_i18n::tr!("pilotes.meteofrance.canicule"),
        6 => moli_i18n::tr!("pilotes.meteofrance.grand_froid"),
        7 => moli_i18n::tr!("pilotes.meteofrance.avalanches"),
        _ => moli_i18n::tr!("pilotes.meteofrance.vagues"),
    }
}

/// « Orange : vent, orages. Jaune : canicule. », worst first.
fn summary(day: &Day) -> String {
    let mut parts = Vec::new();
    for colour in (2..=4).rev() {
        let names: Vec<String> = (0..PHENOMENA.len())
            .filter(|&i| day.phenomena[i] == colour)
            .map(phenomenon_name)
            .collect();
        if !names.is_empty() {
            parts.push(format!("{} : {}.", colour_name(colour), names.join(", ")));
        }
    }
    if parts.is_empty() {
        return moli_i18n::tr!("pilotes.meteofrance.rien");
    }
    parts.join(" ")
}

enum Fetch {
    /// The key is refused: waiting on a person.
    Refused(StatusCode),
    Failed(anyhow::Error),
}

/// The current map. The key goes in a header: it never appears in an error.
async fn fetch(key: &str) -> Result<Json, Fetch> {
    let request = Request::builder()
        .method(Method::GET)
        .uri(PATH)
        .header("apikey", key)
        .header("accept", "application/json")
        .body(Full::new(moli_net::Body::new()))
        .context("request")
        .map_err(Fetch::Failed)?;
    let (status, bytes) = moli_net::web(HOST, 443, true, request, TIMEOUT, MAX_ANSWER)
        .await
        .map_err(|e| {
            Fetch::Failed(anyhow::anyhow!(moli_i18n::tr!(
                "pilotes.meteofrance.injoignable",
                cause = e.root_cause()
            )))
        })?;
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(Fetch::Refused(status));
    }
    if !status.is_success() {
        return Err(Fetch::Failed(anyhow::anyhow!(moli_i18n::tr!(
            "pilotes.meteofrance.reponse",
            status = status.as_u16()
        ))));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        Fetch::Failed(anyhow::anyhow!(moli_i18n::tr!(
            "pilotes.meteofrance.sans_json"
        )))
    })
}

fn point(key: &str, label: String, kind: Kind) -> PointSpec {
    PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access {
            read: true,
            write: false,
        },
        unit: None,
        semantic: Semantic::Other,
    }
}

fn levels() -> Kind {
    Kind::Enum {
        values: LEVELS.iter().map(|&l| l.into()).collect(),
    }
}

fn device(ctx: &DriverCtx, id: &DeviceId, department: &str) -> Device {
    let mut points = vec![
        point(
            "level",
            moli_i18n::tr!("pilotes.meteofrance.niveau"),
            levels(),
        ),
        point(
            "summary",
            moli_i18n::tr!("pilotes.meteofrance.resume"),
            Kind::Text,
        ),
        point(
            "level_tomorrow",
            moli_i18n::tr!("pilotes.meteofrance.niveau_demain"),
            levels(),
        ),
        point(
            "summary_tomorrow",
            moli_i18n::tr!("pilotes.meteofrance.resume_demain"),
            Kind::Text,
        ),
        point(
            "updated",
            moli_i18n::tr!("pilotes.meteofrance.mise_a_jour"),
            Kind::Text,
        ),
    ];
    for (index, key) in PHENOMENA.iter().enumerate() {
        points.push(point(key, phenomenon_name(index), levels()));
    }
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: moli_i18n::tr!("pilotes.meteofrance.nom").into(),
        manufacturer: Some("Météo-France".into()),
        model: Some(moli_i18n::tr!("pilotes.meteofrance.modele", department = department).into()),
        description: Some(moli_i18n::tr!("pilotes.meteofrance.description").into()),
        native_room: None,
        members: Vec::new(),
        points,
    }
}

fn publish(ctx: &DriverCtx, id: &DeviceId, vigilance: &Vigilance) {
    let today = &vigilance.today;
    ctx.set_state(id, "level", Value::from(level(today.level)));
    ctx.set_state(id, "summary", Value::from(summary(today).as_str()));
    for (index, key) in PHENOMENA.iter().enumerate() {
        ctx.set_state(id, key, Value::from(level(today.phenomena[index])));
    }
    // At night the map has no tomorrow: yesterday's « tomorrow » still holds.
    if let Some(tomorrow) = &vigilance.tomorrow {
        ctx.set_state(id, "level_tomorrow", Value::from(level(tomorrow.level)));
        ctx.set_state(
            id,
            "summary_tomorrow",
            Value::from(summary(tomorrow).as_str()),
        );
    }
    if let Some(updated) = &vigilance.updated {
        ctx.set_state(id, "updated", Value::from(updated.as_str()));
    }
}

async fn run(config: &Config, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let Some(key) = ctx.secret(API_KEY).filter(|k| !k.trim().is_empty()) else {
        ctx.wait_for(moli_i18n::tr!(
            "pilotes.meteofrance.pas_de_cle",
            instance = ctx.instance()
        ));
        ctx.cancelled().await;
        return Ok(());
    };
    let key = key.trim().to_owned();
    let id = ctx.device_id("vigilance");
    ctx.upsert_device(device(ctx, &id, &config.department));
    let poll = Duration::from_secs(config.poll_secs);
    loop {
        let map = match fetch(&key).await {
            Ok(map) => map,
            Err(Fetch::Refused(status)) => {
                ctx.set_availability(&id, false);
                ctx.wait_for(moli_i18n::tr!(
                    "pilotes.meteofrance.cle_refusee",
                    status = status.as_u16(),
                    instance = ctx.instance()
                ));
                ctx.cancelled().await;
                return Ok(());
            }
            Err(Fetch::Failed(e)) => return Err(e),
        };
        let Some(vigilance) = read(&map, &config.department) else {
            anyhow::bail!(moli_i18n::tr!(
                "pilotes.meteofrance.departement_absent",
                department = config.department
            ));
        };
        publish(ctx, &id, &vigilance);
        ctx.set_availability(&id, true);
        ctx.ready();
        tokio::select! {
            () = ctx.cancelled() => return Ok(()),
            () = tokio::time::sleep(poll) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str =
        include_str!("../../../integrations/meteofrance/fixtures/cartevigilance.json");

    fn map() -> Json {
        serde_json::from_str(MAP).unwrap()
    }

    #[test]
    fn departments_are_the_mainland_ones() {
        for ok in ["01", "38", "95", "2A", "2B"] {
            assert!(department_ok(ok), "{ok}");
        }
        for no in ["", "1", "00", "20", "96", "971", "2C", "AB", "380"] {
            assert!(!department_ok(no), "{no}");
        }
    }

    #[test]
    fn a_department_is_read_with_its_coast() {
        let v = read(&map(), "13").unwrap();
        assert_eq!(v.updated.as_deref(), Some("2026-01-15T06:02:11Z"));
        // Inland: yellow wind; on the coast: orange waves.
        assert_eq!(v.today.level, 3);
        assert_eq!(v.today.phenomena[0], 2);
        assert_eq!(v.today.phenomena[8], 3);
        assert_eq!(v.today.phenomena[2], 1);
        let tomorrow = v.tomorrow.unwrap();
        assert_eq!(tomorrow.level, 2);
        assert_eq!(level(tomorrow.level), "yellow");
    }

    #[test]
    fn ids_may_be_numbers_or_text() {
        // « 38 » is given with numbers (domain and phenomenon ids, colours).
        let v = read(&map(), "38").unwrap();
        assert_eq!(v.today.phenomena[4], 2);
        assert_eq!(v.today.level, 2);
        assert_eq!(v.tomorrow.unwrap().level, 1);
    }

    #[test]
    fn a_department_off_the_map_is_none() {
        assert_eq!(read(&map(), "75"), None);
        assert_eq!(read(&serde_json::json!({}), "13"), None);
    }

    #[test]
    fn the_night_map_has_no_tomorrow() {
        let mut night = map();
        night["product"]["periods"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["echeance"] == "J");
        let v = read(&night, "13").unwrap();
        assert_eq!(v.tomorrow, None);
    }

    #[test]
    fn the_summary_says_the_worst_first() {
        let v = read(&map(), "13").unwrap();
        let text = summary(&v.today);
        let orange = text.find(&colour_name(3)).unwrap();
        let yellow = text.find(&colour_name(2)).unwrap();
        assert!(orange < yellow, "{text}");
        assert!(text.contains(&phenomenon_name(8)) && text.contains(&phenomenon_name(0)));
        let calm = Day {
            level: 1,
            phenomena: [1; 9],
        };
        assert_eq!(summary(&calm), moli_i18n::tr!("pilotes.meteofrance.rien"));
    }

    #[test]
    fn odd_colours_count_as_green() {
        assert_eq!(colour(&serde_json::json!(7)), 1);
        assert_eq!(colour(&serde_json::json!("x")), 1);
        assert_eq!(colour(&serde_json::json!("4")), 4);
    }

    #[test]
    fn a_driver_needs_a_department_and_a_sane_pace() {
        let config = |department: &str, poll_secs| Config {
            department: department.into(),
            poll_secs,
        };
        assert!(MeteoFrance::new(config("38", DEFAULT_POLL_SECS)).is_ok());
        assert!(MeteoFrance::new(config("Isère", DEFAULT_POLL_SECS)).is_err());
        assert!(MeteoFrance::new(config("38", 60)).is_err());
    }
}

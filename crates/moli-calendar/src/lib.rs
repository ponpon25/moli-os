//! School calendar: the school holidays of one academy and the public
//! holidays, from the official French open data (no account, no key).
//!
//! - School holidays: data.education.gouv.fr, dataset
//!   `fr-en-calendrier-scolaire` (one record per break and academy).
//! - Public holidays: calendrier.api.gouv.fr (`jours-feries/<zone>.json`).
//!
//! One device, « Calendrier scolaire », says whether today (and tomorrow) is
//! a school day: Monday to Friday, neither a public holiday nor in a break.
//! Which weekdays a child really goes to school (Wednesday…) belongs to the
//! automation, not here. Both lists cover months ahead: they are read once a
//! day and kept on disk, so a morning without Internet still knows the day.
//! Nothing here acts: an automation reads `school_day`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Value};
use moli_runtime::{BoxFuture, Driver, DriverCtx};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

const SCHOOL_HOST: &str = "data.education.gouv.fr";
const SCHOOL_PATH: &str = "/api/explore/v2.1/catalog/datasets/fr-en-calendrier-scolaire/records";
const PUBLIC_HOST: &str = "calendrier.api.gouv.fr";
/// The lists change a few times a year.
const REFRESH: Duration = Duration::from_secs(24 * 3600);
/// After a failed reading.
const RETRY: Duration = Duration::from_secs(30 * 60);
/// The day changes at midnight: looked at every minute (the hub only passes
/// on a change).
const TICK: Duration = Duration::from_secs(60);
/// The public holidays' areas of calendrier.api.gouv.fr.
pub const ZONES: [&str; 13] = [
    "metropole",
    "alsace-moselle",
    "guadeloupe",
    "guyane",
    "la-reunion",
    "martinique",
    "mayotte",
    "nouvelle-caledonie",
    "polynesie-francaise",
    "saint-barthelemy",
    "saint-martin",
    "saint-pierre-et-miquelon",
    "wallis-et-futuna",
];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The academy as the dataset writes it (`location`): `"Grenoble"`,
    /// `"Paris"`, `"Lyon"`…
    pub location: String,
    /// The public holidays' area (`metropole`, `alsace-moselle`…).
    #[serde(default = "default_zone")]
    pub zone: String,
    /// Where the days are counted.
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

fn default_zone() -> String {
    "metropole".into()
}

fn default_timezone() -> String {
    "Europe/Paris".into()
}

#[derive(Debug)]
pub struct Calendar {
    config: Config,
    tz: TimeZone,
    cache: PathBuf,
}

impl Calendar {
    /// `data_dir`: where the lists are kept between two starts.
    pub fn new(config: Config, data_dir: &Path) -> anyhow::Result<Self> {
        check(&config)?;
        let tz = TimeZone::get(&config.timezone)
            .map_err(|_| anyhow::anyhow!("timezone: unknown ({:?})", config.timezone))?;
        let cache = data_dir.join(format!("calendar-{}.json", file_safe(&config.location)));
        Ok(Self { config, tz, cache })
    }
}

/// What a configuration must be.
pub fn check(config: &Config) -> anyhow::Result<()> {
    let location = config.location.trim();
    anyhow::ensure!(
        !location.is_empty()
            && location.len() <= 60
            && location
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'')),
        "location: the academy, as the dataset writes it (\"Grenoble\")"
    );
    anyhow::ensure!(
        ZONES.contains(&config.zone.as_str()),
        "zone: one of {}",
        ZONES.join(", ")
    );
    Ok(())
}

fn file_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

impl Driver for Calendar {
    fn kind(&self) -> &'static str {
        "calendar"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(run(self, ctx))
    }
}

/// One school break: from `first` to the day before `until`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Break {
    name: String,
    first: String,
    until: String,
}

/// What is known, kept on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Known {
    breaks: Vec<Break>,
    /// `YYYY-MM-DD` → name.
    holidays: BTreeMap<String, String>,
}

fn date(text: &str) -> Option<Date> {
    text.parse().ok()
}

/// The breaks of the dataset's answer. Dates are midnights in UTC of the
/// local day (« 2026-10-16T22:00:00+00:00 » is the 17th in Paris); the end
/// is the day classes start again. A break whose end is its start (« Pont
/// de l'Ascension ») is that one day. Teachers' own dates are left out.
fn breaks(answer: &Json, tz: &TimeZone) -> Vec<Break> {
    let local = |field: &Json| -> Option<Date> {
        let at: Timestamp = field.as_str()?.parse().ok()?;
        Some(at.to_zoned(tz.clone()).date())
    };
    let mut out: Vec<Break> = answer["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["population"].as_str() != Some("Enseignants"))
        .filter_map(|r| {
            let first = local(&r["start_date"])?;
            let end = local(&r["end_date"])?;
            let until = if end > first {
                end
            } else {
                first.tomorrow().ok()?
            };
            Some(Break {
                name: r["description"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                first: first.to_string(),
                until: until.to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.first.cmp(&b.first));
    out.dedup();
    out
}

/// `{"2026-11-11": "11 novembre", …}`.
fn holidays(answer: &Json) -> BTreeMap<String, String> {
    answer
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(day, _)| date(day).is_some())
        .filter_map(|(day, name)| Some((day.clone(), name.as_str()?.to_owned())))
        .collect()
}

/// One day as the device shows it.
#[derive(Debug, PartialEq, Eq)]
struct Status {
    school_day: bool,
    vacation: Option<String>,
    holiday: Option<String>,
}

impl Known {
    fn vacation(&self, day: Date) -> Option<&Break> {
        self.breaks.iter().find(|b| {
            matches!((date(&b.first), date(&b.until)), (Some(first), Some(until)) if first <= day && day < until)
        })
    }

    fn day(&self, day: Date) -> Status {
        let vacation = self.vacation(day).map(|b| b.name.clone());
        let holiday = self.holidays.get(&day.to_string()).cloned();
        let weekend = matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday);
        Status {
            school_day: !weekend && vacation.is_none() && holiday.is_none(),
            vacation,
            holiday,
        }
    }

    /// The next break starting after `day`.
    fn next_break(&self, day: Date) -> Option<&Break> {
        self.breaks
            .iter()
            .find(|b| date(&b.first).is_some_and(|first| first > day))
    }

    /// Whether `day` is within what the lists cover: past their end, a
    /// « school day » could be a break nobody has published yet.
    fn covers(&self, day: Date) -> bool {
        let breaks = self
            .breaks
            .iter()
            .filter_map(|b| date(&b.until))
            .any(|until| until > day);
        let holidays = self
            .holidays
            .keys()
            .filter_map(|d| date(d))
            .any(|d| d.year() == day.year());
        breaks && holidays
    }
}

fn percent(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn get(host: &str, path: &str) -> anyhow::Result<Json> {
    let (status, body) = moli_net::https_get(host, path, None, &[])
        .await
        .map_err(|e| {
            anyhow::anyhow!(moli_i18n::tr!(
                "pilotes.calendar.injoignable",
                host = host,
                cause = e.root_cause()
            ))
        })?;
    anyhow::ensure!(
        status.is_success(),
        moli_i18n::tr!(
            "pilotes.calendar.reponse",
            host = host,
            status = status.as_u16()
        )
    );
    serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!(moli_i18n::tr!("pilotes.calendar.sans_json", host = host)))
}

/// Both lists, from `today` on.
async fn fetch(config: &Config, tz: &TimeZone, today: Date) -> anyhow::Result<Known> {
    let filter = format!(
        "location=\"{}\" and end_date>=\"{today}\"",
        config.location.trim()
    );
    let path = format!(
        "{SCHOOL_PATH}?where={}&order_by=start_date&limit=100",
        percent(&filter)
    );
    let school = get(SCHOOL_HOST, &path).await?;
    let public = get(PUBLIC_HOST, &format!("/jours-feries/{}.json", config.zone)).await?;
    Ok(Known {
        breaks: breaks(&school, tz),
        holidays: holidays(&public),
    })
}

async fn load(path: &Path) -> Option<Known> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn save(path: &Path, known: &Known) {
    let Ok(text) = serde_json::to_vec_pretty(known) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    let written = async {
        tokio::fs::write(&tmp, &text).await?;
        tokio::fs::rename(&tmp, path).await
    };
    if let Err(e) = written.await {
        tracing::warn!(error = %e, "calendar: the lists could not be kept on disk");
    }
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

fn device(ctx: &DriverCtx, id: &DeviceId, location: &str) -> Device {
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: moli_i18n::tr!("pilotes.calendar.nom").into(),
        manufacturer: Some("Éducation nationale".into()),
        model: Some(moli_i18n::tr!("pilotes.calendar.modele", location = location).into()),
        description: Some(moli_i18n::tr!("pilotes.calendar.description").into()),
        native_room: None,
        members: Vec::new(),
        points: vec![
            point(
                "school_day",
                moli_i18n::tr!("pilotes.calendar.jour_ecole"),
                Kind::Binary,
            ),
            point(
                "school_day_tomorrow",
                moli_i18n::tr!("pilotes.calendar.jour_ecole_demain"),
                Kind::Binary,
            ),
            point(
                "vacation",
                moli_i18n::tr!("pilotes.calendar.vacances"),
                Kind::Binary,
            ),
            point(
                "vacation_name",
                moli_i18n::tr!("pilotes.calendar.vacances_nom"),
                Kind::Text,
            ),
            point(
                "public_holiday",
                moli_i18n::tr!("pilotes.calendar.ferie"),
                Kind::Binary,
            ),
            point(
                "public_holiday_name",
                moli_i18n::tr!("pilotes.calendar.ferie_nom"),
                Kind::Text,
            ),
            point(
                "next_vacation",
                moli_i18n::tr!("pilotes.calendar.prochaines_vacances"),
                Kind::Text,
            ),
        ],
    }
}

fn text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// What the device shows on `today`. `false` when the lists do not cover it.
fn publish(ctx: &DriverCtx, id: &DeviceId, known: &Known, today: Date) -> bool {
    if !known.covers(today) {
        return false;
    }
    let now = known.day(today);
    ctx.set_state(id, "school_day", Value::Bool(now.school_day));
    ctx.set_state(id, "vacation", Value::Bool(now.vacation.is_some()));
    ctx.set_state(id, "vacation_name", text(now.vacation.as_deref()));
    ctx.set_state(id, "public_holiday", Value::Bool(now.holiday.is_some()));
    ctx.set_state(id, "public_holiday_name", text(now.holiday.as_deref()));
    if let Ok(tomorrow) = today.tomorrow()
        && known.covers(tomorrow)
    {
        ctx.set_state(
            id,
            "school_day_tomorrow",
            Value::Bool(known.day(tomorrow).school_day),
        );
    }
    let next = known.next_break(today).and_then(|b| {
        let first = date(&b.first)?;
        Some(moli_i18n::tr!(
            "pilotes.calendar.a_partir_du",
            name = b.name,
            day = first.strftime("%d/%m/%Y")
        ))
    });
    ctx.set_state(id, "next_vacation", text(next.as_deref()));
    true
}

async fn run(me: &Calendar, ctx: &mut DriverCtx) -> anyhow::Result<()> {
    let today = || Timestamp::now().to_zoned(me.tz.clone()).date();
    let id = ctx.device_id("calendar");
    ctx.upsert_device(device(ctx, &id, me.config.location.trim()));
    let mut known = load(&me.cache).await.unwrap_or_default();
    let mut shown = publish(ctx, &id, &known, today());
    ctx.set_availability(&id, shown);
    if shown {
        ctx.ready();
    }
    let mut next_fetch = tokio::time::Instant::now();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            () = ctx.cancelled() => return Ok(()),
            _ = tick.tick() => {}
        }
        if tokio::time::Instant::now() >= next_fetch {
            match fetch(&me.config, &me.tz, today()).await {
                Ok(fresh) if fresh.breaks.is_empty() => {
                    ctx.set_availability(&id, false);
                    ctx.wait_for(moli_i18n::tr!(
                        "pilotes.calendar.academie_inconnue",
                        location = me.config.location.trim()
                    ));
                    ctx.cancelled().await;
                    return Ok(());
                }
                Ok(fresh) => {
                    if fresh != known {
                        save(&me.cache, &fresh).await;
                        known = fresh;
                    }
                    next_fetch = tokio::time::Instant::now() + REFRESH;
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "calendar: reading failed, the kept lists still hold");
                    next_fetch = tokio::time::Instant::now() + RETRY;
                }
            }
        }
        let now_shown = publish(ctx, &id, &known, today());
        if now_shown != shown {
            ctx.set_availability(&id, now_shown);
            shown = now_shown;
        }
        if shown {
            ctx.ready();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHOOL: &str =
        include_str!("../../../integrations/calendar/fixtures/calendrier-scolaire.json");
    const PUBLIC: &str = include_str!("../../../integrations/calendar/fixtures/jours-feries.json");

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn known() -> Known {
        Known {
            breaks: breaks(&serde_json::from_str(SCHOOL).unwrap(), &paris()),
            holidays: holidays(&serde_json::from_str(PUBLIC).unwrap()),
        }
    }

    fn d(text: &str) -> Date {
        text.parse().unwrap()
    }

    #[test]
    fn breaks_are_local_days_without_the_teachers() {
        let k = known();
        assert_eq!(
            k.breaks[0],
            Break {
                name: "Vacances de la Toussaint".into(),
                first: "2026-10-17".into(),
                until: "2026-11-02".into(),
            }
        );
        assert_eq!(k.breaks.len(), 4, "{:?}", k.breaks);
    }

    #[test]
    fn a_break_is_off_from_its_saturday_to_the_day_before_school() {
        let k = known();
        assert!(
            k.day(d("2026-10-16")).vacation.is_none(),
            "last day of school"
        );
        assert_eq!(
            k.day(d("2026-10-19")).vacation.as_deref(),
            Some("Vacances de la Toussaint")
        );
        assert!(!k.day(d("2026-10-30")).school_day);
        let monday = k.day(d("2026-11-02"));
        assert!(
            monday.school_day && monday.vacation.is_none(),
            "back to school"
        );
    }

    #[test]
    fn a_bridge_of_one_day_is_that_day() {
        let k = known();
        assert!(
            !k.day(d("2027-05-07")).school_day,
            "the Friday after Ascension"
        );
        assert!(k.day(d("2027-05-10")).school_day);
        assert_eq!(k.day(d("2027-05-06")).holiday.as_deref(), Some("Ascension"));
    }

    #[test]
    fn weekends_and_public_holidays_are_no_school_days() {
        let k = known();
        assert!(k.day(d("2026-11-10")).school_day, "a Tuesday");
        let armistice = k.day(d("2026-11-11"));
        assert!(!armistice.school_day);
        assert_eq!(armistice.holiday.as_deref(), Some("11 novembre"));
        assert!(!k.day(d("2026-11-14")).school_day, "a Saturday");
        assert!(k.day(d("2026-11-18")).school_day, "a Wednesday counts");
    }

    #[test]
    fn days_past_the_lists_are_not_guessed() {
        let k = known();
        assert!(k.covers(d("2026-11-09")));
        assert!(!k.covers(d("2028-01-10")), "no break and no holiday known");
        assert!(!Known::default().covers(d("2026-11-09")));
    }

    #[test]
    fn the_next_break_is_the_one_ahead() {
        let k = known();
        assert_eq!(
            k.next_break(d("2026-11-09")).map(|b| b.name.as_str()),
            Some("Vacances de Noël")
        );
    }

    #[test]
    fn a_configuration_names_an_academy_and_a_zone() {
        let config = |location: &str, zone: &str| Config {
            location: location.into(),
            zone: zone.into(),
            timezone: default_timezone(),
        };
        assert!(check(&config("Grenoble", "metropole")).is_ok());
        assert!(check(&config("Clermont-Ferrand", "metropole")).is_ok());
        assert!(check(&config("", "metropole")).is_err());
        assert!(check(&config("Lyon\" or 1=1", "metropole")).is_err());
        assert!(check(&config("Lyon", "mars")).is_err());
    }

    #[test]
    fn the_filter_is_encoded() {
        assert_eq!(percent("a=\"b c\""), "a%3D%22b%20c%22");
    }
}

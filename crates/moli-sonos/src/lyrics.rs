//! Lyrics of the song a speaker plays, from LRCLIB (an open service of
//! lyrics, timed when it can): Apple keeps its own for itself. Only the
//! song's name, artist, album and length leave the house, to that one host.

use anyhow::bail;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::apple::percent;

const HOST: &str = "lrclib.net";

/// What the dashboard asks: which song, and what to look it up by.
#[derive(Deserialize)]
pub(crate) struct Ask {
    pub song: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    pub duration: Option<i64>,
}

/// One line, with the second it starts at when the lyrics are timed. A timed
/// line without words marks a pause of the singer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Line {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    pub text: String,
}

/// The lyrics of a song (empty: none known). A song has many editions, each
/// with its own timing: the timed lyrics of the edition whose length is the
/// closest to the one playing (within a couple of seconds, and the same
/// album first), else the plain text of any.
pub(crate) async fn fetch(ask: &Ask) -> anyhow::Result<Vec<Line>> {
    let path = format!(
        "/api/search?track_name={}&artist_name={}",
        percent(&ask.title),
        percent(&ask.artist)
    );
    let mut answer = get(&path).await?;
    // The service says when it is busy: once more, a moment later.
    if answer.0 == 503 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        answer = get(&path).await?;
    }
    match answer.0 {
        404 => Ok(Vec::new()),
        200..=299 => {
            let found: Json = serde_json::from_slice(&answer.1)?;
            match choose(&found, ask) {
                Some(edition) => parse(edition.to_string().as_bytes()),
                None => Ok(Vec::new()),
            }
        }
        status => bail!(
            "{}",
            moli_i18n::tr!("pilotes.sonos.paroles_refus", status = status)
        ),
    }
}

/// How far two lengths may be for the timing of one to be the other's.
const SAME_EDITION: f64 = 2.0;

/// The best edition among the search results, with its timing only when it is
/// the playing one's.
fn choose(found: &Json, ask: &Ask) -> Option<Json> {
    #[allow(clippy::cast_precision_loss)]
    let playing = ask.duration.filter(|s| *s > 0).map(|s| s as f64);
    let gap = |edition: &Json| match (playing, edition["duration"].as_f64()) {
        (Some(a), Some(b)) => (a - b).abs(),
        _ => 0.0,
    };
    let same_album = |edition: &Json| {
        !ask.album.is_empty()
            && edition["albumName"]
                .as_str()
                .is_some_and(|a| a.eq_ignore_ascii_case(&ask.album))
    };
    let has = |edition: &Json, field: &str| {
        edition[field]
            .as_str()
            .is_some_and(|t| !t.trim().is_empty())
    };
    let mut editions: Vec<&Json> = found
        .as_array()?
        .iter()
        .filter(|e| e["instrumental"].as_bool() != Some(true))
        .collect();
    editions.sort_by(|a, b| {
        gap(a)
            .partial_cmp(&gap(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(same_album(b).cmp(&same_album(a)))
    });
    if let Some(timed) = editions
        .iter()
        .find(|e| has(e, "syncedLyrics") && gap(e) <= SAME_EDITION)
    {
        return Some((*timed).clone());
    }
    // No timing that fits: the words alone, from the nearest edition.
    let mut plain = (*editions.iter().find(|e| has(e, "plainLyrics"))?).clone();
    plain["syncedLyrics"] = Json::Null;
    Some(plain)
}

async fn get(path: &str) -> anyhow::Result<(u16, Vec<u8>)> {
    let (status, body) = moli_net::https_get(
        HOST,
        path,
        None,
        &[("user-agent", "Moli-OS (home automation)")],
    )
    .await?;
    Ok((status.as_u16(), body.to_vec()))
}

/// Seconds of an `mm:ss.xx` stamp.
fn stamp(text: &str) -> Option<f64> {
    let (minutes, seconds) = text.split_once(':')?;
    Some(minutes.trim().parse::<f64>().ok()? * 60.0 + seconds.trim().parse::<f64>().ok()?)
}

/// The lines of an answer: the timed ones (`[mm:ss.xx] words`) when there
/// are, else the plain text.
pub(crate) fn parse(body: &[u8]) -> anyhow::Result<Vec<Line>> {
    let json: Json = serde_json::from_slice(body)?;
    if json["instrumental"].as_bool() == Some(true) {
        return Ok(Vec::new());
    }
    if let Some(timed) = json["syncedLyrics"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
    {
        let mut lines = Vec::new();
        for raw in timed.lines() {
            // Several stamps may share one line of words: `[00:10.0][01:20.0] words`.
            let mut rest = raw.trim();
            let mut starts = Vec::new();
            while let Some(inner) = rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
                if let Some(t) = stamp(inner.0) {
                    starts.push(t);
                }
                rest = inner.1;
            }
            let text = rest.split_whitespace().collect::<Vec<_>>().join(" ");
            lines.extend(starts.into_iter().map(|t| Line {
                t: Some(t),
                text: text.clone(),
            }));
        }
        lines.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        return Ok(lines);
    }
    Ok(json["plainLyrics"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .map(|text| Line { t: None, text })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_timed_lines_and_pauses() {
        let body = serde_json::json!({
            "syncedLyrics": "[00:08.50] Un deux\n[00:12.00]\n[01:02.25] Trois",
            "plainLyrics": "Un deux\nTrois"
        })
        .to_string();
        assert_eq!(
            parse(body.as_bytes()).unwrap(),
            [
                Line {
                    t: Some(8.5),
                    text: "Un deux".into()
                },
                Line {
                    t: Some(12.0),
                    text: String::new()
                },
                Line {
                    t: Some(62.25),
                    text: "Trois".into()
                },
            ]
        );
    }

    #[test]
    fn picks_the_edition_of_the_playing_length() {
        let ask = Ask {
            song: "1".into(),
            title: "T".into(),
            artist: "A".into(),
            album: "Album".into(),
            duration: Some(184),
        };
        let found = serde_json::json!([
            {"id": 1, "albumName": "Other", "duration": 192.0, "syncedLyrics": "[00:01.0] a", "plainLyrics": "a"},
            {"id": 2, "albumName": "Album", "duration": 185.0, "syncedLyrics": "[00:02.0] b", "plainLyrics": "b"},
            {"id": 3, "albumName": "Album (Explicit)", "duration": 184.0, "syncedLyrics": "[00:03.0] c", "plainLyrics": "c"}
        ]);
        assert_eq!(choose(&found, &ask).unwrap()["id"], 3);
        // No edition of that length: the plain words of the nearest, untimed.
        let far = Ask {
            duration: Some(150),
            ..ask
        };
        let words = choose(&found, &far).unwrap();
        assert_eq!(words["id"], 3);
        assert!(words["syncedLyrics"].is_null());
    }

    #[test]
    fn falls_back_to_plain_text_and_knows_instrumentals() {
        let plain = serde_json::json!({"syncedLyrics": null, "plainLyrics": "A\n\nB"}).to_string();
        assert_eq!(
            parse(plain.as_bytes()).unwrap(),
            [
                Line {
                    t: None,
                    text: "A".into()
                },
                Line {
                    t: None,
                    text: "B".into()
                }
            ]
        );
        let instrumental =
            serde_json::json!({"instrumental": true, "plainLyrics": "x"}).to_string();
        assert!(parse(instrumental.as_bytes()).unwrap().is_empty());
    }
}

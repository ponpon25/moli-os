//! Lyrics of the song a speaker plays, from LRCLIB (an open service of
//! lyrics, timed when it can): Apple keeps its own for itself. Only the
//! song's name, artist, album and length leave the house, to that one host.

use std::fmt::Write as _;

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

/// The lyrics of a song (empty: none known).
pub(crate) async fn fetch(ask: &Ask) -> anyhow::Result<Vec<Line>> {
    let by_name = format!(
        "/api/get?track_name={}&artist_name={}",
        percent(&ask.title),
        percent(&ask.artist)
    );
    // The exact edition first (its album and length), then any of the song.
    let mut exact = by_name.clone();
    if !ask.album.is_empty() {
        write!(exact, "&album_name={}", percent(&ask.album))?;
    }
    if let Some(seconds) = ask.duration.filter(|s| *s > 0) {
        write!(exact, "&duration={seconds}")?;
    }
    for path in [exact, by_name] {
        let mut answer = get(&path).await?;
        // The service says when it is busy: once more, a moment later.
        if answer.0 == 503 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            answer = get(&path).await?;
        }
        match answer.0 {
            404 => {}
            200..=299 => return parse(&answer.1),
            status => bail!(
                "{}",
                moli_i18n::tr!("pilotes.sonos.paroles_refus", status = status)
            ),
        }
    }
    Ok(Vec::new())
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

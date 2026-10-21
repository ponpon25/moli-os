//! Apple Music: the catalogue searched through the Apple Music API
//! (MusicKit), with a developer token Moli signs itself from the house's own
//! key, and what is found turned into what a Sonos plays. The music itself
//! never passes through Moli: the speaker plays it from Apple.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use moli_net::es256::Signer;
use moli_net::upnp::escape;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

const HOST: &str = "api.music.apple.com";
/// A token is signed for this long, and signed again at half of it.
const TOKEN_LIFE: Duration = Duration::from_secs(12 * 3600);
/// What Sonos calls Apple Music.
const SERVICE: &str = "204";
/// The service descriptor Sonos puts in what it plays for a service
/// (`SA_RINCON` + service * 256 + 7).
const DESCRIPTOR: &str = "SA_RINCON52231_X_#Svc52231-0-Token";

/// `[driver.options.apple]`: who the house is for Apple. The private key
/// itself is a secret (`apple_music_key`), never here.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub team_id: String,
    pub key_id: String,
    /// The catalogue searched (a country).
    #[serde(default = "storefront")]
    pub storefront: String,
}

fn storefront() -> String {
    "fr".into()
}

/// One search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Hit {
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

/// What to send a speaker for a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Playable {
    pub uri: String,
    pub meta: String,
    /// Plays from the queue (a track, an album, a playlist); a station
    /// plays as it is.
    pub queue: bool,
}

pub(crate) struct AppleMusic {
    config: Config,
    signer: Signer,
    token: Mutex<Option<(String, Instant)>>,
}

impl std::fmt::Debug for AppleMusic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleMusic")
            .field("team_id", &self.config.team_id)
            .field("key_id", &self.config.key_id)
            .finish_non_exhaustive()
    }
}

impl AppleMusic {
    /// From the house's configuration and the private key (the `.p8` file's
    /// text, as Apple gives it).
    pub(crate) fn new(config: Config, pem: &str) -> anyhow::Result<Self> {
        Ok(Self {
            config,
            signer: Signer::from_pem(pem).context("the Apple key")?,
            token: Mutex::new(None),
        })
    }

    /// The developer token, signed again when it grows old.
    fn token(&self) -> anyhow::Result<String> {
        let mut kept = self.token.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((jwt, at)) = kept.as_ref()
            && at.elapsed() < TOKEN_LIFE / 2
        {
            return Ok(jwt.clone());
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let jwt = self.signer.jwt(
            &self.config.key_id,
            &self.config.team_id,
            now,
            TOKEN_LIFE.as_secs(),
        )?;
        *kept = Some((jwt.clone(), Instant::now()));
        Ok(jwt)
    }

    /// What a person has in Apple Music (`what`: `recent`, `playlists`,
    /// `albums`, `recommendations`), asked with their own token.
    pub(crate) async fn browse(&self, what: &str, user_token: &str) -> anyhow::Result<Vec<Hit>> {
        let path = match what {
            // Apple allows 10 at most here.
            "recent" => "/v1/me/recent/played?limit=10&l=fr-FR",
            "playlists" => "/v1/me/library/playlists?limit=40&include=catalog&l=fr-FR",
            "albums" => "/v1/me/library/albums?limit=40&include=catalog&l=fr-FR",
            "recommendations" => "/v1/me/recommendations?limit=8&l=fr-FR",
            _ => bail!("{}", moli_i18n::tr!("pilotes.sonos.lecture_inconnue")),
        };
        let token = self.token()?;
        let (status, body) = moli_net::https_get(
            HOST,
            path,
            Some(&token),
            &[("music-user-token", user_token)],
        )
        .await?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            bail!("{}", moli_i18n::tr!("pilotes.sonos.apple_deconnecte"));
        }
        if !status.is_success() {
            bail!(
                "{}",
                moli_i18n::tr!("pilotes.sonos.apple_refus", status = status.as_u16())
            );
        }
        parse_library(&body)
    }

    /// The songs an artist is best known for.
    pub(crate) async fn top_songs(&self, artist: &str) -> anyhow::Result<Vec<(String, String)>> {
        if !clean(artist) {
            bail!("{}", moli_i18n::tr!("pilotes.sonos.lecture_inconnue"));
        }
        let path = format!(
            "/v1/catalog/{}/artists/{artist}/view/top-songs?limit=12&l=fr-FR",
            self.config.storefront
        );
        let token = self.token()?;
        let (status, body) = moli_net::https_get(HOST, &path, Some(&token), &[]).await?;
        if !status.is_success() {
            bail!(
                "{}",
                moli_i18n::tr!("pilotes.sonos.apple_refus", status = status.as_u16())
            );
        }
        parse_songs(&body)
    }

    /// Songs, albums, playlists, artists and stations of the catalogue for `term`.
    pub(crate) async fn search(&self, term: &str) -> anyhow::Result<Vec<Hit>> {
        let term = term.trim();
        if term.is_empty() || term.len() > 200 {
            bail!("{}", moli_i18n::tr!("pilotes.sonos.recherche_vide"));
        }
        let path = format!(
            "/v1/catalog/{}/search?term={}&types=songs,albums,playlists,artists,stations&limit=4&l=fr-FR",
            self.config.storefront,
            percent(term)
        );
        let token = self.token()?;
        let (status, body) = moli_net::https_get(HOST, &path, Some(&token), &[]).await?;
        if !status.is_success() {
            bail!(
                "{}",
                moli_i18n::tr!("pilotes.sonos.apple_refus", status = status.as_u16())
            );
        }
        parse_hits(&body)
    }
}

// ---- the developer token ---------------------------------------------------

/// Percent-encodes a query value.
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

// ---- what the search says ----------------------------------------------------

/// The songs, albums and playlists of a search answer.
pub(crate) fn parse_hits(body: &[u8]) -> anyhow::Result<Vec<Hit>> {
    let json: Json = serde_json::from_slice(body).context("Apple's answer is not JSON")?;
    let mut hits = Vec::new();
    for (section, kind, who) in [
        ("songs", "song", "artistName"),
        ("albums", "album", "artistName"),
        ("playlists", "playlist", "curatorName"),
        ("artists", "artist", ""),
        ("stations", "station", ""),
    ] {
        let Some(data) = json["results"][section]["data"].as_array() else {
            continue;
        };
        for item in data {
            let attributes = &item["attributes"];
            let (Some(id), Some(title)) = (item["id"].as_str(), attributes["name"].as_str()) else {
                continue;
            };
            // Only a live station plays on a Sonos; the ones made for a taste
            // (« Station pop ») do not, so they are not offered.
            if kind == "station" && attributes["isLive"].as_bool() != Some(true) {
                continue;
            }
            let subtitle = match kind {
                "song" => match (attributes[who].as_str(), attributes["albumName"].as_str()) {
                    (Some(artist), Some(album)) => Some(format!("{artist} · {album}")),
                    (Some(artist), None) => Some(artist.to_owned()),
                    _ => None,
                },
                _ => attributes[who].as_str().map(str::to_owned),
            };
            hits.push(Hit {
                kind,
                id: id.to_owned(),
                title: title.to_owned(),
                subtitle,
                art: attributes["artwork"]["url"]
                    .as_str()
                    .map(|url| url.replace("{w}", "240").replace("{h}", "240")),
            });
        }
    }
    Ok(hits)
}

/// What something of a person's Apple Music is: what Sonos can play and
/// the identifier it knows it by (a library item is known by its catalogue
/// twin when it has one).
fn library_hit(item: &Json) -> Option<Hit> {
    let attributes = &item["attributes"];
    let title = attributes["name"].as_str()?;
    let kind = match item["type"].as_str()? {
        t if t.contains("album") => "album",
        t if t.contains("playlist") => "playlist",
        t if t.contains("station") => "station",
        t if t.contains("song") => "song",
        t if t.contains("artist") => "artist",
        _ => return None,
    };
    if kind == "station" && attributes["isLive"].as_bool() != Some(true) {
        return None;
    }
    let own = item["id"].as_str()?;
    let id = item["relationships"]["catalog"]["data"][0]["id"]
        .as_str()
        .or_else(|| attributes["playParams"]["globalId"].as_str())
        // A catalogue item is its own; a playlist of the library is played by its
        // library identifier; any other library item has no address Sonos knows.
        .or_else(|| (!own.starts_with("l.") && !own.starts_with("i.")).then_some(own))?;
    let subtitle = attributes["artistName"]
        .as_str()
        .or_else(|| attributes["curatorName"].as_str())
        .map(str::to_owned);
    Some(Hit {
        kind,
        id: id.to_owned(),
        title: title.to_owned(),
        subtitle,
        art: attributes["artwork"]["url"]
            .as_str()
            .map(|url| url.replace("{w}", "240").replace("{h}", "240")),
    })
}

/// The things of a library, recent or recommendation answer (the
/// recommendations hold their things one level down).
pub(crate) fn parse_library(body: &[u8]) -> anyhow::Result<Vec<Hit>> {
    let json: Json = serde_json::from_slice(body).context("Apple's answer is not JSON")?;
    let mut hits: Vec<Hit> = Vec::new();
    let mut take = |item: &Json| {
        if let Some(hit) = library_hit(item)
            && !hits.iter().any(|h| h.kind == hit.kind && h.id == hit.id)
        {
            hits.push(hit);
        }
    };
    for item in json["data"].as_array().into_iter().flatten() {
        take(item);
        for inner in item["relationships"]["contents"]["data"]
            .as_array()
            .into_iter()
            .flatten()
        {
            take(inner);
        }
    }
    // A page is not a catalogue: the first ones are enough.
    hits.truncate(60);
    Ok(hits)
}

/// The songs of a `top-songs` answer: identifier and name.
pub(crate) fn parse_songs(body: &[u8]) -> anyhow::Result<Vec<(String, String)>> {
    let json: Json = serde_json::from_slice(body).context("Apple's answer is not JSON")?;
    Ok(json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|song| {
            Some((
                song["id"].as_str()?.to_owned(),
                song["attributes"]["name"].as_str()?.to_owned(),
            ))
        })
        .collect())
}

/// An identifier fit to be written in an address and in XML.
fn clean(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// What a Sonos needs to play a result of the catalogue, for the Apple Music
/// account (`account`: the `sn` the house's speakers use) it is linked to.
pub(crate) fn playable(
    kind: &str,
    id: &str,
    title: &str,
    account: &str,
) -> anyhow::Result<Playable> {
    if !clean(id) || !account.bytes().all(|b| b.is_ascii_digit()) {
        bail!("{}", moli_i18n::tr!("pilotes.sonos.lecture_inconnue"));
    }
    let (uri, item, parent, class) = match kind {
        "song" => (
            format!("x-sonos-http:song%3a{id}.mp4?sid={SERVICE}&flags=8232&sn={account}"),
            format!("10032020song%3a{id}"),
            "-1".to_owned(),
            "object.item.audioItem.musicTrack",
        ),
        "album" => (
            format!(
                "x-rincon-cpcontainer:1004206calbum%3a{id}?sid={SERVICE}&flags=8300&sn={account}"
            ),
            format!("1004206calbum%3a{id}"),
            "10052064".to_owned(),
            "object.container.album.musicAlbum",
        ),
        "playlist" => (
            format!(
                "x-rincon-cpcontainer:1006206cplaylist%3a{id}?sid={SERVICE}&flags=8300&sn={account}"
            ),
            format!("1006206cplaylist%3a{id}"),
            "10052064".to_owned(),
            "object.container.playlistContainer",
        ),
        // A station (`ra.…`): a stream, told as Sonos tells its own.
        "station" => (
            format!("x-sonosapi-hls:hls%3a{id}?sid={SERVICE}&flags=8232&sn={account}"),
            format!("10092028hls%3a{id}"),
            format!("00020000radio:{}", escape(&title.to_lowercase())),
            "object.item.audioItem.audioBroadcast",
        ),
        _ => bail!("{}", moli_i18n::tr!("pilotes.sonos.lecture_inconnue")),
    };
    // What a speaker is told it plays: a list needs its description and the
    // service's descriptor to start; a track shows its title in the queue at
    // once (without it, the queue shows nothing until the track loads).
    let meta = format!(
        r#"<DIDL-Lite xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:r="urn:schemas-rinconnetworks-com:metadata-1-0/" xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"><item id="{item}" parentID="{parent}" restricted="true"><dc:title>{}</dc:title><upnp:class>{class}</upnp:class><desc id="cdudn" nameSpace="urn:schemas-rinconnetworks-com:metadata-1-0/">{DESCRIPTOR}</desc></item></DIDL-Lite>"#,
        escape(title)
    );
    Ok(Playable {
        uri,
        meta,
        queue: kind != "station",
    })
}

/// The Apple Music account the house's speakers use (`sn`), from any
/// Apple Music address they already know (a favorite).
pub(crate) fn account_of<'a>(uris: impl IntoIterator<Item = &'a str>) -> Option<String> {
    uris.into_iter()
        .filter(|uri| uri.contains(&format!("sid={SERVICE}")))
        .find_map(|uri| {
            let digits: String = uri
                .split("sn=")
                .nth(1)?
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then_some(digits)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_catalogue_found() {
        let body = br#"{"results":{
          "songs":{"data":[{"id":"111","attributes":{"name":"Dai Dai","artistName":"Artist A","albumName":"Single A","artwork":{"url":"https://example.com/{w}x{h}bb.jpg"}}}]},
          "albums":{"data":[{"id":"222","attributes":{"name":"Single A","artistName":"Artist A"}}]},
          "playlists":{"data":[{"id":"pl.abc","attributes":{"name":"Mix","curatorName":"Apple Music"}},{"id":"pl.noname","attributes":{}}]}}}"#;
        let hits = parse_hits(body).unwrap();
        assert_eq!(hits.len(), 3, "a result with no name is left out");
        assert_eq!(hits[0].kind, "song");
        assert_eq!(hits[0].subtitle.as_deref(), Some("Artist A · Single A"));
        assert_eq!(
            hits[0].art.as_deref(),
            Some("https://example.com/240x240bb.jpg")
        );
        assert_eq!(hits[2].id, "pl.abc");
        assert!(parse_hits(b"{}").unwrap().is_empty());
        assert!(parse_hits(b"nope").is_err());
    }

    #[test]
    fn reads_artists_stations_and_top_songs() {
        let body = br#"{"results":{
          "artists":{"data":[{"id":"42","attributes":{"name":"Artist A","artwork":{"url":"https://example.com/{w}x{h}.jpg"}}}]},
          "stations":{"data":[{"id":"ra.9","attributes":{"name":"Live Radio","isLive":true}},{"id":"ra.10","attributes":{"name":"Station pop","isLive":false}},{"id":"ra.11","attributes":{"name":"No word"}}]}}}"#;
        let hits = parse_hits(body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].kind, hits[0].id.as_str()), ("artist", "42"));
        assert_eq!((hits[1].kind, hits[1].id.as_str()), ("station", "ra.9"));
        let top = br#"{"data":[{"id":"1","attributes":{"name":"One"}},{"id":"2","attributes":{"name":"Two"}},{"attributes":{}}]}"#;
        assert_eq!(
            parse_songs(top).unwrap(),
            [("1".into(), "One".into()), ("2".into(), "Two".into())]
        );
        assert!(parse_songs(b"{}").unwrap().is_empty());
    }

    #[test]
    fn reads_a_persons_library_and_recommendations() {
        let body = br#"{"data":[
          {"id":"p.abc","type":"library-playlists","attributes":{"name":"Mes favoris","playParams":{"id":"p.abc","kind":"playlist","isLibrary":true}}},
          {"id":"l.xyz","type":"library-albums","attributes":{"name":"Album perso","artistName":"A"},"relationships":{"catalog":{"data":[{"id":"1234","type":"albums"}]}}},
          {"id":"l.upload","type":"library-albums","attributes":{"name":"Fichier importe"}},
          {"id":"rec","type":"personal-recommendation","attributes":{"name":"ignored"},"relationships":{"contents":{"data":[
            {"id":"pl.u-1","type":"playlists","attributes":{"name":"Pour toi","curatorName":"Apple Music"}},
            {"id":"ra.2","type":"stations","attributes":{"name":"Station","isLive":true}},
            {"id":"ra.3","type":"stations","attributes":{"name":"Station pop","isLive":false}},
            {"id":"pl.u-1","type":"playlists","attributes":{"name":"Pour toi"}}]}}}]}"#;
        let hits = parse_library(body).unwrap();
        let seen: Vec<(&str, &str)> = hits.iter().map(|h| (h.kind, h.id.as_str())).collect();
        assert_eq!(
            seen,
            [
                ("playlist", "p.abc"),
                ("album", "1234"),
                ("playlist", "pl.u-1"),
                ("station", "ra.2")
            ],
            "a library album is its catalogue twin, an upload has none, doubles are one"
        );
        assert_eq!(hits[1].subtitle.as_deref(), Some("A"));
    }

    #[test]
    fn a_result_becomes_something_a_speaker_plays() {
        let song = playable("song", "111", "x", "13").unwrap();
        assert_eq!(
            song.uri,
            "x-sonos-http:song%3a111.mp4?sid=204&flags=8232&sn=13"
        );
        assert!(
            song.meta.contains("<dc:title>x</dc:title>"),
            "a track is named too"
        );
        let album = playable("album", "222", "Single & Co", "13").unwrap();
        assert!(
            album
                .uri
                .starts_with("x-rincon-cpcontainer:1004206calbum%3a222?sid=204")
        );
        assert!(album.meta.contains("Single &amp; Co"), "titles are escaped");
        assert!(album.meta.contains(DESCRIPTOR));
        let playlist = playable("playlist", "pl.abc", "Mix", "13").unwrap();
        assert!(playlist.uri.contains("1006206cplaylist%3apl.abc"));
        let station = playable("station", "ra.1437317694", "Rire & Chansons", "13").unwrap();
        assert_eq!(
            station.uri,
            "x-sonosapi-hls:hls%3ara.1437317694?sid=204&flags=8232&sn=13"
        );
        assert!(station.meta.contains("00020000radio:rire &amp; chansons"));
        assert!(!station.queue && song.queue && album.queue);
        // Nothing foreign gets into an address or XML.
        assert!(playable("song", "1/../2", "x", "13").is_err());
        assert!(playable("song", "1", "x", "13&x").is_err());
        assert!(
            playable("artist", "1", "x", "13").is_err(),
            "an artist is songs, not one address"
        );
    }

    #[test]
    fn finds_the_account_in_known_addresses() {
        let known = [
            "x-sonosapi-hls:hls%3ara.1?sid=254&flags=8232&sn=7",
            "x-rincon-cpcontainer:1004206calbum%3a1?sid=204&flags=8300&sn=13",
        ];
        assert_eq!(account_of(known).as_deref(), Some("13"));
        assert_eq!(account_of(["x-sonos-http:a?sid=1&sn=2"]), None);
    }

    #[test]
    fn a_search_term_is_encoded() {
        assert_eq!(percent("Kendji Girac & co"), "Kendji%20Girac%20%26%20co");
        assert_eq!(percent("é"), "%C3%A9");
    }
}

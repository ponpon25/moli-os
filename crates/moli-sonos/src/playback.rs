//! What Sonos says about what to play, read and written as it comes: play
//! modes (shuffle, repeat) and the items of its lists (favorites, saved
//! playlists, the queue).

use moli_net::upnp::{tag, unescape};
use serde::Serialize;

/// The values of the `repeat` point.
pub(crate) const REPEATS: [&str; 3] = ["off", "all", "one"];

/// The play mode Sonos knows for a shuffle and a repeat.
pub(crate) fn play_mode(shuffle: bool, repeat: &str) -> &'static str {
    match (shuffle, repeat) {
        (false, "all") => "REPEAT_ALL",
        (false, "one") => "REPEAT_ONE",
        (true, "all") => "SHUFFLE",
        (true, "one") => "SHUFFLE_REPEAT_ONE",
        (true, _) => "SHUFFLE_NOREPEAT",
        (false, _) => "NORMAL",
    }
}

/// Shuffle and repeat, from a play mode.
pub(crate) fn mode_parts(mode: &str) -> (bool, &'static str) {
    match mode {
        "REPEAT_ALL" => (false, "all"),
        "REPEAT_ONE" => (false, "one"),
        "SHUFFLE" => (true, "all"),
        "SHUFFLE_REPEAT_ONE" => (true, "one"),
        "SHUFFLE_NOREPEAT" => (true, "off"),
        _ => (false, "off"),
    }
}

/// One entry of a Sonos list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub title: String,
    pub artist: Option<String>,
    /// Where it plays from (empty: a shortcut Sonos shows but cannot play).
    pub uri: String,
    /// What the player wants to know about it (the favorite's own
    /// description of what it points to).
    pub meta: String,
}

/// The items of a `Browse` answer (`<item>` and `<container>`).
pub(crate) fn items(response: &str) -> Vec<Item> {
    let Some(result) = tag(response, "Result") else {
        return Vec::new();
    };
    let didl = unescape(result);
    ["<item ", "<container "]
        .iter()
        .flat_map(|open| didl.split(open).skip(1))
        .filter_map(item)
        .collect()
}

fn item(chunk: &str) -> Option<Item> {
    Some(Item {
        title: unescape(tag(chunk, "title")?).trim().to_owned(),
        artist: tag(chunk, "creator")
            .map(|a| unescape(a).trim().to_owned())
            .filter(|a| !a.is_empty()),
        uri: res(chunk).map(unescape).unwrap_or_default(),
        meta: tag(chunk, "resMD").map(unescape).unwrap_or_default(),
    })
}

/// The text of `<res …>…</res>`.
fn res(chunk: &str) -> Option<&str> {
    let at = chunk.find("<res")? + "<res".len();
    let rest = &chunk[at..];
    if !rest.starts_with(['>', ' ']) {
        return None;
    }
    let start = rest.find('>')? + 1;
    let end = rest[start..].find("</res>")?;
    Some(&rest[start..start + end])
}

/// The favorite called `name` (case and surrounding spaces ignored).
pub(crate) fn find<'a>(items: &'a [Item], name: &str) -> Option<&'a Item> {
    let name = name.trim().to_lowercase();
    items.iter().find(|i| i.title.to_lowercase() == name)
}

/// How a favorite starts: a list (playlist, album…) goes through the queue,
/// anything else (a station, a track) plays as it is.
pub(crate) fn through_queue(uri: &str) -> bool {
    uri.starts_with("x-rincon-cpcontainer:") || uri.starts_with("file:///jffs/settings/savedqueues")
}

#[derive(Serialize)]
struct Line<'a> {
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<&'a str>,
}

/// The queue, for a point: a list of titles, as JSON.
pub(crate) fn queue_json(items: &[Item]) -> String {
    let lines: Vec<Line> = items
        .iter()
        .map(|i| Line {
            title: &i.title,
            artist: i.artist.as_deref(),
        })
        .collect();
    serde_json::to_string(&lines).unwrap_or_else(|_| "[]".into())
}

/// The favorites, for a point: their names, as JSON.
pub(crate) fn names_json(items: &[Item]) -> String {
    let names: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    serde_json::to_string(&names).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Browse` answer, as Sonos sends it: the list is escaped once, and
    /// what a favorite points to (`resMD`) twice.
    fn answer() -> String {
        let meta = moli_net::upnp::escape(
            r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"><item id="x" parentID="y" restricted="true"><dc:title>Album Un &amp; Deux</dc:title></item></DIDL-Lite>"#,
        );
        let didl = format!(
            r#"<DIDL-Lite><item id="FV:2/1" parentID="FV:2"><dc:title>Station Radio</dc:title><res></res><r:resMD>{meta}</r:resMD></item><item id="FV:2/2" parentID="FV:2"><dc:title>Album Un &amp; Deux</dc:title><res protocolInfo="x-rincon-cpcontainer:*:*:*">x-rincon-cpcontainer:1004206calbum%3a1?sid=204&amp;flags=8300&amp;sn=13</res><dc:creator>Artiste</dc:creator><r:resMD>{meta}</r:resMD></item><container id="SQ:3" parentID="SQ:"><dc:title>Mix</dc:title><res protocolInfo="file:*:audio/mpegurl:*">file:///jffs/settings/savedqueues.rsq#3</res></container></DIDL-Lite>"#
        );
        format!(
            "<s:Envelope><s:Body><u:BrowseResponse><Result>{}</Result></u:BrowseResponse></s:Body></s:Envelope>",
            moli_net::upnp::escape(&didl)
        )
    }

    #[test]
    fn reads_favorites_and_playlists() {
        let list = items(&answer());
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].uri, "", "a shortcut cannot play");
        assert_eq!(list[1].title, "Album Un & Deux");
        assert_eq!(list[1].artist.as_deref(), Some("Artiste"));
        assert_eq!(
            list[1].uri,
            "x-rincon-cpcontainer:1004206calbum%3a1?sid=204&flags=8300&sn=13"
        );
        assert!(
            list[1].meta.starts_with("<DIDL-Lite "),
            "the description is whole XML again: {}",
            list[1].meta
        );
        assert!(list[1].meta.contains(r#"restricted="true""#));
        assert_eq!(list[2].uri, "file:///jffs/settings/savedqueues.rsq#3");
        assert!(items("<nothing/>").is_empty());
    }

    #[test]
    fn finds_a_favorite_by_name() {
        let list = items(&answer());
        assert!(find(&list, "  album un & deux ").is_some());
        assert!(find(&list, "nothing").is_none());
        assert!(through_queue(&list[1].uri));
        assert!(through_queue(&list[2].uri));
        assert!(!through_queue("x-sonosapi-hls:hls%3ara.1?sid=204"));
    }

    #[test]
    fn modes_go_both_ways() {
        for shuffle in [false, true] {
            for repeat in REPEATS {
                let mode = play_mode(shuffle, repeat);
                assert_eq!(mode_parts(mode), (shuffle, repeat), "{mode}");
            }
        }
        assert_eq!(play_mode(false, "off"), "NORMAL");
        assert_eq!(mode_parts("something else"), (false, "off"));
    }

    #[test]
    fn lists_are_json() {
        let list = items(&answer());
        assert_eq!(
            names_json(&list),
            r#"["Station Radio","Album Un & Deux","Mix"]"#
        );
        assert!(queue_json(&list).contains(r#"{"title":"Album Un & Deux","artist":"Artiste"}"#));
        assert_eq!(queue_json(&[]), "[]");
    }
}

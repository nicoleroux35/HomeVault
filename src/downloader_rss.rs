/* =============================================================================
 * File          : downloader_rss.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   RSS/Atom subscriptions for the Downloader (Download Station's "RSS"
 *   half): a list of feeds the appliance watches, a parser that turns a feed
 *   into download candidates, and a per-feed "already seen" marker so a
 *   refresh queues each item once.
 *
 *   No new daemon and no new scheduler: the periodic refresh is a
 *   `service = downloader_rss` scheduled task, which reuses the generic
 *   engine (P1-31). The parser is pure and unit-tested; only `fetch` touches
 *   the network, through a bounded `reqwest` client.
 *
 *   NZB and eMule are deliberately absent (comparison document §3.3): niche,
 *   credentials to store, and a legal exposure a home NAS should not take on.
 *   Only a link the feed publishes is queued, and it still goes through the
 *   downloader's own scheme allowlist before aria2 sees it.
 *
 * Dependencies:
 *   - serde (>= 1.0), xml (>= 1), reqwest (>= 0.12),
 *     crate::network, crate::config::paths
 *
 * Usage:
 *   handlers::downloader exposes /api/downloader/feeds; see
 *   docs/guides/downloader.md.
 * =============================================================================
 */

//! RSS/Atom subscriptions for the Downloader (see docs/guides/downloader.md).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Deadline on one feed fetch. A feed is a few kilobytes; a server that has
/// not answered in twenty seconds is not going to.
pub const FEED_TIMEOUT: Duration = Duration::from_secs(20);

/// Largest feed body accepted. A feed that claims more is refused rather than
/// loaded into memory: this is a subscription, not a file download.
pub const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;

/// Longest accepted feed URL.
pub const MAX_URL_LEN: usize = 2048;

/// Longest accepted feed name.
pub const MAX_NAME_LEN: usize = 80;

/// Items kept per feed in the "already seen" marker, so the file cannot grow
/// without bound on a busy feed.
pub const MAX_SEEN: usize = 500;

/// Items returned by one parse.
pub const MAX_ITEMS: usize = 200;

/// One download candidate found in a feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssItem {
    /// Human title (falls back to the link when the feed has none).
    pub title: String,
    /// The page the item points at (also the fallback download URL).
    pub link: String,
    /// What the Downloader should fetch: the enclosure when the feed has one,
    /// else the item link. Only http(s)/magnet ever lands here.
    pub download_url: String,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// Stable identity used to queue an item exactly once (enclosure URL,
    /// else the feed's guid/id, else the link).
    pub guid: String,
}

impl RssItem {
    /// The label shown in the UI: the title, else the last path segment.
    pub fn label(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        self.download_url
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(self.download_url.as_str())
            .to_string()
    }
}

/// One subscribed feed, as persisted in `data/downloader_feeds.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RssFeed {
    /// Stable id (uuid), never derived from the URL.
    pub id: String,
    /// Display name; defaults to the feed URL's host the first time.
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Queue new items automatically on refresh (true) or only list them.
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub last_checked_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Identities already queued, newest last. Bounded to [`MAX_SEEN`].
    #[serde(default)]
    pub seen: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// The persisted store (atomic rewrite, like every other JSON settings file).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RssStore {
    #[serde(default)]
    pub feeds: Vec<RssFeed>,
}

/// `data/downloader_feeds.json`.
pub fn config_path() -> std::path::PathBuf {
    crate::config::paths::data_dir().join("downloader_feeds.json")
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// Validate a feed URL.
///
/// http(s) only, no embedded credentials (they would end up in a log line the
/// moment the fetch fails), and no literal loopback/RFC 1918 address — a feed
/// URL is an admin choice about the *internet*, and pointing it at the
/// appliance's own services would turn it into an SSRF primitive.
pub fn validate_feed_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("A feed address is required".to_string());
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!(
            "The feed address is too long (maximum {MAX_URL_LEN})"
        ));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The feed address must not contain spaces".to_string());
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "The feed address must start with http:// or https://".to_string())?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("A feed address must use http:// or https://".to_string());
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err("The feed address must include a host".to_string());
    }
    if authority.contains('@') {
        return Err(
            "Remove the user name and password from the feed address; use a token in the path instead"
                .to_string(),
        );
    }
    let host = host_of(authority);
    if host.is_empty() {
        return Err("The feed address must include a host".to_string());
    }
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("A feed address cannot point at this machine".to_string());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if crate::network::is_private_address(ip) {
            return Err("A feed address cannot point at a private or loopback address".to_string());
        }
    }
    Ok(url.to_string())
}

/// `host` of an authority, without the port and without IPv6 brackets.
fn host_of(authority: &str) -> String {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_string();
    }
    authority.split(':').next().unwrap_or("").to_string()
}

/// A display name for a new feed: its host, else the whole URL.
pub fn default_feed_name(url: &str) -> String {
    let host = host_of(
        url.split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(url)
            .split('/')
            .next()
            .unwrap_or(url),
    );
    let name = if host.is_empty() { url } else { &host };
    name.chars().take(MAX_NAME_LEN).collect()
}

/// Validate an operator-supplied feed name.
pub fn validate_feed_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("A name is required".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!("The name is longer than {MAX_NAME_LEN} characters"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("The name must not contain control characters".to_string());
    }
    Ok(name.to_string())
}

/// Whether a link may be queued: http(s) or a magnet with an `xt=` parameter.
///
/// The full check (length, whitespace, scheme allowlist) stays in the
/// downloader's own `validate_uri`, which every queued item still goes
/// through — a feed is untrusted input.
pub fn is_downloadable(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        return lower.contains("xt=");
    }
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && lower.len() > "https://".len()
}

// ---------------------------------------------------------------------------
// Feed parsing (pure)
// ---------------------------------------------------------------------------

/// The item being read, in the flat shape RSS and Atom share.
#[derive(Default)]
struct PendingItem {
    title: String,
    link: String,
    guid: String,
    published: String,
    enclosure: String,
    enclosure_size: Option<u64>,
    atom_link: String,
    /// Inside `<item>` (RSS) or `<entry>` (Atom)?
    active: bool,
    /// The text element currently open (`title`, `link`, …).
    field: Option<&'static str>,
}

/// Turn a parsed `<item>`/`<entry>` into a candidate, or `None` when it holds
/// nothing the Downloader can fetch (a news-only feed yields none).
fn finish_item(pending: PendingItem) -> Option<RssItem> {
    let download_url = if !pending.enclosure.trim().is_empty() {
        pending.enclosure.trim().to_string()
    } else if is_downloadable(&pending.link) {
        pending.link.trim().to_string()
    } else if is_downloadable(&pending.atom_link) {
        pending.atom_link.trim().to_string()
    } else {
        return None;
    };
    if !is_downloadable(&download_url) {
        return None;
    }
    let guid = if !pending.guid.trim().is_empty() {
        pending.guid.trim().to_string()
    } else {
        download_url.clone()
    };
    let link = if !pending.link.trim().is_empty() {
        pending.link.trim().to_string()
    } else {
        pending.atom_link.trim().to_string()
    };
    Some(RssItem {
        title: pending.title.trim().to_string(),
        link,
        download_url,
        published: Some(pending.published.trim().to_string()).filter(|p| !p.is_empty()),
        size: pending.enclosure_size,
        guid,
    })
}

/// Parse an RSS 2.0 or Atom document into download candidates.
///
/// Pure: the caller supplies the text, so the refusal matrix is unit-tested
/// without a network. Items keep the feed's order; the cap is [`MAX_ITEMS`].
pub fn parse_feed(xml_text: &str) -> Result<Vec<RssItem>, String> {
    use xml::reader::{EventReader, XmlEvent};

    if xml_text.trim().is_empty() {
        return Err("The feed is empty".to_string());
    }
    let config = xml::reader::ParserConfig::new()
        .cdata_to_characters(true)
        .whitespace_to_characters(true)
        .ignore_comments(true)
        .coalesce_characters(true);
    let reader = EventReader::new_with_config(std::io::Cursor::new(xml_text.as_bytes()), config);

    let mut pending = PendingItem::default();
    let mut items: Vec<RssItem> = Vec::new();

    for event in reader {
        let event = event.map_err(|e| format!("The feed is not valid XML: {e}"))?;
        match event {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                let local = name.local_name.to_ascii_lowercase();
                if local == "item" || local == "entry" {
                    pending = PendingItem {
                        active: true,
                        ..PendingItem::default()
                    };
                    continue;
                }
                if !pending.active {
                    continue;
                }
                match local.as_str() {
                    "title" => pending.field = Some("title"),
                    "link" => {
                        // Atom carries the URL in `href`; RSS puts it in text.
                        let href = attributes
                            .iter()
                            .find(|a| a.name.local_name.eq_ignore_ascii_case("href"))
                            .map(|a| a.value.clone());
                        let rel = attributes
                            .iter()
                            .find(|a| a.name.local_name.eq_ignore_ascii_case("rel"))
                            .map(|a| a.value.to_ascii_lowercase());
                        match href {
                            Some(href) if rel.as_deref() == Some("enclosure") => {
                                pending.enclosure = href;
                                // Atom carries the size on the same element
                                // (`<link rel="enclosure" href="…"
                                // length="2048"/>`), exactly like RSS.
                                if let Some(length) = attributes
                                    .iter()
                                    .find(|a| a.name.local_name.eq_ignore_ascii_case("length"))
                                    .and_then(|a| a.value.trim().parse::<u64>().ok())
                                {
                                    pending.enclosure_size = Some(length);
                                }
                            }
                            Some(href) => {
                                if pending.atom_link.is_empty() {
                                    pending.atom_link = href;
                                }
                            }
                            None => pending.field = Some("link"),
                        }
                    }
                    "guid" | "id" => pending.field = Some("guid"),
                    "pubdate" | "published" | "updated" | "date" => {
                        pending.field = Some("published")
                    }
                    "enclosure" => {
                        for attribute in &attributes {
                            match attribute.name.local_name.to_ascii_lowercase().as_str() {
                                "url" => pending.enclosure = attribute.value.clone(),
                                "length" => {
                                    pending.enclosure_size =
                                        attribute.value.trim().parse::<u64>().ok()
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            XmlEvent::EndElement { name } => {
                if !pending.active {
                    continue;
                }
                let local = name.local_name.to_ascii_lowercase();
                if local == "item" || local == "entry" {
                    pending.active = false;
                    if let Some(item) = finish_item(std::mem::take(&mut pending)) {
                        if items.len() < MAX_ITEMS {
                            items.push(item);
                        }
                    }
                    pending = PendingItem::default();
                    continue;
                }
                pending.field = None;
            }
            XmlEvent::Characters(text) => {
                if !pending.active {
                    continue;
                }
                match pending.field {
                    Some("title") => pending.title.push_str(&text),
                    Some("link") => pending.link.push_str(text.trim()),
                    Some("guid") => pending.guid.push_str(text.trim()),
                    Some("published") => pending.published.push_str(text.trim()),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Ok(items)
}

// ---------------------------------------------------------------------------
// Persistence and fetching (blocking / network)
// ---------------------------------------------------------------------------

/// Load the persisted feeds. A corrupt file is warned about and treated as
/// empty rather than taking the Downloader down with it.
pub fn load() -> Vec<RssFeed> {
    let raw = match std::fs::read_to_string(config_path()) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<RssStore>(&raw) {
        Ok(store) => store.feeds,
        Err(e) => {
            tracing::warn!(
                "downloader feeds: {} is not valid JSON: {e}",
                config_path().display()
            );
            Vec::new()
        }
    }
}

/// Persist the feeds atomically with mode 0600 (a feed URL may carry a private
/// token in its path).
pub fn save(feeds: &[RssFeed]) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_vec_pretty(&RssStore {
        feeds: feeds.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.write_all(&body)
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        file.sync_all().ok();
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Fetch and parse one feed. Network + parse, bounded by [`FEED_TIMEOUT`].
pub async fn fetch(url: &str) -> Result<Vec<RssItem>, String> {
    let url = validate_feed_url(url)?;
    let client = reqwest::Client::builder()
        .timeout(FEED_TIMEOUT)
        // A redirect is followed only a few times: a feed that bounces forever
        // would otherwise hold the worker for the whole timeout.
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("HomeVault downloader")
        .build()
        .map_err(|e| format!("Could not create the HTTP client: {e}"))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("The feed did not answer: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The feed answered HTTP {}", response.status()));
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_FEED_BYTES {
            return Err(format!(
                "The feed is larger than {} KB",
                MAX_FEED_BYTES / 1024
            ));
        }
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("The feed could not be read: {e}"))?;
    if body.len() > MAX_FEED_BYTES {
        return Err(format!(
            "The feed is larger than {} KB",
            MAX_FEED_BYTES / 1024
        ));
    }
    parse_feed(&body)
}

/// The items of `feed` that have never been queued.
pub fn unseen<'a>(feed: &RssFeed, items: &'a [RssItem]) -> Vec<&'a RssItem> {
    items
        .iter()
        .filter(|item| !feed.seen.iter().any(|seen| seen == &item.guid))
        .collect()
}

/// Record `items` as seen on `feed`, keeping the marker bounded.
pub fn mark_seen(feed: &mut RssFeed, items: &[RssItem]) {
    for item in items {
        if feed.seen.iter().any(|seen| seen == &item.guid) {
            continue;
        }
        feed.seen.push(item.guid.clone());
    }
    if feed.seen.len() > MAX_SEEN {
        let excess = feed.seen.len() - MAX_SEEN;
        feed.seen.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Shows</title>
  <item>
    <title>Episode 12</title>
    <link>https://example.com/ep12</link>
    <guid>ep-12</guid>
    <pubDate>Mon, 09 Nov 2026 10:00:00 GMT</pubDate>
    <enclosure url="https://cdn.example.com/ep12.mp4" length="1048576" type="video/mp4"/>
  </item>
  <item>
    <title>No enclosure</title>
    <link>https://example.com/page</link>
  </item>
  <item>
    <title>Nothing to fetch</title>
  </item>
  <item>
    <title>Wrong scheme</title>
    <enclosure url="file:///etc/shadow"/>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Feed</title>
  <entry>
    <title>Atom item</title>
    <id>urn:uuid:1234</id>
    <updated>2026-11-09T10:00:00Z</updated>
    <link rel="alternate" href="https://example.com/post"/>
    <link rel="enclosure" href="https://example.com/post.mp3" length="2048"/>
  </entry>
</feed>"#;

    fn feed() -> RssFeed {
        RssFeed {
            id: "f1".to_string(),
            name: "Shows".to_string(),
            url: "https://example.com/feed.xml".to_string(),
            enabled: true,
            auto_download: false,
            last_checked_at: None,
            last_error: None,
            seen: Vec::new(),
        }
    }

    #[test]
    fn test_feed_url_shape_and_ssrf_guard() {
        assert_eq!(
            validate_feed_url("https://example.com/feed.xml").unwrap(),
            "https://example.com/feed.xml"
        );
        for bad in [
            "http://192.168.1.10/feed",
            "http://localhost:8080/feed",
            "http://127.0.0.1/feed",
            "http://[::1]/feed",
            "ftp://example.com/feed",
            "https://user:pw@example.com/feed",
            "https://example.com/a b",
            "",
        ] {
            assert!(validate_feed_url(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn test_parse_rss_keeps_only_downloadable_items() {
        let items = parse_feed(RSS).unwrap();
        assert_eq!(items.len(), 2, "the news-only and file:// items drop out");
        assert_eq!(items[0].title, "Episode 12");
        assert_eq!(items[0].download_url, "https://cdn.example.com/ep12.mp4");
        assert_eq!(items[0].guid, "ep-12");
        assert_eq!(items[0].size, Some(1_048_576));
        assert_eq!(
            items[0].published.as_deref(),
            Some("Mon, 09 Nov 2026 10:00:00 GMT")
        );
        // No enclosure: the item link is the download URL, and the link is
        // also the identity when the feed has no guid.
        assert_eq!(items[1].download_url, "https://example.com/page");
        assert_eq!(items[1].guid, "https://example.com/page");
    }

    #[test]
    fn test_parse_atom_uses_the_enclosure_link() {
        let items = parse_feed(ATOM).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Atom item");
        assert_eq!(items[0].download_url, "https://example.com/post.mp3");
        assert_eq!(items[0].guid, "urn:uuid:1234");
        assert_eq!(items[0].size, Some(2048));
    }

    #[test]
    fn test_parse_rejects_malformed_and_empty_documents() {
        assert!(parse_feed("").is_err());
        assert!(parse_feed("<rss><channel><item>").is_err());
        // Valid XML but not a feed: no items, not an error.
        assert!(parse_feed("<html><body>hi</body></html>")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_magnet_is_downloadable() {
        assert!(is_downloadable("magnet:?xt=urn:btih:abcdef"));
        assert!(!is_downloadable("magnet:?dn=nothing"));
        assert!(is_downloadable("HTTPS://example.com/f.iso"));
        assert!(!is_downloadable("file:///etc/passwd"));
        assert!(!is_downloadable("https://"));
    }

    #[test]
    fn test_seen_marker_queues_each_item_once_and_stays_bounded() {
        let mut feed = feed();
        let items = parse_feed(RSS).unwrap();
        assert_eq!(unseen(&feed, &items).len(), 2);
        mark_seen(&mut feed, &items);
        assert!(unseen(&feed, &items).is_empty());
        // Re-parsing the same feed (a refresh) queues nothing new.
        mark_seen(&mut feed, &parse_feed(RSS).unwrap());
        assert_eq!(feed.seen.len(), 2);

        // A long history cannot grow without bound.
        for n in 0..(MAX_SEEN + 25) {
            let item = RssItem {
                title: String::new(),
                link: String::new(),
                download_url: "https://example.com/x".to_string(),
                published: None,
                size: None,
                guid: format!("g{n}"),
            };
            mark_seen(&mut feed, &[item]);
        }
        assert_eq!(feed.seen.len(), MAX_SEEN);
        assert_eq!(feed.seen.last().map(String::as_str), Some("g524"));
    }

    #[test]
    fn test_default_name_and_label() {
        assert_eq!(
            default_feed_name("https://feeds.example.com/rss").as_str(),
            "feeds.example.com"
        );
        assert_eq!(
            default_feed_name("https://example.com:8443/f").as_str(),
            "example.com"
        );
        let item = RssItem {
            title: "  ".to_string(),
            link: String::new(),
            download_url: "https://example.com/dir/ep12.mp4".to_string(),
            published: None,
            size: None,
            guid: "g".to_string(),
        };
        assert_eq!(item.label(), "ep12.mp4");
    }

    #[test]
    fn test_store_round_trip_and_forward_compatibility() {
        let store = RssStore {
            feeds: vec![feed()],
        };
        let json = serde_json::to_string(&store).unwrap();
        let back: RssStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.feeds, store.feeds);

        // A file from a newer HomeVault still loads, with safe defaults.
        let future =
            r#"{"feeds":[{"id":"x","name":"n","url":"https://e.com/f","future_key":true}]}"#;
        let parsed: RssStore = serde_json::from_str(future).unwrap();
        assert_eq!(parsed.feeds.len(), 1);
        assert!(parsed.feeds[0].enabled);
        assert!(!parsed.feeds[0].auto_download);
    }
}

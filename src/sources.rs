use serde::Deserialize;
use std::sync::{LazyLock, OnceLock};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::settings::SourcesConfig;
use crate::log_sources;

static CFG: OnceLock<SourcesConfig> = OnceLock::new();

pub fn init(cfg: SourcesConfig) {
    let _ = CFG.set(cfg);
}

fn cfg() -> &'static SourcesConfig {
    CFG.get_or_init(SourcesConfig::default)
}

const YTDLP: &str = "yt-dlp";
const JS_RUNTIME_ARGS: &[&str] = &["--js-runtimes", "node"];
const FORMAT_ARGS: &[&str] = &["-f", "ba[abr>0][vcodec=none]/best"];
const TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct ResolvedMeta {
    pub webpage_url: String,
    pub title: String,
    pub author: String,
    pub duration_secs: Option<u64>,
    pub thumbnail: String,
    pub is_live: bool,
    pub is_spotify_match: bool,
    pub ui_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YtDlpEntry {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    webpage_url: Option<String>,
    url: Option<String>,
    #[serde(default)]
    is_live: Option<bool>,
}

async fn run_ytdlp(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(YTDLP);
    cmd.args(JS_RUNTIME_ARGS).args(args);
    let out = tokio::time::timeout(std::time::Duration::from_secs(TIMEOUT_SECS), cmd.output())
        .await
        .map_err(|_| format!("{YTDLP} timed out after {TIMEOUT_SECS}s"))?
        .map_err(|e| format!("failed to spawn {YTDLP}: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "yt-dlp failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn parse_entry(v: &serde_json::Value, spotify: bool, ui_link: Option<String>) -> Option<ResolvedMeta> {
    if v.get("_type").and_then(|t| t.as_str()) == Some("playlist") {
        return None;
    }
    let e: YtDlpEntry = serde_json::from_value(v.clone()).ok()?;
    let webpage_url = e
        .webpage_url
        .or(e.url)
        .filter(|u| u.starts_with("http") || u.contains("search:"))?;
    Some(ResolvedMeta {
        webpage_url,
        title: e.title.unwrap_or_else(|| "Unknown".into()),
        author: e
            .channel
            .or(e.uploader)
            .unwrap_or_else(|| "Unknown".into()),
        duration_secs: e.duration.map(|d| d as u64).filter(|d| *d > 0),
        thumbnail: e.thumbnail.unwrap_or_default(),
        is_live: e.is_live.unwrap_or(false),
        is_spotify_match: spotify,
        ui_link,
    })
}

pub fn is_url(q: &str) -> bool {
    q.starts_with("http://") || q.starts_with("https://")
}

fn youtube_search(query: &str) -> String {
    format!("ytsearch1:{query}")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SearchPlatform {
    YouTube,
    SoundCloud,
    Spotify,
    AppleMusic,
}

/// Parse an optional leading platform prefix: ytsearch[:N], scsearch[:N],
/// spsearch[:N], amsearch[:N]. Returns (platform, stripped query).
pub fn parse_prefix(q: &str) -> Option<(SearchPlatform, String)> {
    let lower = q.to_ascii_lowercase();
    for (tag, plat) in [
        ("ytsearch", SearchPlatform::YouTube),
        ("scsearch", SearchPlatform::SoundCloud),
        ("spsearch", SearchPlatform::Spotify),
        ("amsearch", SearchPlatform::AppleMusic),
    ] {
        if let Some(rest) = lower.strip_prefix(tag) {
            let rest = rest.trim_start_matches(':');
            // ignore a numeric count — we always take the best match
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            return Some((plat, rest.to_string()));
        }
    }
    None
}

pub async fn resolve(raw_query: &str) -> Result<ResolvedMeta, String> {
    if let Some(sp) = SpotifyRef::parse(raw_query) {
        return sp.resolve_single().await;
    }

    let (explicit_platform, stripped) = match parse_prefix(raw_query) {
        Some((p, q)) => (Some(p), q),
        None => (None, raw_query.to_string()),
    };

    if is_url(&stripped) {
        return extract_via_ytdlp(&stripped).await;
    }

    // Explicit prefix -> direct to that platform (bypasses the fallback chain)
    if let Some(platform) = explicit_platform {
        return match platform {
            SearchPlatform::YouTube => {
                if !cfg().youtube_enabled {
                    Err("YouTube source is disabled in config.toml".into())
                } else {
                    search_youtube(&stripped).await
                }
            }
            SearchPlatform::SoundCloud => {
                if !cfg().soundcloud_enabled {
                    Err("SoundCloud source is disabled in config.toml".into())
                } else {
                    search_soundcloud(&stripped).await
                }
            }
            SearchPlatform::Spotify => {
                if !cfg().spotify_enabled {
                    Err("Spotify source is disabled in config.toml".into())
                } else {
                    spotify_search_match(&stripped).await
                }
            }
            SearchPlatform::AppleMusic => {
                if !cfg().apple_music_enabled {
                    Err("Apple Music source is disabled in config.toml".into())
                } else {
                    apple_music_search_match(&stripped).await
                }
            }
        };
    }

    // Plain text: fixed priority order ported from the old bot's lavalinkSearch()
    // ytmsearch -> ytsearch -> spsearch -> scsearch
    resolve_by_name(&stripped).await
}

async fn resolve_by_name(query: &str) -> Result<ResolvedMeta, String> {
    log_sources!(
        "Search",
        "resolving `{}` (ytmusic -> youtube -> spotify -> soundcloud)",
        query
    );

    if cfg().youtube_enabled {
        match search_ytmusic(query).await {
            Ok(m) => return Ok(m),
            Err(e) => log_sources!(
                "YouTubeMusic",
                "no result ({e}); falling back to YouTube"
            ),
        }
        match search_youtube(query).await {
            Ok(m) => return Ok(m),
            Err(e) => log_sources!("YouTube", "no result ({e}); falling back to Spotify"),
        }
    } else {
        log_sources!("Search", "youtube disabled in config; skipping ytmusic/ytsearch");
    }

    if cfg().spotify_enabled {
        match spotify_search_match(query).await {
            Ok(m) => return Ok(m),
            Err(e) => log_sources!(
                "Spotify",
                "no result ({e}); falling back to SoundCloud"
            ),
        }
    } else {
        log_sources!("Search", "spotify disabled in config; skipping spsearch");
    }

    if cfg().soundcloud_enabled {
        match search_soundcloud(query).await {
            Ok(m) => return Ok(m),
            Err(e) => log_sources!("SoundCloud", "no result ({e})"),
        }
    } else {
        log_sources!("Search", "soundcloud disabled in config; skipping scsearch");
    }

    Err(format!(
        "No results for `{query}` on any platform (ytmusic, youtube, spotify, soundcloud)"
    ))
}

async fn extract_via_ytdlp(query: &str) -> Result<ResolvedMeta, String> {
    let stdout = run_ytdlp(&[
        "-j",
        "--no-playlist",
        query,
        FORMAT_ARGS[0],
        FORMAT_ARGS[1],
    ])
    .await?;
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(m) = parse_entry(&v, false, None) {
                return Ok(m);
            }
    }
    Err(format!("No results for `{query}`"))
}

pub async fn search_youtube(query: &str) -> Result<ResolvedMeta, String> {
    extract_via_ytdlp(&youtube_search(query)).await
}

// ── YouTube Music search (InnerTube WEB_REMIX, like Nodelink's ytmsearch) ────

pub async fn search_ytmusic(query: &str) -> Result<ResolvedMeta, String> {
    let http = reqwest::Client::new();
    let body = serde_json::json!({
        "context": {"client": {
            "clientName": "WEB_REMIX",
            "clientVersion": "1.20240401.01.00",
            "hl": "en",
            "gl": "US"
        }},
        "query": query,
        // ytmusicapi "songs" filter (get_search_params): official catalog songs
        // only — without it YTM returns its mixed shelf and covers rank first.
        "params": "EgWKAQIIAWoMEA4QChADEAQQCRAF"
    });
    let resp = http
        .post("https://music.youtube.com/youtubei/v1/search?prettyPrint=false")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("ytmusic request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ytmusic search failed (HTTP {})", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let mut items: Vec<&serde_json::Value> = Vec::new();
    walk_items(&v, &mut items);

    let chosen = items
        .iter()
        .find(|it| {
            it.get("playlistItemData")
                .and_then(|p| p.get("videoId"))
                .is_some()
                || it
                    .pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
                    .is_some()
        })
        .ok_or_else(|| format!("No results on YouTube Music for `{query}`"))?;

    let video_id = chosen
        .get("playlistItemData")
        .and_then(|p| p.get("videoId"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            chosen.pointer(
                "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
            ).and_then(|v| v.as_str())
        })
        .unwrap_or_default()
        .to_string();

    let col_text = |idx: usize| -> String {
        chosen
            .get("flexColumns")
            .and_then(|c| c.get(idx))
            .map(|col| {
                let mut out = String::new();
                if let Some(runs) = col.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
                    && let Some(arr) = runs.as_array() {
                        for r in arr {
                            if let Some(t) = r.get("text").and_then(|t| t.as_str()) {
                                out.push_str(t);
                            }
                        }
                    }
                out
            })
            .unwrap_or_default()
    };

    let raw_title = col_text(0);
    // Songs-filter layout: col0 = title, col1 = "Artist • Album • 3:22",
    // col2+ = play counts. Join everything after the title so both this and
    // the unfiltered layout ("Song • Artist • 3:22" all in one column) parse.
    let meta_text = {
        let mut cols: Vec<String> = Vec::new();
        if let Some(arr) = chosen.get("flexColumns").and_then(|c| c.as_array()) {
            for col in arr.iter().skip(1) {
                let mut out = String::new();
                if let Some(runs) =
                    col.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")
                    && let Some(a) = runs.as_array()
                {
                    for r in a {
                        if let Some(t) = r.get("text").and_then(|t| t.as_str()) {
                            out.push_str(t);
                        }
                    }
                }
                if !out.is_empty() {
                    cols.push(out);
                }
            }
        }
        cols.join(" • ")
    };
    let (author, duration_secs) = parse_ytm_meta(&meta_text);

    let thumbnail = chosen
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|a| a.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    let m = ResolvedMeta {
        webpage_url: format!("https://www.youtube.com/watch?v={video_id}"),
        title: clean_ytm_title(&raw_title),
        author: if author.is_empty() { "Unknown".into() } else { author },
        duration_secs,
        thumbnail,
        is_live: false,
        is_spotify_match: false,
        ui_link: None,
    };
    log_sources!(
        "YouTubeMusic",
        "`{}` -> {} - {}",
        query,
        m.author,
        m.title
    );
    Ok(m)
}

fn walk_items<'a>(o: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    if let Some(obj) = o.as_object() {
        if let Some(it) = obj.get("musicResponsiveListItemRenderer") {
            out.push(it);
        }
        for v in obj.values() {
            walk_items(v, out);
        }
    } else if let Some(arr) = o.as_array() {
        for v in arr {
            walk_items(v, out);
        }
    }
}

fn parse_mmss(s: &str) -> Option<u64> {
    let s = s.trim();
    // require at least one ':' so bare numbers (view counts etc.) never match
    if !s.contains(':') {
        return None;
    }
    let mut total = 0u64;
    for part in s.split(':') {
        let v: u64 = part.trim().parse().ok()?;
        total = total.checked_mul(60)?.checked_add(v)?;
    }
    Some(total)
}

/// YTM's second flex column looks like "Song • Artist • 3:33" where the first
/// token is a category label, not the artist. Returns (artist, duration).
fn parse_ytm_meta(meta_text: &str) -> (String, Option<u64>) {
    const CATEGORY_TOKENS: &[&str] = &["song", "video", "single", "ep", "album", "playlist", "live"];

    let segments: Vec<&str> = meta_text
        .split('•')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let author = segments
        .iter()
        .find(|s| !CATEGORY_TOKENS.contains(&s.to_ascii_lowercase().as_str()))
        .copied()
        .unwrap_or_default();

    let duration_secs = segments.iter().rev().find_map(|s| parse_mmss(s));
    (author.to_string(), duration_secs)
}

fn clean_ytm_title(raw: &str) -> String {
    crate::state::clean_title(if raw.is_empty() { "Unknown" } else { raw })
}

pub async fn resolve_playlist(url: &str) -> Result<Vec<ResolvedMeta>, String> {
    if let Some(sp) = SpotifyRef::parse(url) {
        return sp.resolve_collection().await;
    }

    let stdout = run_ytdlp(&["-j", "--flat-playlist", url]).await?;
    let mut out = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("_type").and_then(|t| t.as_str()) == Some("playlist") {
            if let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
                for e in entries {
                    if let Some(m) = parse_entry(e, false, None) {
                        out.push(m);
                    }
                }
            }
            continue;
        }
        if let Some(m) = parse_entry(&v, false, None) {
            out.push(m);
        }
    }
    if out.is_empty() {
        return Err("Playlist empty or failed to load".into());
    }
    Ok(out)
}

pub async fn probe_version() -> Result<String, String> {
    run_ytdlp(&["--version"]).await
}

pub fn is_playlist_query(q: &str) -> bool {
    q.contains("playlist") || q.contains("album") || q.contains("list=") || q.contains("/sets/")
}

// ── Spotify (metadata only → matched on YouTube) ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpotifyKind {
    Track,
    Album,
    Playlist,
}

pub struct SpotifyRef {
    pub kind: SpotifyKind,
    pub id: String,
}

impl SpotifyRef {
    pub fn parse(url: &str) -> Option<Self> {
        if !url.contains("open.spotify.com") && !url.starts_with("spotify:") {
            return None;
        }
        let parts: Vec<&str> = url.split(&['/', ':'][..]).filter(|p| !p.is_empty()).collect();
        let mut kind = None;
        let mut id = None;
        let mut iter = parts.iter().peekable();
        while let Some(p) = iter.next() {
            match *p {
                "track" | "album" | "playlist" => {
                    kind = Some(match *p {
                        "track" => SpotifyKind::Track,
                        "album" => SpotifyKind::Album,
                        _ => SpotifyKind::Playlist,
                    });
                    id = iter.next().copied();
                }
                _ => {}
            }
        }
        Some(Self {
            kind: kind?,
            id: id?.split('?').next()?.to_string(),
        })
    }
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    expires_in: u64,
}

static SPOTIFY_TOKEN: LazyLock<Mutex<Option<(String, std::time::Instant, u64)>>> =
    LazyLock::new(|| Mutex::new(None));

async fn spotify_token(http: &reqwest::Client) -> Result<String, String> {
    {
        let guard = SPOTIFY_TOKEN.lock().await;
        if let Some((tok, fetched, ttl)) = guard.as_ref()
            && fetched.elapsed().as_secs() < ttl.saturating_sub(60) {
                return Ok(tok.clone());
            }
    }

    let id = std::env::var("SPOTIFY_CLIENT_ID").map_err(|_| {
        "Spotify links need SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET in .env (search by song name works without them)".to_string()
    })?;
    let secret = std::env::var("SPOTIFY_CLIENT_SECRET").map_err(|_| SPOTIFY_ENV_MSG.to_string())?;

    let resp = http
        .post("https://accounts.spotify.com/api/token")
        .form(&[("grant_type", "client_credentials")])
        .header("Authorization", format!("Basic {}", base64_encode(&format!("{id}:{secret}"))))
        .send()
        .await
        .map_err(|e| format!("spotify token request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("spotify auth rejected (HTTP {})", resp.status()));
    }
    let t: TokenResp = resp.json().await.map_err(|e| e.to_string())?;

    let mut guard = SPOTIFY_TOKEN.lock().await;
    *guard = Some((t.access_token.clone(), std::time::Instant::now(), t.expires_in));
    Ok(t.access_token)
}

const SPOTIFY_ENV_MSG: &str = "Spotify links need SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET in .env (search by song name works without them)";

// minimal base64 to avoid pulling a new crate
fn base64_encode(input: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[derive(Deserialize)]
struct SpTrack {
    name: String,
    #[serde(default)]
    artists: Vec<SpArtist>,
    #[serde(default, deserialize_with = "ms_opt")]
    duration_ms: Option<u64>,
    #[serde(default)]
    album: Option<SpAlbum>,
}

#[derive(Deserialize)]
struct SpArtist {
    name: String,
}

#[derive(Deserialize)]
struct SpAlbum {
    #[serde(default)]
    images: Vec<SpImage>,
}

#[derive(Deserialize)]
struct SpImage {
    url: String,
}

fn ms_opt<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<u64> = serde::Deserialize::deserialize(deserializer)?;
    Ok(v.map(|ms| ms / 1000))
}

fn sp_meta(t: &SpTrack, ui: String) -> ResolvedMeta {
    let artist = t.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    ResolvedMeta {
        webpage_url: youtube_search(&format!("{} {}", artist, t.name)),
        title: t.name.clone(),
        author: artist,
        duration_secs: t.duration_ms,
        thumbnail: t.album.as_ref().and_then(|a| a.images.first()).map(|i| i.url.clone()).unwrap_or_default(),
        is_live: false,
        is_spotify_match: true,
        ui_link: Some(ui),
    }
}

impl SpotifyRef {
    async fn resolve_single(self) -> Result<ResolvedMeta, String> {
        if self.kind != SpotifyKind::Track {
            return self.resolve_collection().await.and_then(|v| {
                v.into_iter().next().ok_or("Spotify collection was empty".to_string())
            });
        }
        let http = reqwest::Client::new();
        let token = spotify_token(&http).await?;
        let resp = http
            .get(format!("https://api.spotify.com/v1/tracks/{}", self.id))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("spotify track lookup failed (HTTP {})", resp.status()));
        }
        let t: SpTrack = resp.json().await.map_err(|e| e.to_string())?;
        Ok(sp_meta(&t, format!("https://open.spotify.com/track/{}", self.id)))
    }

    async fn resolve_collection(self) -> Result<Vec<ResolvedMeta>, String> {
        match self.kind {
            SpotifyKind::Album => {
                let http = reqwest::Client::new();
                let token = spotify_token(&http).await?;
                self.resolve_album(&http, &token).await
            }
            _ => self.resolve_playlist_embed().await,
        }
    }

    async fn resolve_album(
        self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<Vec<ResolvedMeta>, String> {
        let label = format!("https://open.spotify.com/album/{}", self.id);
        let url_base =
            format!("https://api.spotify.com/v1/albums/{}/tracks?limit=50", self.id);

        let mut metas = Vec::new();
        let mut next = Some(url_base);
        while let Some(u) = next {
            let resp = http.get(&u).bearer_auth(token).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("spotify album lookup failed (HTTP {})", resp.status()));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

            if let Some(arr) = body.get("items").and_then(|i| i.as_array()).cloned() {
                for it in arr {
                    if let Ok(t) = serde_json::from_value::<SpTrack>(it)
                        && !t.name.is_empty()
                    {
                        metas.push(sp_meta(&t, label.clone()));
                    }
                }
            }
            next = body.get("next").and_then(|n| n.as_str()).map(str::to_string);
        }

        if metas.is_empty() {
            return Err("Album empty or failed to load".into());
        }
        Ok(metas)
    }

    /// Playlists are 403-gated on the Web API for client-credential apps, so we
    /// scrape the public embed page instead (title/subtitle/duration per track).
    async fn resolve_playlist_embed(&self) -> Result<Vec<ResolvedMeta>, String> {
        use crate::log_sources;

        let ui = format!("https://open.spotify.com/playlist/{}", self.id);
        let url = format!("https://open.spotify.com/embed/playlist/{}", self.id);
        let http = reqwest::Client::new();
        let html = http
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .send()
            .await
            .map_err(|e| format!("spotify embed fetch failed: {e}"))?
            .text()
            .await
            .map_err(|e| e.to_string())?;

        let json_str = html
            .split("<script id=\"__NEXT_DATA__\" type=\"application/json\">")
            .nth(1)
            .and_then(|rest| rest.split("</script>").next())
            .ok_or("spotify embed page missing data (layout changed?)")?;

        let data: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("bad embed json: {e}"))?;
        let entity = data.pointer("/props/pageProps/state/data/entity");
        let Some(entity) = entity else {
            return Err("spotify embed data missing entity".into());
        };

        let mut metas = Vec::new();
        if let Some(list) = entity.get("trackList").and_then(|t| t.as_array()) {
            for t in list {
                let name = t.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let artist = t.get("subtitle").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let duration_ms = t.get("duration").and_then(|v| v.as_u64());
                metas.push(ResolvedMeta {
                    webpage_url: youtube_search(&format!("{artist} {name}")),
                    title: name.to_string(),
                    author: if artist.is_empty() {
                        "Unknown".into()
                    } else {
                        artist.to_string()
                    },
                    duration_secs: duration_ms.map(|ms| ms / 1000).filter(|d| *d > 0),
                    thumbnail: entity
                        .pointer("/coverArt/sources/0/url")
                        .and_then(|u| u.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    is_live: false,
                    is_spotify_match: true,
                    ui_link: Some(ui.clone()),
                });
            }
        }

        if metas.is_empty() {
            return Err("Playlist empty or failed to load".into());
        }
        log_sources!(
            "Spotify",
            "playlist `{}` -> {} tracks (embed)",
            entity.get("title").and_then(|t| t.as_str()).unwrap_or("?"),
            metas.len()
        );
        Ok(metas)
    }
}

// ── SoundCloud search (api-v2, client_id scraped like Nodelink) ─────────────

const SC_BASE: &str = "https://api-v2.soundcloud.com";
static SC_CLIENT_ID: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

async fn sc_client_id(http: &reqwest::Client) -> Result<String, String> {
    {
        let guard = SC_CLIENT_ID.lock().await;
        if let Some(id) = guard.as_ref() {
            return Ok(id.clone());
        }
    }

    if let Ok(env_id) = std::env::var("SOUNDCLOUD_CLIENT_ID")
        && env_id.len() == 32 {
            *SC_CLIENT_ID.lock().await = Some(env_id.clone());
            return Ok(env_id);
        }

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";
    let html = http
        .get("https://soundcloud.com")
        .header("User-Agent", UA)
        .send()
        .await
        .map_err(|e| format!("soundcloud homepage: {e}"))?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let asset_re =
        regex::Regex::new(r"https://a-v2\.sndcdn\.com/assets/[a-zA-Z0-9-]+\.js").unwrap();
    let id_re = regex::Regex::new(
        r#"(?:[?&/]?(?:client_id)[\s:=&]*"?|"data":\{"id":")([A-Za-z0-9]{32})"?"#,
    )
    .unwrap();

    for asset in asset_re.find_iter(&html).take(12) {
        let url = asset.as_str();
        let resp = http.get(url).header("User-Agent", UA).send().await;
        let body = match resp {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(_) => continue,
        };
        if let Some(m) = id_re.captures(&body) {
            let id = m.get(1).unwrap().as_str().to_string();
            log_sources!(
                "SoundCloud",
                "loaded client_id {} from {}",
                id,
                &url[..url.len().min(60)]
            );
            *SC_CLIENT_ID.lock().await = Some(id.clone());
            return Ok(id);
        }
    }
    Err("could not scrape a SoundCloud client_id — set SOUNDCLOUD_CLIENT_ID in .env as fallback".into())
}

#[derive(Deserialize)]
struct ScCollection {
    #[serde(default)]
    collection: Vec<ScItem>,
}

#[derive(Deserialize)]
struct ScItem {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    permalink_url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    artwork_url: Option<String>,
    #[serde(default)]
    user: Option<ScUser>,
}

#[derive(Deserialize)]
struct ScUser {
    username: String,
}

pub async fn search_soundcloud(query: &str) -> Result<ResolvedMeta, String> {
    let http = reqwest::Client::new();
    let client_id = sc_client_id(&http).await?;
    let url = format!(
        "{SC_BASE}/search?q={}&client_id={client_id}&limit=5&offset=0&linked_partitioning=1&facet=model",
        urlencoding::encode(query)
    );
    let resp = http
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send()
        .await
        .map_err(|e| format!("soundcloud search failed: {e}"))?;
    if !resp.status().is_success() {
        // stale client_id? drop cache so next attempt re-scrapes
        *SC_CLIENT_ID.lock().await = None;
        return Err(format!("soundcloud search failed (HTTP {})", resp.status()));
    }
    let body: ScCollection = resp.json().await.map_err(|e| e.to_string())?;

    let item = body
        .collection
        .iter()
        .find(|i| i.kind == "track" && i.permalink_url.is_some())
        .ok_or_else(|| format!("No results on SoundCloud for `{query}`"))?;

    log_sources!(
        "SoundCloud",
        "matched `{}` -> {}",
        query,
        item.permalink_url.as_deref().unwrap_or("")
    );
    Ok(ResolvedMeta {
        webpage_url: item.permalink_url.clone().unwrap_or_default(),
        title: item.title.clone().unwrap_or_else(|| "Unknown".into()),
        author: item.user.as_ref().map(|u| u.username.clone()).unwrap_or_default(),
        duration_secs: item.duration.map(|ms| ms / 1000),
        thumbnail: item.artwork_url.clone().unwrap_or_default(),
        is_live: false,
        is_spotify_match: false,
        ui_link: None,
    })
}

// ── spsearch / amsearch → metadata then matched on YouTube ──────────────────

pub async fn spotify_search_match(query: &str) -> Result<ResolvedMeta, String> {
    let http = reqwest::Client::new();
    let token = spotify_token(&http).await?;
    let resp = http
        .get(format!(
            "https://api.spotify.com/v1/search?type=track&limit=1&q={}",
            urlencoding::encode(query)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("spotify search failed (HTTP {})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let track = body
        .pointer("/tracks/items/0")
        .cloned()
        .ok_or_else(|| format!("No results on Spotify for `{query}`"))?;
    let t: SpTrack = serde_json::from_value(track).map_err(|e| e.to_string())?;
    let meta = sp_meta(
        &t,
        format!("https://open.spotify.com/search/{}", urlencoding::encode(query)),
    );
    log_sources!("Spotify", "`{}` -> {} - {}", query, meta.author, meta.title);
    Ok(meta)
}

#[derive(Deserialize)]
struct ItunesResp {
    #[serde(default)]
    results: Vec<ItunesTrack>,
}

#[derive(Deserialize)]
struct ItunesTrack {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    artwork_url_100: Option<String>,
    #[serde(default, rename = "trackTimeMillis")]
    track_time_millis: Option<u64>,
}

pub async fn apple_music_search_match(query: &str) -> Result<ResolvedMeta, String> {
    let http = reqwest::Client::new();
    let resp = http
        .get(format!(
            "https://itunes.apple.com/search?media=music&entity=song&limit=1&term={}",
            urlencoding::encode(query)
        ))
        .send()
        .await
        .map_err(|e| format!("itunes search failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("itunes search failed (HTTP {})", resp.status()));
    }
    let body: ItunesResp = resp.json().await.map_err(|e| e.to_string())?;
    let t = body
        .results
        .into_iter()
        .next()
        .ok_or_else(|| format!("No Apple Music results for `{query}`"))?;

    let yt_query = format!("{} {}", t.artist_name, t.track_name);
    log_sources!("AppleMusic", "`{}` -> {}", query, yt_query);

    let mut m = search_youtube(&yt_query).await?;
    m.is_spotify_match = false;
    m.ui_link = Some(format!(
        "https://music.apple.com/search?term={}",
        urlencoding::encode(query)
    ));
    if let Some(art) = t.artwork_url_100
        && !art.is_empty() {
            m.thumbnail = art;
        }
    m.duration_secs = t.track_time_millis.map(|ms| ms / 1000);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ytm_meta_skips_category_token() {
        let (author, dur) = parse_ytm_meta("Song • Rick Astley • 3:33");
        assert_eq!(author, "Rick Astley");
        assert_eq!(dur, Some(3 * 60 + 33));
    }

    #[test]
    fn ytm_meta_handles_hours_and_missing_parts() {
        let (author, dur) = parse_ytm_meta("Video • Some Channel • 1:02:03");
        assert_eq!(author, "Some Channel");
        assert_eq!(dur, Some(3723));

        let (author, dur) = parse_ytm_meta("Song • Just A Name");
        assert_eq!(author, "Just A Name");
        assert_eq!(dur, None);
    }

    #[test]
    fn mmss_rejects_bare_numbers() {
        assert_eq!(parse_mmss("3:22"), Some(202));
        assert_eq!(parse_mmss("1:02:03"), Some(3723));
        assert_eq!(parse_mmss("1.2M views"), None);
        assert_eq!(parse_mmss("42"), None);
    }

    #[tokio::test]
    #[ignore = "live network test"]
    async fn ytmusic_search_live() {
        let m = search_ytmusic("blinding lights").await.unwrap();
        assert!(m.webpage_url.contains("youtube.com/watch?v="));
        println!(
            "ytmusic -> title={} | author={} | dur={:?} | thumb={} | url={}",
            m.title, m.author, m.duration_secs, !m.thumbnail.is_empty(), m.webpage_url
        );
    }

    #[tokio::test]
    #[ignore = "live network test"]
    async fn fallback_chain_live() {
        for q in ["finding her kushagra", "blinding lights", "die with a smile"] {
            match resolve(q).await {
                Ok(m) => println!(
                    "CHAIN {q}\n  title={}\n  author={}\n  dur={:?}\n  live={}\n  thumb={}\n  url={}",
                    m.title, m.author, m.duration_secs, m.is_live, !m.thumbnail.is_empty(), m.webpage_url
                ),
                Err(e) => println!("CHAIN {q} FAILED: {e}"),
            }
        }
    }
}


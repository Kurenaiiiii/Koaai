use std::collections::VecDeque;
use std::fmt;

use dashmap::DashMap;
use regex::RegexBuilder;
use serenity::model::id::{ChannelId, GuildId, MessageId};
use songbird::input::AuxMetadata;
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    Off,
    Track,
    Queue,
}

impl LoopMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Track,
            Self::Track => Self::Queue,
            Self::Queue => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Track => "Single Track",
            Self::Queue => "Entire Queue",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Track => "Track",
            Self::Queue => "Queue",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceTag {
    Youtube,
    SoundCloud,
    SpotifyMatched,
    File,
    Discord,
}

impl fmt::Display for SourceTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Youtube => "Youtube",
            Self::SoundCloud => "Soundcloud",
            Self::SpotifyMatched => "Spotify",
            Self::File => "File",
            Self::Discord => "Discord",
        };
        f.write_str(s)
    }
}

fn clean_patterns() -> &'static Vec<regex::Regex> {
    static RE: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        const PATTERNS: &[&str] = &[
            r"\(re-?upload[^)]*\)",
            r"\[re-?upload[^\]]*\]",
            r"\(official[^)]*\)",
            r"\[official[^\]]*\]",
            r"\(lyrics?[^)]*\)",
            r"\[lyrics?[^\]]*\]",
            r"\(audio\)",
            r"\[audio\]",
            r"\(slowed[^)]*\)",
            r"\[slowed[^\]]*\]",
        ];
        PATTERNS
            .iter()
            .map(|p| {
                RegexBuilder::new(p)
                    .case_insensitive(true)
                    .build()
                    .expect("static regex must compile")
            })
            .collect()
    })
}

pub fn clean_title(title: &str) -> String {
    let mut res = title.to_string();
    for p in clean_patterns() {
        res = p.replace_all(&res, "").to_string();
    }
    let collapsed = res.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed
        .trim_start_matches([' ', '|', '‑', '-'])
        .trim_end_matches([' ', '|', '‑', '-']);
    if trimmed.is_empty() {
        title.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn filename_from_url(uri: &str) -> Option<String> {
    if uri.is_empty() {
        return None;
    }
    let path = Url::parse(uri).ok()?.path().to_string();
    let raw = path.rsplit('/').next()?;
    let no_ext = raw.rsplit_once('.').map(|(b, _)| b).unwrap_or(raw);
    let pretty = no_ext.replace(['_', '-'], " ").trim().to_string();
    if pretty.len() >= 20 && pretty.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if pretty.is_empty() { None } else { Some(pretty) }
}

// ── Autoplay helpers ─────────────────────────────────────────────────────

/// How many started tracks are remembered for repeat-avoidance per guild.
pub const AUTOPLAY_HISTORY_CAP: usize = 100;
/// Autoplay never picks livestreams, unknown-length entries, or anything
/// longer than this (mixes love 1-hour compilations).
pub const AUTOPLAY_MAX_SECS: u64 = 20 * 60;
/// How many mix entries to consider per trigger. Mixes hold ~50 unique
/// videos; fetching the whole thing maximizes the survivor pool so the
/// random roll actually has room to vary (tiny pools = same chain).
pub const AUTOPLAY_MIX_LIMIT: u32 = 50;

/// Extracts the 11-char YouTube videoId from a watch URL. None for anything
/// else (lazy `ytsearch1:` queries, SoundCloud/file URLs, ...).
pub fn extract_video_id(uri: &str) -> Option<String> {
    let url = Url::parse(uri).ok()?;
    let host = url.host_str().unwrap_or_default();
    if host.ends_with("youtube.com") {
        let id = url
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())?;
        return (id.len() == 11).then_some(id);
    }
    if host == "youtu.be" {
        let id = url.path().trim_matches('/').to_string();
        return (id.len() == 11).then_some(id);
    }
    None
}

/// Normalizes an author name for the never-same-artist-twice-in-a-row rule.
/// Catches the big one: YouTube's auto-generated `"Coldplay - Topic"`
/// channels vs plain `"Coldplay"`. Best-effort, not a musicologist.
pub fn normalize_author(author: &str) -> String {
    let collapsed = author
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .strip_suffix("- topic")
        .map(str::trim)
        .unwrap_or(&collapsed)
        .to_string()
}

/// Reduces a title to its matchable stub so re-uploads compare equal:
/// `"Khat - Navjot Ahuja"` and `"KHAT (Lyrics)"` both become `"khat"`.
/// Cuts at the first separator that isn't at position 0 — a title that
/// STARTS with a paren (e.g. `"(Untitled)"`) keeps it.
pub fn normalize_title_stub(title: &str) -> String {
    const SEPS: &[&str] = &[
        " - ",
        " | ",
        " (",
        " [",
        " ft ",
        " ft.",
        " feat ",
        " feat.",
        " featuring ",
    ];
    let lower = title.to_lowercase();
    let mut end = lower.len();
    for sep in SEPS {
        if let Some(i) = lower.find(sep)
            && i > 0
            && i < end
        {
            end = i;
        }
    }
    lower[..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One remembered play: exact key plus fuzzy match surfaces. Radio supply is
/// infinite, so false positives (skipping a legit same-named song) cost
/// nothing while false negatives (audible repeats) cost everything — err
/// aggressive.
#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub key: String,
    pub title_stub: String,
    pub author_norm: String,
}

/// Title markers of uploads radio must never pick. Two species:
/// non-original audio (covers, mashups, slowed edits, ...) and video-first
/// uploads whose audio isn't the song (skits, story animations, teasers —
/// useless without the picture). Deliberately NOT bare "story" or "video":
/// "Story of My Life" and half of all official uploads would die.
const NON_SONG_MARKERS: &[&str] = &[
    "cover", "mashup", "mash up", "mash-up", "unplugged", "acoustic", "remix",
    "slowed", "sped up", "speed up", "spedup", "reverb", "nightcore",
    "8d audio", "lofi", "lo-fi", "karaoke", "instrumental", "ringtone",
    "tiktok", "teaser", "trailer", "short film", "episode", "interview",
    "behind the scenes", "making of", "vlog", "reaction", "animated",
    "animation", "love story",
];

pub fn is_non_song(title: &str) -> bool {
    let lower = title.to_lowercase();
    NON_SONG_MARKERS.iter().any(|m| lower.contains(m))
}

/// Verifies a YouTube-Music audio upgrade before swapping it in: same song
/// (stub match), not the seed, no author back-to-back, never played/queued.
/// Pure logic — unit-tested.
#[allow(clippy::too_many_arguments)]
pub fn audio_upgrade_ok(
    ytm_video_id: &str,
    ytm_title: &str,
    ytm_author: &str,
    cand_stub: &str,
    seed_video_id: &str,
    seed_author_norm: &str,
    history: &VecDeque<HistoryEntry>,
    queued: &[HistoryEntry],
) -> bool {
    if ytm_video_id.is_empty() || ytm_video_id == seed_video_id {
        return false;
    }
    if cand_stub.is_empty() || normalize_title_stub(ytm_title) != cand_stub {
        return false; // YTM returned a different song
    }
    if !seed_author_norm.is_empty() && normalize_author(ytm_author) == seed_author_norm {
        return false;
    }
    if history.iter().any(|h| h.key == ytm_video_id)
        || queued.iter().any(|h| h.key == ytm_video_id)
    {
        return false;
    }
    true
}

/// Tiny non-crypto RNG without adding a `rand` dependency. Radio shuffling
/// only — uniformity past "feels random across 5 songs" doesn't matter.
/// A process-wide counter is folded in so rapid successive calls can never
/// share an input (kills any same-nanosecond doubt for good).
pub fn rand_below(bound: usize) -> usize {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    if bound <= 1 {
        return 0;
    }
    let mut h = RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let salt = CTR
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9E3779B97F4A7C15);
    h.write_u64(nanos ^ salt);
    std::hint::black_box(&mut h);
    (h.finish() % bound as u64) as usize
}

impl HistoryEntry {
    pub fn new(uri: &str, title: &str, author: &str) -> Self {
        Self {
            key: track_key(uri),
            title_stub: normalize_title_stub(title),
            author_norm: normalize_author(author),
        }
    }
}

/// Dedup key for a track: the videoId when it's YouTube, else the full URI.
pub fn track_key(uri: &str) -> String {
    extract_video_id(uri).unwrap_or_else(|| uri.to_string())
}

/// A single YouTube-Mix entry, already resolved to playable metadata.
#[derive(Clone, Debug)]
pub struct MixCandidate {
    pub video_id: String,
    pub webpage_url: String,
    pub title: String,
    pub author: String,
    pub duration_secs: Option<u64>,
    pub thumbnail: String,
    pub is_live: bool,
}

/// Collects mix entry indices that survive every autoplay filter, in mix
/// order. The caller randomizes among the first few — a deterministic
/// first-match would play the SAME chain every time a seed repeats.
/// Pure logic — fully unit-tested.
pub fn autoplay_pick(
    candidates: &[MixCandidate],
    seed_video_id: &str,
    seed_author_norm: &str,
    history: &VecDeque<HistoryEntry>,
    queued: &[HistoryEntry],
) -> Vec<usize> {
    // Cheap exact-match sets first.
    let hist_keys: Vec<&str> = history.iter().map(|h| h.key.as_str()).collect();
    let queued_keys: Vec<&str> = queued.iter().map(|h| h.key.as_str()).collect();
    candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            if c.video_id.is_empty() || c.video_id == seed_video_id {
                return None; // the seed itself (mixes list it first)
            }
            if c.title.trim().is_empty() {
                return None;
            }
            if is_non_song(&c.title) {
                return None; // covers, video-versions, slowed edits — songs only
            }
            if c.is_live {
                return None;
            }
            match c.duration_secs {
                Some(d) if d > 0 && d <= AUTOPLAY_MAX_SECS => {}
                _ => return None, // unknown length or a 2-hour compilation
            }
            if !seed_author_norm.is_empty()
                && normalize_author(&c.author) == seed_author_norm
            {
                return None; // same artist as the last song
            }
            let key = track_key(&c.webpage_url);
            if hist_keys.contains(&key.as_str()) || queued_keys.contains(&key.as_str()) {
                return None; // exact replay / already queued
            }
            // Re-upload twin: different videoId, same song ("khat" vs
            // "Khat - Navjot Ahuja"). Empty stubs never match.
            let stub = normalize_title_stub(&c.title);
            if !stub.is_empty()
                && (history.iter().any(|h| h.title_stub == stub)
                    || queued.iter().any(|h| h.title_stub == stub))
            {
                return None;
            }
            Some(i)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct Track {
    pub uri: String,
    pub duration_secs: Option<u64>,
    pub requester: String,
    pub thumbnail: String,
    pub source: SourceTag,
    pub title: String,
    pub author: String,
    pub is_live: bool,
    pub ui_link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedInfo {
    pub webpage_url: String,
    pub title: String,
    pub author: String,
    pub duration_secs: Option<u64>,
    pub thumbnail: String,
    pub is_live: bool,
    pub is_spotify_match: bool,
    pub ui_link: Option<String>,
}

impl Track {
    pub fn from_resolved(info: ResolvedInfo, requester: impl Into<String>) -> Self {
        let source = if info.is_spotify_match {
            SourceTag::SpotifyMatched
        } else {
            classify_source(&info.webpage_url)
        };
        let raw_title = info.title;
        let (title, author) = match source {
            SourceTag::Discord | SourceTag::File => (
                filename_from_url(&info.webpage_url)
                    .filter(|_| raw_title.is_empty())
                    .unwrap_or_else(|| clean_title(&raw_title)),
                if info.author == "Unknown" {
                    "File Upload".to_string()
                } else {
                    info.author
                },
            ),
            _ => (clean_title(&raw_title), info.author),
        };
        Self {
            uri: info.webpage_url,
            duration_secs: info.duration_secs,
            requester: requester.into(),
            thumbnail: info.thumbnail,
            source,
            title,
            author,
            is_live: info.is_live,
            ui_link: info.ui_link,
        }
    }

    pub fn link_for_ui(&self) -> String {
        self.ui_link
            .clone()
            .unwrap_or_else(|| self.uri_safe())
    }

    pub fn duration_display(&self) -> String {
        match self.duration_secs {
            Some(s) if !self.is_live => fmt_sec(s),
            _ => "Live / Unknown".into(),
        }
    }
}

impl From<&crate::sources::ResolvedMeta> for ResolvedInfo {
    fn from(m: &crate::sources::ResolvedMeta) -> Self {
        Self {
            webpage_url: m.webpage_url.clone(),
            is_spotify_match: m.is_spotify_match,
            ui_link: m.ui_link.clone(),
            title: m.title.clone(),
            author: m.author.clone(),
            duration_secs: m.duration_secs,
            thumbnail: m.thumbnail.clone(),
            is_live: m.is_live,
        }
    }
}

impl From<crate::sources::ResolvedMeta> for ResolvedInfo {
    fn from(m: crate::sources::ResolvedMeta) -> Self {
        Self {
            webpage_url: m.webpage_url,
            title: m.title,
            author: m.author,
            duration_secs: m.duration_secs,
            thumbnail: m.thumbnail,
            is_live: m.is_live,
            is_spotify_match: m.is_spotify_match,
            ui_link: m.ui_link,
        }
    }
}

const DISCORD_CDN_HOSTS: &[&str] = &["cdn.discordapp.com", "media.discordapp.net"];

fn classify_source(uri: &str) -> SourceTag {
    if let Ok(u) = Url::parse(uri) {
        let host = u.host_str().unwrap_or_default();
        if DISCORD_CDN_HOSTS.contains(&host) {
            return SourceTag::Discord;
        }
        if host.ends_with("youtube.com") || host == "youtu.be" {
            return SourceTag::Youtube;
        }
        if host.contains("soundcloud.com") {
            return SourceTag::SoundCloud;
        }
        if host.contains("spotify.com") {
            return SourceTag::SpotifyMatched;
        }
        if u.scheme().starts_with("http") {
            return SourceTag::File;
        }
    }
    SourceTag::File
}

impl Track {
    pub fn from_aux(meta: &AuxMetadata, requester: impl Into<String>) -> Self {
        let uri = meta.source_url.clone().unwrap_or_default();
        let source = classify_source(&uri);
        let raw_title = meta.title.clone().unwrap_or_default();
        let is_live = meta.duration.is_none();

        let (title, author) = match source {
            SourceTag::Discord | SourceTag::File => {
                let title = filename_from_url(&uri)
                    .or_else(|| {
                        let looks_hashy = raw_title.len() >= 20
                            && raw_title
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                        (!raw_title.is_empty() && !looks_hashy && raw_title != uri)
                            .then(|| clean_title(&raw_title))
                    })
                    .or_else(|| {
                        Url::parse(&uri)
                            .ok()
                            .and_then(|u| {
                                u.path().rsplit('/').next().map(|s| s.to_string())
                            })
                            .filter(|s| !s.is_empty())
                    })
                    .unwrap_or_else(|| "Unknown File".into());
                let author = meta
                    .artist
                    .clone()
                    .filter(|a| a != "Unknown" && !a.is_empty())
                    .unwrap_or_else(|| "File Upload".into());
                (title, author)
            }
            _ => (
                clean_title(if raw_title.is_empty() { "Unknown" } else { &raw_title }),
                meta.channel
                    .clone()
                    .or_else(|| meta.artist.clone())
                    .unwrap_or_else(|| "Unknown".into()),
            ),
        };

        Self {
            uri,
            duration_secs: meta.duration.map(|d| d.as_secs()),
            requester: requester.into(),
            thumbnail: meta.thumbnail.clone().unwrap_or_default(),
            source,
            title,
            author,
            is_live,
            ui_link: None,
        }
    }
}

#[derive(Debug)]
pub struct GuildState {
    pub queue: VecDeque<Track>,
    pub current: Option<Track>,
    pub previous: Option<Track>,
    pub current_handle: Option<songbird::tracks::TrackHandle>,
    pub loop_mode: LoopMode,
    pub volume: u16,
    pub np_message: Option<(ChannelId, MessageId)>,
    pub home_channel: Option<ChannelId>,
    pub voice_channel_id: Option<ChannelId>,
    pub playing: bool,
    pub paused: bool,
    /// True while a command-driven stop is waiting for the TrackEnd event it
    /// caused. Timestamped so a leftover flag can never swallow a later
    /// NATURAL track end (which would wedge the player in "playing" state).
    stop_intentional: bool,
    stop_flag_at: Option<std::time::Instant>,
    pub error_streak: u32,
    pub recovering: bool,
    /// Open-circuit deadline after a systemic stop (max-errors or failed
    /// recovery). While open, automatic Error/End events are dropped silently
    /// so a deterministically-failing source can't spin a rejoin+retry storm.
    /// Explicit user commands (play/skip/stop/...) clear it — user intent
    /// always breaks the circuit.
    pub cooldown_until: Option<std::time::Instant>,
    /// Last time the 24/7 watchdog rejoined after an unexpected voice drop.
    /// Bounds rejoin flapping (e.g. an admin repeatedly kicking the bot) to
    /// ~once a minute. Deliberately preserved across state resets.
    pub stay_last_rejoin: Option<std::time::Instant>,
    /// Recently started tracks, oldest first. Autoplay consults this so the
    /// radio never repeats itself; capped so a 24/7 radio can't grow memory
    /// without bound.
    pub autoplay_history: VecDeque<HistoryEntry>,
    /// True when the current track plays from an in-memory Opus cache
    /// (set after the first seek) — native seeks are instant on it.
    pub current_is_cached: bool,
    pub inactivity_task: Option<tokio::task::JoinHandle<()>>,
    pub stay_return_task: Option<tokio::task::JoinHandle<()>>,
}

/// How long a command-set intentional-stop flag remains believable. Any
/// TrackEnd arriving after this window is treated as a natural end.
const STOP_FLAG_TTL: std::time::Duration = std::time::Duration::from_secs(10);

impl GuildState {
    pub fn request_stop(&mut self) {
        self.stop_intentional = true;
        self.stop_flag_at = Some(std::time::Instant::now());
    }

    /// Consumes the intentional-stop flag; returns false if it is absent or
    /// too old to trust (i.e. treat this end as natural).
    pub fn take_fresh_stop(&mut self) -> bool {
        if !self.stop_intentional {
            return false;
        }
        self.stop_intentional = false;
        self.stop_flag_at.take().is_some_and(|t| t.elapsed() < STOP_FLAG_TTL)
    }

    pub fn with_default_volume(volume: u16) -> Self {
        Self {
            volume,
            ..Default::default()
        }
    }

    /// True while a systemic stop is cooling down (see `cooldown_until`).
    pub fn circuit_open(&self) -> bool {
        self.cooldown_until
            .is_some_and(|t| std::time::Instant::now() < t)
    }

    /// User did something explicit — the failure may be over, let events flow.
    pub fn break_circuit(&mut self) {
        self.cooldown_until = None;
    }
}

impl Default for GuildState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            current: None,
            previous: None,
            current_handle: None,
            loop_mode: LoopMode::Off,
            volume: 100,
            np_message: None,
            home_channel: None,
            voice_channel_id: None,
            playing: false,
            paused: false,
            stop_intentional: false,
            stop_flag_at: None,
            error_streak: 0,
            recovering: false,
            cooldown_until: None,
            stay_last_rejoin: None,
            autoplay_history: VecDeque::new(),
            current_is_cached: false,
            inactivity_task: None,
            stay_return_task: None,
        }
    }
}

pub struct Registry {
    inner: DashMap<GuildId, GuildState>,
    default_volume: std::sync::atomic::AtomicU16,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            inner: DashMap::new(),
            default_volume: std::sync::atomic::AtomicU16::new(100),
        }
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_default_volume(&self, volume: u16) {
        self.default_volume
            .store(volume, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get(&self, guild_id: GuildId) -> dashmap::mapref::one::RefMut<'_, GuildId, GuildState> {
        self.inner
            .entry(guild_id)
            .or_insert_with(|| {
                let vol = self.default_volume.load(std::sync::atomic::Ordering::Relaxed);
                GuildState::with_default_volume(vol)
            })
    }

    /// Non-creating lookup — returns None if the guild has never been touched.
    /// Use this in hot paths like VoiceStateUpdate where phantom inserts would
    /// cause unbounded registry growth and defeat GC.
    pub fn get_if_exists(
        &self,
        guild_id: GuildId,
    ) -> Option<dashmap::mapref::one::RefMut<'_, GuildId, GuildState>> {
        self.inner.get_mut(&guild_id)
    }

    pub fn remove(&self, guild_id: GuildId) {
        self.inner.remove(&guild_id);
    }

    /// Reclaims hashbrown shard capacity after mass removal.
    /// DashMap's shards grow but never shrink on `remove`; without this the
    /// registry's internal buckets stay at peak size even after pruning, which
    /// pins RSS in long-running bots that have seen many guilds.
    pub fn shrink_to_fit(&self) {
        self.inner.shrink_to_fit();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn guild_ids(&self) -> Vec<GuildId> {
        self.inner.iter().map(|e| *e.key()).collect()
    }

    pub fn total_queue_len(&self) -> usize {
        self.inner.iter().map(|g| g.queue.len()).sum()
    }
}

pub fn fmt_sec(s: u64) -> String {
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_sec_ports_js_behavior() {
        assert_eq!(fmt_sec(0), "0:00");
        assert_eq!(fmt_sec(59), "0:59");
        assert_eq!(fmt_sec(60), "1:00");
        assert_eq!(fmt_sec(61), "1:01");
        assert_eq!(fmt_sec(3661), "1:01:01");
        assert_eq!(fmt_sec(7385), "2:03:05");
    }

    #[test]
    fn clean_title_strips_noise() {
        assert_eq!(clean_title("Song Name (Official Video)"), "Song Name");
        assert_eq!(clean_title("[Official Audio] Song"), "Song");
        assert_eq!(clean_title("Song (Lyrics) - Artist"), "Song - Artist");
        assert_eq!(clean_title("Song [Re-Upload] x"), "Song x");
        assert_eq!(clean_title("Slowed Song (slowed + reverb)"), "Slowed Song");
        assert_eq!(clean_title("A  B   C"), "A B C");
        assert_eq!(clean_title("- Song -"), "Song");
        assert_eq!(clean_title("(official)"), "(official)");
    }

    #[test]
    fn filename_from_url_extracts_pretty_names() {
        assert_eq!(
            filename_from_url("https://cdn.discordapp.com/attachments/1/2/My_Cool-Song.mp3?ex=123"),
            Some("My Cool Song".into())
        );
        assert_eq!(
            filename_from_url("https://example.com/hashnameabcdefghijklmnop.mp3"),
            None
        );
        assert_eq!(filename_from_url(""), None);
    }

    #[test]
    fn loop_mode_cycles() {
        assert_eq!(LoopMode::Off.cycle(), LoopMode::Track);
        assert_eq!(LoopMode::Track.cycle(), LoopMode::Queue);
        assert_eq!(LoopMode::Queue.cycle(), LoopMode::Off);
    }

    #[test]
    fn source_classification() {
        assert_eq!(
            classify_source("https://www.youtube.com/watch?v=x"),
            SourceTag::Youtube
        );
        assert_eq!(classify_source("https://youtu.be/x"), SourceTag::Youtube);
        assert_eq!(
            classify_source("https://cdn.discordapp.com/attachments/1/2/a.mp3"),
            SourceTag::Discord
        );
        assert_eq!(
            classify_source("https://media.discordapp.net/attachments/1/2/a.mp3"),
            SourceTag::Discord
        );
        assert_eq!(
            classify_source("https://open.spotify.com/track/x"),
            SourceTag::SpotifyMatched
        );
        assert_eq!(
            classify_source("https://somehost.com/song.mp3"),
            SourceTag::File
        );
    }

    #[test]
    fn track_from_aux_youtube() {
        let mut m = AuxMetadata::default();
        m.title = Some("Never Gonna Give You Up (Official Video)".into());
        m.channel = Some("Rick Astley".into());
        m.source_url = Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ".into());
        m.duration = Some(std::time::Duration::from_secs(213));

        let t = Track::from_aux(&m, "kurenai");
        assert_eq!(t.source, SourceTag::Youtube);
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.author, "Rick Astley");
        assert_eq!(t.requester, "kurenai");
        assert_eq!(t.duration_secs, Some(213));
        assert!(!t.is_live);
    }

    #[test]
    fn track_from_aux_discord_file_uses_filename() {
        let mut m = AuxMetadata::default();
        m.title = Some("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".into());
        m.source_url =
            Some("https://cdn.discordapp.com/attachments/1/2/my_track_name.flac".into());

        let t = Track::from_aux(&m, "user");
        assert_eq!(t.source, SourceTag::Discord);
        assert_eq!(t.title, "my track name");
        assert_eq!(t.author, "File Upload");
    }

    #[test]
    fn stop_flag_expires_and_consumes() {
        let mut st = GuildState::default();
        assert!(!st.take_fresh_stop(), "no flag set -> natural end");

        st.request_stop();
        assert!(st.take_fresh_stop(), "fresh flag -> command owns flow");
        assert!(!st.stop_intentional, "flag consumed");
        assert!(!st.take_fresh_stop());

        // Simulate a stale flag (cleanup ran while idle, End never came).
        st.request_stop();
        st.stop_flag_at = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(30))
                .unwrap(),
        );
        assert!(!st.take_fresh_stop(), "stale flag must NOT swallow a natural end");
    }

    #[test]
    fn circuit_breaker_opens_and_breaks() {
        let mut st = GuildState::default();
        assert!(!st.circuit_open(), "fresh state -> circuit closed");
        assert!(st.stay_last_rejoin.is_none(), "no auto-rejoin yet");

        st.cooldown_until =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        assert!(st.circuit_open(), "future deadline -> circuit open");

        st.break_circuit();
        assert!(!st.circuit_open(), "explicit user action breaks the circuit");

        st.cooldown_until = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap(),
        );
        assert!(!st.circuit_open(), "elapsed deadline -> circuit closed");
    }

    #[test]
    fn registry_isolation_per_guild() {
        let reg = Registry::new();
        let gid = GuildId::new(1);
        reg.get(gid).queue.push_back(Track {
            uri: String::new(),
            duration_secs: None,
            requester: "x".into(),
            thumbnail: String::new(),
            source: SourceTag::File,
            title: "t".into(),
            author: String::new(),
            is_live: false,
            ui_link: None,
        });
        assert_eq!(reg.get(gid).queue.len(), 1);
        assert_eq!(reg.get(GuildId::new(2)).queue.len(), 0);
        assert_eq!(reg.total_queue_len(), 1);
    }

    #[test]
    fn video_id_extraction() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=RDdQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            extract_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".into())
        );
        assert_eq!(extract_video_id("ytsearch1:coldplay yellow"), None);
        assert_eq!(
            extract_video_id("https://soundcloud.com/artist/track"),
            None
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=short"),
            None
        );
        assert_eq!(extract_video_id("not a url"), None);
    }

    #[test]
    fn author_normalization_catches_topic_channels() {
        assert_eq!(normalize_author("Coldplay"), "coldplay");
        assert_eq!(normalize_author("Coldplay - Topic"), "coldplay");
        assert_eq!(normalize_author("  Coldplay   -   Topic  "), "coldplay");
        assert_eq!(normalize_author("The Weeknd"), "the weeknd");
        assert_eq!(normalize_author(""), "");
    }

    fn mix_cand(
        id: &str,
        title: &str,
        author: &str,
        dur: Option<u64>,
        live: bool,
    ) -> MixCandidate {
        MixCandidate {
            video_id: id.into(),
            webpage_url: format!("https://www.youtube.com/watch?v={id}"),
            title: title.into(),
            author: author.into(),
            duration_secs: dur,
            thumbnail: String::new(),
            is_live: live,
        }
    }

    #[test]
    fn autoplay_pick_applies_every_filter() {
        let seed = "SEEDSEED111";
        let seed_author = normalize_author("Coldplay");
        fn hist(uri: &str, title: &str, author: &str) -> HistoryEntry {
            HistoryEntry::new(uri, title, author)
        }
        let history: VecDeque<HistoryEntry> = vec![hist(
            "https://www.youtube.com/watch?v=PLAYEDPLAY1",
            "Old Song",
            "Someone",
        )]
        .into_iter()
        .collect();
        let queued = vec![hist(
            "https://www.youtube.com/watch?v=QUEUEDQUEU1",
            "Waiting Song",
            "Someone",
        )];

        // Index 0: the seed itself. 1: same artist. 2: played (exact id).
        // 3: queued. 4: re-upload twin (different id, same title stub).
        // 5-6: cover + mashup. 7: live. 8: too long. 9: unknown length.
        // 10: empty title. 11-12: clean winners, in mix order.
        let cands = vec![
            mix_cand(seed, "Seed Song", "Coldplay", Some(200), false),
            mix_cand("AAAAAAAAAAA", "Other", "Coldplay - Topic", Some(200), false),
            mix_cand("PLAYEDPLAY1", "Old Song", "Someone", Some(200), false),
            mix_cand("QUEUEDQUEU1", "Waiting Song", "Someone", Some(200), false),
            mix_cand(
                "GGGGGGGGGGG",
                "Old Song - Random Uploader",
                "Uploader",
                Some(210),
                false
            ),
            mix_cand("HHHHHHHHHHH", "Brand New (cover)", "Singer", Some(200), false),
            mix_cand("IIIIIIIIIII", "Old Song Mega Mashup", "DJ X", Some(200), false),
            mix_cand("BBBBBBBBBBB", "Live", "Someone", Some(200), true),
            mix_cand("CCCCCCCCCCC", "Epic 3h mix", "Someone", Some(3 * 3600), false),
            mix_cand("DDDDDDDDDDD", "Mystery", "Someone", None, false),
            mix_cand("EEEEEEEEEEE", "   ", "Someone", Some(200), false),
            mix_cand("FFFFFFFFFFF", "Fresh Track", "Someone Else", Some(180), false),
            mix_cand("JJJJJJJJJJJ", "Another One", "Third Party", Some(190), false),
        ];
        assert_eq!(
            autoplay_pick(&cands, seed, &seed_author, &history, &queued),
            vec![11, 12]
        );
    }

    #[test]
    fn title_stubs_catch_reuploads() {
        assert_eq!(normalize_title_stub("Khat"), "khat");
        assert_eq!(normalize_title_stub("Khat - Navjot Ahuja"), "khat");
        assert_eq!(normalize_title_stub("KHAT (Lyrics)"), "khat");
        assert_eq!(
            normalize_title_stub("Arz Kiya Hai | Coke Studio Bharat"),
            "arz kiya hai"
        );
        assert_eq!(
            normalize_title_stub("Like Him (feat. Lola Young)"),
            "like him"
        );
        assert_eq!(normalize_title_stub("(Untitled)"), "(untitled)");
        assert_eq!(normalize_title_stub("Higher Power"), "higher power");
    }

    #[test]
    fn autoplay_pick_returns_empty_when_everything_filtered() {
        let cands = vec![mix_cand(
            "AAAAAAAAAAA",
            "Same Artist",
            "Coldplay",
            Some(200),
            false,
        )];
        let history = VecDeque::new();
        let queued: Vec<HistoryEntry> = vec![];
        assert!(autoplay_pick(&cands, "SEEDSEED111", &normalize_author("coldplay"), &history, &queued).is_empty());
        assert!(autoplay_pick(&[], "SEEDSEED111", &normalize_author("x"), &history, &queued).is_empty());
    }

    #[test]
    fn cover_blocklist() {
        // NOTE: "Noor, Khan, Madhurxo - Aarzu" (skit-laden video version)
        // carries no blockable keyword — that species dies via the YTM audio
        // upgrade path instead (see audio_upgrade_verification).
        for t in [
            "Khat (cover)",
            "Best Mashup 2024",
            "Song slowed + reverb",
            "Track (8d audio)",
            "Hit Lofi Remix",
            "Unplugged Version",
            "Dance Karaoke",
            "Bairan – Animated Love Story | Banjaare",
            "Film Trailer Theme",
            "Tour Vlog Part 2",
        ] {
            assert!(is_non_song(t), "{t} should be blocked");
        }
        for t in [
            "Khat",
            "Arz Kiya Hai",
            "Higher Power",
            "Story of My Life",
            "Drama",
            "Dooron Dooron (Live from The Voice Notes Concert)",
        ] {
            assert!(!is_non_song(t), "{t} should pass");
        }
    }

    #[test]
    fn audio_upgrade_verification() {
        let history: VecDeque<HistoryEntry> = vec![HistoryEntry::new(
            "https://www.youtube.com/watch?v=PLAYEDPLAY1",
            "Old Song",
            "Someone",
        )]
        .into_iter()
        .collect();
        let queued: Vec<HistoryEntry> = vec![];
        // Clean upgrade: same song, fresh id, different author than seed.
        assert!(audio_upgrade_ok(
            "NEWNEWNEW11",
            "Aarzu",
            "Noor",
            "aarzu",
            "SEEDSEED111",
            "coldplay",
            &history,
            &queued
        ));
        // YTM returned a different song.
        assert!(!audio_upgrade_ok(
            "NEWNEWNEW11",
            "Something Else",
            "Noor",
            "aarzu",
            "SEEDSEED111",
            "coldplay",
            &history,
            &queued
        ));
        // Same artist back-to-back.
        assert!(!audio_upgrade_ok(
            "NEWNEWNEW11",
            "Aarzu",
            "Coldplay",
            "aarzu",
            "SEEDSEED111",
            "coldplay",
            &history,
            &queued
        ));
        // Upgraded id was already played.
        assert!(!audio_upgrade_ok(
            "PLAYEDPLAY1",
            "Aarzu",
            "Noor",
            "aarzu",
            "SEEDSEED111",
            "coldplay",
            &history,
            &queued
        ));
        // Upgraded id is the seed itself.
        assert!(!audio_upgrade_ok(
            "SEEDSEED111",
            "Aarzu",
            "Noor",
            "aarzu",
            "SEEDSEED111",
            "coldplay",
            &history,
            &queued
        ));
    }

    #[test]
    fn rand_below_stays_in_bounds() {
        assert_eq!(rand_below(0), 0);
        assert_eq!(rand_below(1), 0);
        for _ in 0..200 {
            assert!(rand_below(5) < 5);
        }
    }

    #[test]
    fn track_key_prefers_video_id() {
        assert_eq!(
            track_key("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "dQw4w9WgXcQ"
        );
        assert_eq!(
            track_key("https://soundcloud.com/a/b"),
            "https://soundcloud.com/a/b"
        );
    }
}

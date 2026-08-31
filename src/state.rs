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

    pub fn remove(&self, guild_id: GuildId) {
        self.inner.remove(&guild_id);
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
}

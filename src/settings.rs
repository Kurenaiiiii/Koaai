use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub bot: BotConfig,
    pub audio: AudioConfig,
    pub sources: SourcesConfig,
    pub logging: LoggingConfig,
    pub banner: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BotConfig {
    pub default_prefix: String,
    pub items_per_page: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub auto_leave_secs: u64,
    pub empty_channel_grace_secs: u64,
    pub error_streak_rejoin_at: u32,
    pub max_consecutive_errors: u32,
    pub default_volume: u16,
    pub max_volume: u16,
    pub bitrate_kbps: u32,
    pub np_delete_previous: bool,
    pub channel_status_updates: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SourcesConfig {
    pub youtube_enabled: bool,
    pub soundcloud_enabled: bool,
    pub spotify_enabled: bool,
    pub apple_music_enabled: bool,
    pub default_search: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file: FileLogConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct FileLogConfig {
    pub enabled: bool,
    pub path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bot: BotConfig::default(),
            audio: AudioConfig::default(),
            sources: SourcesConfig::default(),
            logging: LoggingConfig::default(),
            banner: true,
        }
    }
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            default_prefix: "+".into(),
            items_per_page: 10,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            auto_leave_secs: 300,
            empty_channel_grace_secs: 10,
            error_streak_rejoin_at: 3,
            max_consecutive_errors: 6,
            default_volume: 100,
            max_volume: 200,
            // Opus target for streams that must be re-encoded (file uploads,
            // SoundCloud mp3, etc). YouTube sources are Opus already and are
            // passed through untouched by songbird while volume == 100%.
            bitrate_kbps: 256,
            np_delete_previous: true,
            channel_status_updates: true,
        }
    }
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            youtube_enabled: true,
            soundcloud_enabled: true,
            spotify_enabled: true,
            apple_music_enabled: true,
            default_search: "ytsearch".into(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: FileLogConfig::default(),
        }
    }
}

impl Default for FileLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "logs".into(),
        }
    }
}

const DEFAULT_TOML: &str = r#"# Koaai configuration — edit freely, restart to apply.
# Secrets (TOKEN, SPOTIFY_CLIENT_ID/SECRET) stay in .env.

[bot]
default_prefix = "+"
items_per_page = 10

[audio]
auto_leave_secs = 300          # leave VC after this many idle seconds
empty_channel_grace_secs = 10  # wait before leaving an empty channel
error_streak_rejoin_at = 3     # consecutive track errors before voice rejoin
max_consecutive_errors = 6     # consecutive errors before stopping playback
default_volume = 100           # starting volume per guild (1-200)
max_volume = 200               # volume clamp ceiling
bitrate_kbps = 256             # opus target (0 = auto, max 512); YT sources pass through untouched while volume is 100%
np_delete_previous = true      # delete the old Now Playing card when the next starts
channel_status_updates = true  # set the VC status to the current track

[sources]
youtube_enabled = true
soundcloud_enabled = true
spotify_enabled = true         # also needs SPOTIFY_CLIENT_ID/SECRET in .env
apple_music_enabled = true
default_search = "ytsearch"    # (legacy) name searches use the fixed chain ytmusic->youtube->spotify->soundcloud; this key only matters for future use

[logging]
level = "info"                 # info | warn | error
file.enabled = false
file.path = "logs"

banner = true                  # ascii art on startup
"#;

pub fn load() -> Config {
    let path = "config.toml";
    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str(&contents) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[CONFIG] failed to parse {path}: {e} — using defaults");
                Config::default()
            }
        },
        Err(_) => {
            let _ = std::fs::write(path, DEFAULT_TOML);
            println!("wrote default config to {path}");
            Config::default()
        }
    }
}

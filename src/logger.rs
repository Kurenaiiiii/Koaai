use std::fs::OpenOptions;
use std::io::Write;
use std::sync::OnceLock;

use chrono::Local;

pub struct LogConfig {
    pub level: Level,
    pub file_enabled: bool,
    pub file_path: String,
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Level {
    Info,
    Warn,
    Error,
}

static CONFIG: OnceLock<LogConfig> = OnceLock::new();

pub fn init(level: Level, file_enabled: bool, file_path: &str) {
    let _ = CONFIG.set(LogConfig {
        level,
        file_enabled,
        file_path: file_path.to_string(),
    });
}

fn config() -> &'static LogConfig {
    CONFIG.get_or_init(|| LogConfig {
        level: Level::Info,
        file_enabled: false,
        file_path: "logs".into(),
    })
}

const RESET: &str = "\x1b[0m";

fn label_color(level: &str) -> (&'static str, &'static str) {
    match level {
        "INFO" => ("INFO", "\x1b[1m\x1b[3;42m"),
        "WARN" => ("WARN", "\x1b[1m\x1b[3;43m"),
        "ERROR" => ("ERROR", "\x1b[1m\x1b[3;41m"),
        "DEBUG" => ("DEBUG", "\x1b[1m\x1b[3;45m"),
        "SOURCES" => ("SOURCES", "\x1b[1m\x1b[3;46m"),
        "STARTED" => ("STARTED", "\x1b[1m\x1b[3;44m"),
        "NETWORK" => ("NETWORK", "\x1b[1m\x1b[3;44m"),
        _ => ("LOG", ""),
    }
}

pub fn log(level: &str, category: &str, msg: &str) {
    let cfg = config();
    let min = match level {
        "DEBUG" | "SOURCES" => Level::Info,
        "INFO" | "STARTED" | "NETWORK" => Level::Info,
        "WARN" => Level::Warn,
        "ERROR" => Level::Error,
        _ => Level::Info,
    };
    if min < cfg.level {
        return;
    }

    let time = Local::now().format("%H:%M:%S%.3f");
    let (label, color) = label_color(level);
    let cat = if category.is_empty() {
        String::new()
    } else {
        format!(": {category} >")
    };

    println!("[{time}] {color}[{label}] >{RESET}{cat} {msg}");

    if cfg.file_enabled {
        let _ = std::fs::create_dir_all(&cfg.file_path);
        let path = format!("{}/koaai.log", cfg.file_path);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let iso = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%z");
            let _ = writeln!(f, "[{iso}] [{label}] {cat} {msg}");
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($cat:expr, $($arg:tt)*) => {
        $crate::logger::log("INFO", $cat, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($cat:expr, $($arg:tt)*) => {
        $crate::logger::log("WARN", $cat, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($cat:expr, $($arg:tt)*) => {
        $crate::logger::log("ERROR", $cat, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_sources {
    ($cat:expr, $($arg:tt)*) => {
        $crate::logger::log("SOURCES", $cat, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_started {
    ($cat:expr, $($arg:tt)*) => {
        $crate::logger::log("STARTED", $cat, &format!($($arg)*))
    };
}

pub fn print_banner(version: &str) {
    let ascii = r"
██╗  ██╗ ██████╗  █████╗  █████╗ ██╗
██║ ██╔╝██╔═══██╗██╔══██╗██╔══██╗██║
█████╔╝ ██║   ██║███████║███████║██║
██╔═██╗ ██║   ██║██╔══██║██╔══██║██║
██║  ██╗╚██████╔╝██║  ██║██║  ██║██║
╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝";
    println!("\x1b[32m{ascii}\x1b[0m");
    println!(
        "\x1b[32m  v{version}\x1b[0m \x1b[2m— single-process music engine\x1b[0m\n"
    );
}

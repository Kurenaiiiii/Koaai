use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};
use tracing::info;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS guild_settings (
    guild_id TEXT PRIMARY KEY,
    prefix   TEXT NOT NULL DEFAULT '+'
);
CREATE TABLE IF NOT EXISTS noprefix_users (
    user_id TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS stay_channels (
    guild_id   TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS reports (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id    TEXT    UNIQUE NOT NULL,
    user_id      TEXT    NOT NULL,
    username     TEXT    NOT NULL,
    guild_id     TEXT,
    guild_name   TEXT,
    channel_id   TEXT,
    channel_name TEXT,
    category     TEXT    NOT NULL,
    description  TEXT    NOT NULL,
    steps        TEXT,
    extra        TEXT,
    resolved     INTEGER DEFAULT 0,
    resolved_at  TEXT,
    created_at   TEXT    NOT NULL
);
";

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        // WAL is preferred (crash safety), but some container filesystems
        // can't do it — fall back to the default journal instead of refusing
        // to boot.
        if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
            tracing::warn!(path, error = %e, "WAL mode unavailable, using default journal mode");
        }
        conn.execute_batch(SCHEMA)?;
        info!(path, "database opened and schema verified");
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn checkpoint(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    pub fn prefixes(&self) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT guild_id, prefix FROM guild_settings")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_prefix(&self, guild_id: &str, prefix: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO guild_settings (guild_id, prefix) VALUES (?, ?)",
            [guild_id, prefix],
        )?;
        Ok(())
    }

    pub fn delete_prefix(&self, guild_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM guild_settings WHERE guild_id = ?", [guild_id])?;
        Ok(())
    }

    pub fn stay_channels(&self) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT guild_id, channel_id FROM stay_channels")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_stay_channel(&self, guild_id: &str, channel_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO stay_channels (guild_id, channel_id) VALUES (?, ?)",
            [guild_id, channel_id],
        )?;
        Ok(())
    }

    pub fn delete_stay_channel(&self, guild_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM stay_channels WHERE guild_id = ?", [guild_id])?;
        Ok(())
    }

    pub fn prefix_for(&self, guild_id: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT prefix FROM guild_settings WHERE guild_id = ?",
            [guild_id],
            |r| r.get(0),
        )
        .optional()
    }

    pub fn insert_report(&self, r: &ReportRow) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO reports
                (report_id, user_id, username, guild_id, guild_name, channel_id, channel_name,
                 category, description, steps, extra, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                &r.report_id,
                &r.user_id,
                &r.username,
                r.guild_id.as_deref().unwrap_or(""),
                r.guild_name.as_deref().unwrap_or(""),
                r.channel_id.as_deref().unwrap_or(""),
                r.channel_name.as_deref().unwrap_or(""),
                &r.category,
                &r.description,
                r.steps.as_deref().unwrap_or(""),
                r.extra.as_deref().unwrap_or(""),
                &r.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn reports(&self, filter: ReportFilter) -> Result<Vec<ReportRow>, rusqlite::Error> {
        let query = match filter {
            ReportFilter::All => "SELECT * FROM reports ORDER BY created_at DESC",
            ReportFilter::Open => "SELECT * FROM reports WHERE resolved = 0 ORDER BY created_at DESC",
            ReportFilter::Resolved => {
                "SELECT * FROM reports WHERE resolved = 1 ORDER BY created_at DESC"
            }
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(query)?;
        let rows = stmt
            .query_map([], map_report)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn report_counts(&self) -> Result<(u64, u64, u64), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let count = |sql: &str| -> Result<u64, rusqlite::Error> {
            conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|v| v as u64)
        };
        let total = count("SELECT COUNT(*) FROM reports")?;
        let open = count("SELECT COUNT(*) FROM reports WHERE resolved = 0")?;
        let resolved = count("SELECT COUNT(*) FROM reports WHERE resolved = 1")?;
        Ok((total, open, resolved))
    }

    pub fn report_by_id(&self, report_id: &str) -> Result<Option<ReportRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM reports WHERE report_id = ?")?;
        stmt.query_row([report_id], map_report).optional()
    }

    /// Marks a report resolved; returns false if it was already resolved.
    pub fn resolve_report(&self, report_id: &str, resolved_at: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE reports SET resolved = 1, resolved_at = ? WHERE report_id = ? AND resolved = 0",
            [resolved_at, report_id],
        )?;
        Ok(changed > 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportFilter {
    All,
    Open,
    Resolved,
}

impl ReportFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Open,
            Self::Open => Self::Resolved,
            Self::Resolved => Self::All,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "open" => Self::Open,
            "resolved" => Self::Resolved,
            _ => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "📋 All",
            Self::Open => "🔴 Open",
            Self::Resolved => "✅ Resolved",
        }
    }

    pub fn button_label(self) -> &'static str {
        match self {
            Self::All => "🔴 Open Only",
            Self::Open => "✅ Resolved Only",
            Self::Resolved => "📋 Show All",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReportRow {
    pub report_id: String,
    pub user_id: String,
    pub username: String,
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub category: String,
    pub description: String,
    pub steps: Option<String>,
    pub extra: Option<String>,
    pub resolved: bool,
    pub resolved_at: Option<String>,
    pub created_at: String,
}

fn map_report(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportRow> {
    let opt = |col: rusqlite::Result<Option<String>, rusqlite::Error>| -> rusqlite::Result<Option<String>> {
        col.map(|s| s.filter(|v| !v.is_empty()))
    };
    Ok(ReportRow {
        report_id: r.get("report_id")?,
        user_id: r.get("user_id")?,
        username: r.get("username")?,
        guild_id: opt(r.get("guild_id"))?,
        guild_name: opt(r.get("guild_name"))?,
        channel_id: opt(r.get("channel_id"))?,
        channel_name: opt(r.get("channel_name"))?,
        category: r.get("category")?,
        description: r.get("description")?,
        steps: opt(r.get("steps"))?,
        extra: opt(r.get("extra"))?,
        resolved: r.get::<_, i64>("resolved")? != 0,
        resolved_at: opt(r.get("resolved_at"))?,
        created_at: r.get("created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_real_bot_db() {
        if !std::path::Path::new("bot.db").exists() {
            return;
        }
        let db = Db::open("bot.db").expect("open real bot.db");
        let prefixes = db.prefixes().unwrap();
        let stays = db.stay_channels().unwrap();
        tracing::info!(?prefixes, ?stays, "loaded existing data");
    }

    #[test]
    fn roundtrip_prefix_and_stay() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.prefix_for("123").unwrap(), None);
        db.set_prefix("123", "?").unwrap();
        assert_eq!(db.prefix_for("123").unwrap().as_deref(), Some("?"));
        db.set_stay_channel("123", "456").unwrap();
        assert_eq!(
            db.stay_channels().unwrap(),
            vec![("123".into(), "456".into())]
        );
        db.delete_stay_channel("123").unwrap();
        assert!(db.stay_channels().unwrap().is_empty());
    }

    #[test]
    fn report_roundtrip_and_filters() {
        let db = Db::open(":memory:").unwrap();
        let mk = |id: &str, resolved: bool| ReportRow {
            report_id: id.into(),
            user_id: "42".into(),
            username: "tester".into(),
            guild_id: Some("1".into()),
            guild_name: Some("Guild".into()),
            channel_id: Some("9".into()),
            channel_name: Some("general".into()),
            category: "bug".into(),
            description: "desc".into(),
            steps: Some("1. do".into()),
            extra: None,
            resolved,
            resolved_at: None,
            created_at: "2026-08-25T00:00:00+00:00".into(),
        };
        db.insert_report(&mk("RPT-A", false)).unwrap();
        db.insert_report(&mk("RPT-B", false)).unwrap();

        let (total, open, res) = db.report_counts().unwrap();
        assert_eq!((total, open, res), (2, 2, 0));

        // resolve twice -> second call must report no change
        assert!(db.resolve_report("RPT-A", "2026-08-25T01:00:00+00:00").unwrap());
        assert!(!db
            .resolve_report("RPT-A", "2026-08-25T02:00:00+00:00")
            .unwrap());

        let (total, open, res) = db.report_counts().unwrap();
        assert_eq!((total, open, res), (2, 1, 1));

        let open_rows = db.reports(ReportFilter::Open).unwrap();
        assert_eq!(open_rows.len(), 1);
        assert_eq!(open_rows[0].report_id, "RPT-B");

        let done = db.report_by_id("RPT-A").unwrap().unwrap();
        assert!(done.resolved);
        assert_eq!(done.resolved_at.as_deref(), Some("2026-08-25T01:00:00+00:00"));

        assert!(db.report_by_id("RPT-ZZ").unwrap().is_none());
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use serenity::model::id::{ChannelId, GuildId};
use tokio::sync::RwLock;

use crate::db;
use crate::state::Registry;

pub fn default_prefix(cfg: &crate::settings::Config) -> String {
    cfg.bot.default_prefix.clone()
}

pub struct Core {
    pub http_api: Arc<serenity::http::Http>,
    pub http_client: reqwest::Client,
    pub db: db::Db,
    pub registry: Registry,
    pub prefixes: RwLock<HashMap<GuildId, String>>,
    pub stay_channels: RwLock<HashMap<GuildId, ChannelId>>,
    pub voice: Arc<songbird::Songbird>,
    pub bot_id: serenity::model::id::UserId,
    pub cfg: crate::settings::Config,
}

impl Core {
    pub async fn load(
        db: db::Db,
        http_api: Arc<serenity::http::Http>,
        voice: Arc<songbird::Songbird>,
        bot_id: serenity::model::id::UserId,
        cfg: crate::settings::Config,
    ) -> Result<Arc<Self>, String> {
        let prefixes = db
            .prefixes()
            .map_err(|e| format!("loading prefixes: {e}"))?
            .into_iter()
            .filter_map(|(g, p)| g.parse::<u64>().ok().map(|id| (GuildId::new(id), p)))
            .collect();
        let stay_channels = db
            .stay_channels()
            .map_err(|e| format!("loading stay_channels: {e}"))?
            .into_iter()
            .filter_map(|(g, c)| {
                Some((GuildId::new(g.parse().ok()?), ChannelId::new(c.parse().ok()?)))
            })
            .collect();

        Ok(Arc::new(Self {
            registry: {
                let r = Registry::new();
                r.set_default_volume(cfg.audio.default_volume);
                r
            },
            http_api,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            prefixes: RwLock::new(prefixes),
            stay_channels: RwLock::new(stay_channels),
            db,
            voice,
            bot_id,
            cfg,
        }))
    }

    pub async fn prefix(&self, guild_id: Option<GuildId>) -> String {
        match guild_id {
            Some(g) => self.prefixes.read().await.get(&g).cloned(),
            None => None,
        }
        .unwrap_or_else(|| default_prefix(&self.cfg))
    }

    pub async fn set_prefix(&self, guild_id: GuildId, prefix: &str) -> Result<(), ()> {
        self.db
            .set_prefix(&guild_id.to_string(), prefix)
            .map_err(|_| ())?;
        self.prefixes
            .write()
            .await
            .insert(guild_id, prefix.to_string());
        Ok(())
    }

    pub async fn clear_stay_channel(&self, guild_id: GuildId) {
        let _ = self.db.delete_stay_channel(&guild_id.to_string());
        self.stay_channels.write().await.remove(&guild_id);
    }

    pub async fn set_stay_channel(&self, guild_id: GuildId, channel_id: ChannelId) {
        let _ = self
            .db
            .set_stay_channel(&guild_id.to_string(), &channel_id.to_string());
        self.stay_channels
            .write()
            .await
            .insert(guild_id, channel_id);
    }

    pub async fn stay_channel(&self, guild_id: GuildId) -> Option<ChannelId> {
        self.stay_channels.read().await.get(&guild_id).copied()
    }

    /// Snapshot of every configured 24/7 channel, used for boot rejoin.
    pub async fn stay_channels_snapshot(&self) -> Vec<(GuildId, ChannelId)> {
        self.stay_channels
            .read()
            .await
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect()
    }
}

impl Core {
    pub fn default_prefix_cfg(&self) -> String {
        crate::core::default_prefix(&self.cfg)
    }
}

mod commands;
mod config;
mod core;
mod db;
mod logger;
mod memory;
mod player;
mod settings;
mod sources;
mod state;
mod ui;

use std::sync::Arc;

use poise::serenity_prelude as serenity;

pub use crate::core::Core;

pub struct Data {
    pub core: Arc<Core>,
}

impl std::fmt::Debug for Data {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Data").finish_non_exhaustive()
    }
}

type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

/// Per-guild prefix lookup for the prefix framework. Falls back to the
/// configured default outside guilds / when unset.
fn dynamic_prefix(
    ctx: poise::PartialContext<'_, Data, Error>,
) -> poise::BoxFuture<'_, Result<Option<std::borrow::Cow<'static, str>>, Error>> {
    Box::pin(async move {
        let core = ctx.framework.user_data().core.clone();
        Ok(Some(std::borrow::Cow::Owned(core.prefix(ctx.guild_id).await)))
    })
}

fn init_logging(cfg: &settings::Config) {
    let level = match cfg.logging.level.as_str() {
        "warn" => logger::Level::Warn,
        "error" => logger::Level::Error,
        _ => logger::Level::Info,
    };
    logger::init(
        level,
        cfg.logging.file.enabled,
        &cfg.logging.file.path.clone(),
    );
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

const YTDLP_KNOWN_GOOD: &str = "2026.08.20";

struct Handler {
    core: Arc<Core>,
    commands: Arc<Vec<poise::Command<Data, Error>>>,
    registered: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl serenity::EventHandler for Handler {
    async fn dispatch(&self, ctx: &serenity::all::Context, event: &serenity::FullEvent) {
        use serenity::FullEvent::*;
        match event {
            Ready { data_about_bot, .. } => {
                log_started!(
                    "gateway",
                    "ready as {} ({})",
                    data_about_bot.user.name,
                    data_about_bot.user.id
                );
                // Idle + "Listening to +help | For Help" — re-asserted on every
                // Ready so reconnects never leave the bot stuck Online.
                ctx.set_presence(
                    Some(serenity::gateway::ActivityData::listening("+help | For Help")),
                    serenity::model::user::OnlineStatus::Idle,
                );
                if !self.registered.swap(
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                ) {
                    match std::env::var("DEV_GUILD_ID")
                        .ok()
                        .and_then(|g| g.parse::<u64>().ok())
                        .map(serenity::GuildId::new)
                    {
                        Some(gid) => {
                            match poise::builtins::register_in_guild(
                                &ctx.http,
                                self.commands.as_slice(),
                                gid,
                            )
                            .await
                            {
                                Ok(()) => log_started!("discord", "commands registered to dev guild {gid}"),
                                Err(e) => log_error!("discord", "guild registration failed: {e}"),
                            }
                        }
                        None => {
                            match poise::builtins::register_globally(
                                &ctx.http,
                                self.commands.as_slice(),
                            )
                            .await
                            {
                                Ok(()) => log_started!("discord", "commands registered globally"),
                                Err(e) => log_error!("discord", "global registration failed: {e}"),
                            }
                        }
                    }

                    // Restart-proof 24/7: come back to every stay channel
                    // automatically after boot/reconnect.
                    player::rejoin_stay_channels(&self.core).await;
                }
            }
            VoiceStateUpdate { old: Some(old), new, .. } => {
                player::handle_voice_state_update(ctx, &self.core, old, new).await;
            }
            InteractionCreate { interaction, .. } => {
                match interaction {
                    // Router precedence mirrors old index.js:320-348:
                    // report buttons/selects first, then music/help router;
                    // modals are always report.
                    serenity::model::application::Interaction::Component(comp) => {
                        let handled = commands::report::component(&self.core, comp).await;
                        if !handled {
                            let _ = ui::router::component(&self.core, ctx, comp).await;
                        }
                    }
                    serenity::model::application::Interaction::Modal(m) => {
                        let guild_name = m
                            .guild_id
                            .and_then(|g| ctx.cache.guild(g))
                            .map(|g| g.name.clone().into_string());
                        let _ = commands::report::modal(&self.core, guild_name, m).await;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

async fn check_token(token: &str) -> Result<(String, String), String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://discord.com/api/v10/users/@me")
        .header("Authorization", format!("Bot {token}"))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("token check request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("token rejected by Discord (HTTP {})", resp.status()));
    }
    let body: serde_json::Value =
        resp.json().await.map_err(|e| format!("bad token-check response: {e}"))?;
    let username = body["username"]
        .as_str()
        .ok_or("token-check response missing username")?
        .to_string();
    let id = body["id"].as_str().unwrap_or_default().to_string();
    Ok((id, username))
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = settings::load();
    if cfg.banner {
        logger::print_banner(env!("CARGO_PKG_VERSION"));
    }
    init_logging(&cfg);

    log_started!("bootstrap", "koaai v{} (rusqlite {})",
        env!("CARGO_PKG_VERSION"), rusqlite::version());

    sources::init(cfg.sources.clone());

    let token = std::env::var("TOKEN").expect("TOKEN must be set in env");

    match sources::probe_version().await {
        Ok(v) => {
            log_info!("selfcheck", "yt-dlp {v}");
            if v.as_str() < YTDLP_KNOWN_GOOD {
                log_warn!("selfcheck",
                    "yt-dlp {v} is older than the known-good {YTDLP_KNOWN_GOOD} nightly; YouTube may return 403 - update with `pip install --upgrade yt-dlp --pre`");
            }
        }
        Err(e) => {
            log_error!("selfcheck", "{e}");
            std::process::exit(1);
        }
    };
    // Data-dir sanity first: bot.db + config.toml live in the working directory
    // and are created on first boot (empty server dir is normal). Probe
    // writability up front so a read-only/missing CWD fails with a clear
    // message instead of a cryptic sqlite error later. (rusqlite opens lazily,
    // so without this the real OS error would be misattributed.)
    {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        match std::fs::write(".koaai_writetest", b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(".koaai_writetest");
                log_info!("selfcheck", "data dir {cwd} is writable");
            }
            Err(e) => {
                log_error!("selfcheck", "data dir {cwd} is NOT writable: {e} — the bot needs a writable working directory for bot.db/config.toml");
                std::process::exit(1);
            }
        }
    }
    let database = match db::Db::open("bot.db") {
        Ok(d) => d,
        Err(e) => {
            log_error!("selfcheck", "db open failed: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = database.checkpoint() {
        log_error!("selfcheck", "db checkpoint: {e}");
        std::process::exit(1);
    }

    let (bot_id_raw, bot_name) = match check_token(&token).await {
        Ok(v) => v,
        Err(e) => {
            log_error!("selfcheck", "{e}");
            std::process::exit(1);
        }
    };
    let bot_id = bot_id_raw.parse::<u64>().map_or_else(
        |_| serenity::model::id::UserId::new(0),
        serenity::model::id::UserId::new,
    );
    log_info!("selfcheck", "token valid, identity is @{bot_name}");
    log_started!("selfcheck", "all checks passed");

    let token_parsed =
        token.parse::<serenity::Token>().expect("invalid bot token format");
    let http = Arc::new(serenity::Http::new(token_parsed.clone()));

    let voice = songbird::Songbird::serenity();
    let core = Core::load(database, http.clone(), voice.clone(), bot_id, cfg)
        .await
        .unwrap_or_else(|e| {
            log_error!("boot", "loading core state: {e}");
            std::process::exit(1);
        });

    // Crash-safe SQLite: WAL checkpoint every hour so even a hard kill never
    // loses more than an hour of prefix/24-7/report writes.
    // Also keeps RSS flat: stale guilds are pruned every 10 min and
    // malloc_trim is called to return glibc freelists to the OS (prevents
    // the 9 MB -> 28 MB idle creep and the nonstop-loop stair-step).
    {
        let core2 = core.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(600));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut checkpoint_counter: u8 = 0;
            loop {
                tick.tick().await;
                // Drop state for guilds that are disconnected AND empty —
                // keeps long-running RSS flat no matter how many servers
                // the bot has ever touched.
                let mut pruned = 0usize;
                for g in core2.registry.guild_ids() {
                    let stale = {
                        // Don't use `get` that would insert phantom guilds; `guild_ids`
                        // guarantees existence, but be explicit.
                        let Some(st) = core2.registry.get_if_exists(g) else {
                            continue;
                        };
                        st.voice_channel_id.is_none()
                            && st.queue.is_empty()
                            && st.current.is_none()
                            && st.current_handle.is_none()
                    };
                    if stale {
                        core2.registry.remove(g);
                        pruned += 1;
                    }
                }
                if pruned > 0 {
                    // DashMap shards grow but never shrink on remove — reclaim bucket memory.
                    core2.registry.shrink_to_fit();
                    // Avoid log spam: only emit once per hour (or when burst >=10).
                    // The bot was spamming "pruned 3 stale guild(s)" every 10 min for days
                    // because VoiceStateUpdate was inserting phantom GuildStates for
                    // every voice event via `registry.get()`; that phantom bug is now
                    // fixed with `get_if_exists`, but throttle logging anyway.
                    if pruned >= 10 || checkpoint_counter % 6 == 5 {
                        log_info!(
                            "gc",
                            "pruned {pruned} stale guild(s) — {} guilds remain",
                            core2.registry.len()
                        );
                    } else {
                        tracing::debug!(pruned, remaining = core2.registry.len(), "gc pruned stale guilds");
                    }
                }
                // Shrink any overgrown VecDeque capacities and trim heap.
                // Even active guilds can retain 10+ MB of queue capacity after
                // a large playlist; shrinking on idle boundaries prevents that
                // from pinning RSS forever.
                for gid in core2.registry.guild_ids() {
                    if let Some(mut st) = core2.registry.get_if_exists(gid) {
                        if st.queue.capacity() > 32 && st.queue.len() < st.queue.capacity() / 2 {
                            st.queue.shrink_to_fit();
                        }
                    }
                }
                crate::memory::trim();

                checkpoint_counter = checkpoint_counter.wrapping_add(1);
                // Checkpoint roughly hourly (600s * 6 = 3600s)
                if checkpoint_counter % 6 == 0 {
                    if let Err(e) = core2.db.checkpoint() {
                        log_warn!("db", "hourly checkpoint failed: {e}");
                    }
                }
            }
        });
    }

    // Graceful shutdown on SIGINT/SIGTERM: flush DB, drop voice connections,
    // exit. No data loss, no zombie voice states.
    {
        let core2 = core.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term =
                signal(SignalKind::terminate()).expect("install SIGTERM handler");
            let mut intr =
                signal(SignalKind::interrupt()).expect("install SIGINT handler");
            tokio::select! {
                _ = term.recv() => {},
                _ = intr.recv() => {},
            }
            log_warn!("shutdown", "signal received — flushing state and leaving voice");

            let mut guilds: Vec<serenity::model::id::GuildId> =
                core2.stay_channels_snapshot().await.into_iter().map(|(g, _)| g).collect();
            for g in core2.registry.guild_ids() {
                if !guilds.contains(&g) {
                    guilds.push(g);
                }
            }
            for g in guilds {
                let _ = core2.voice.remove(g).await;
            }
            if let Err(e) = core2.db.checkpoint() {
                log_error!("shutdown", "final db checkpoint failed: {e}");
            } else {
                log_started!("shutdown", "database flushed");
            }
            std::process::exit(0);
        });
    }

    let all_commands = Arc::new(commands::all());
    let handler = Handler {
        core: core.clone(),
        commands: all_commands.clone(),
        registered: std::sync::atomic::AtomicBool::new(false),
    };

    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;

    let mut fopts = poise::FrameworkOptions::<Data, Error> {
        // NOTE: no static `prefix` here — dynamic_prefix alone decides
        // (guild prefix when set, otherwise the default). A static fallback
        // would let BOTH prefixes work after /setprefix.
        prefix_options: poise::PrefixFrameworkOptions {
            dynamic_prefix: Some(dynamic_prefix),
            ..Default::default()
        },
        on_error: |error| {
            Box::pin(async move {
                // Users chatting with the prefix on ("- kabhi kabhi") are not
                // errors — ignore unknown commands silently like the old bot.
match &error {
                    poise::FrameworkError::UnknownCommand { .. } => {
                        // Users chatting with the prefix on are not errors.
                        return;
                    }
                    poise::FrameworkError::ArgumentParse { ctx, .. } => {
                        let comps = crate::ui::error_container(&format!(
                            "Missing or invalid arguments for `{}`.\\n-# Check `/help {}` for usage.",
                            ctx.command().qualified_name,
                            ctx.command().qualified_name,
                        ));
                        let _ = ctx
                            .send(
                                poise::CreateReply::default()
                                    .flags(crate::ui::CV2_EPHEMERAL)
                                    .components(comps),
                            )
                            .await;
                        return;
                    }
                    poise::FrameworkError::Command { ctx, error, .. } => {
                        // Expected, user-facing rejections (locked to 24/7,
                        // not in VC, etc.) are already replied to by the
                        // command itself — log them quietly, not as errors.
                        // NOTE: log Debug ({error:?}), not Display ({error}).
                        // Display for Command is always just "error in command
                        // `/x`" and hides the inner cause (e.g. Unknown
                        // interaction vs 400 Invalid Form Body).
                        let name = &ctx.command().qualified_name;
                        log_info!("commands", "{name} returned an error: {error:?}");
                        return;
                    }
                    _ => {}
                }
                log_error!("commands", "{error:?}");
            })
        },
        ..Default::default()
    };
    fopts.commands = commands::all();
    let framework = poise::FrameworkBuilder::<Data, Error>::default()
        .options(fopts)
        .build();

    let data = Arc::new(Data { core });

    let mut client = serenity::Client::builder(token_parsed, intents)
        .framework(Box::new(framework))
        .data(data)
        .voice_manager(voice)
        .event_handler(Arc::new(handler))
        .await
        .expect("client creation failed");

    if let Err(e) = client.start().await {
        log_error!("discord", "client error: {e}");
        std::process::exit(1);
    }
}
use std::time::Instant;

use crate::config;
use crate::player;
use crate::{Context, Error};

async fn send_cv2(
    ctx: Context<'_>,
    comps: crate::ui::Comps,
) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .flags(crate::ui::CV2_FLAGS)
            .components(comps),
    )
    .await?;
    Ok(())
}

async fn send_cv2_ephemeral(
    ctx: Context<'_>,
    comps: crate::ui::Comps,
) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .flags(crate::ui::CV2_EPHEMERAL)
            .components(comps),
    )
    .await?;
    Ok(())
}

fn started_at() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}

/// Check the bot's API latency and voice connection status
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let core = ctx.data().core.clone();
    let connected = core.voice.get(ctx.guild_id().unwrap()).is_some();
    let status = if connected {
        config::emojis::ONLINE
    } else {
        config::emojis::OFFLINE
    };

    let api_ms = measure_api_latency(&core.http_api).await;

    send_cv2_ephemeral(
        ctx,
        crate::ui::info_container(format!(
            "## {} Pong!\n-# API latency: `{api_ms}` • Voice: {status}\n-# Slash commands respond instantly; prefix commands too.",
            config::emojis::PING
        )),
    )
    .await
}

async fn measure_api_latency(http: &serenity::http::Http) -> String {
    let start = std::time::Instant::now();
    match http.get_current_user().await {
        Ok(_) => format!("{}ms", start.elapsed().as_millis()),
        Err(_) => "?".into(),
    }
}

/// Lock the bot to your voice channel with 24/7 mode (no auto-leave)
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn join(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let user_vc = ctx.guild().and_then(|g| {
        g.voice_states
            .get(&ctx.author().id)
            .and_then(|vs| vs.channel_id)
    });
    let Some(user_vc) = user_vc else {
        let comps = crate::ui::error_container("Join a VC first!");
        return send_cv2_ephemeral(ctx, comps).await;
    };

    if let Some(t) = core.registry.get(guild_id).inactivity_task.take() {
        t.abort();
    }
    if let Some(t) = core.registry.get(guild_id).stay_return_task.take() {
        t.abort();
    }

    player::ensure_voice(&core, guild_id, user_vc, false)
        .await
        .map_or_else(|e| Err(Error::from(e)), |_| Ok(()))?;

    core.registry.get(guild_id).home_channel =
        Some(serenity::all::ChannelId::new(ctx.channel_id().get()));
    core.set_stay_channel(guild_id, user_vc).await;

    let comps = crate::ui::success_container(&format!(
        "24/7 Mode Enabled in <#{user_vc}>\n-# I'll stay here even when idle — no auto-leave, ever. `+leave` releases me."
    ));
    send_cv2_ephemeral(ctx, comps).await
}

/// Disable 24/7 mode and leave the voice channel
#[poise::command(slash_command, prefix_command, aliases("disconnect", "dc"), guild_only)]
pub async fn leave(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    core.clear_stay_channel(guild_id).await;
    {
        let mut st = core.registry.get(guild_id);
        st.request_stop();
        st.queue.clear();
        st.current = None;
        st.previous = None;
        st.playing = false;
        st.loop_mode = crate::state::LoopMode::Off;
        if let Some(t) = st.inactivity_task.take() {
            t.abort();
        }
        if let Some(t) = st.stay_return_task.take() {
            t.abort();
        }
    }
    let _ = core.voice.remove(guild_id).await;
    core.registry.get(guild_id).voice_channel_id = None;

    let comps = crate::ui::info_container(format!(
        "{}  Left and disabled 24/7 mode.\n-# From now on I auto-leave after 5 idle minutes. Re-enable anytime with `+join`.",
        config::emojis::WAVE
    ));
    send_cv2_ephemeral(ctx, comps).await
}

/// Change this server's command prefix (requires administrator)
#[poise::command(slash_command, prefix_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn setprefix(
    ctx: Context<'_>,
    #[description = "New prefix"]
    #[rest]
    new_prefix: String,
) -> Result<(), Error> {
    let core = ctx.data().core.clone();
    let guild_id = ctx.guild_id().unwrap();

    let new_prefix = new_prefix.trim();
    if new_prefix.is_empty() || new_prefix.len() > 8 {
        let comps = crate::ui::error_container("Provide a short prefix (1-8 chars).");
        return send_cv2(ctx, comps).await;
    }

    match core.set_prefix(guild_id, new_prefix).await {
        Ok(()) => {
            let comps = crate::ui::success_container(&format!(
                "Prefix set to `{new_prefix}`\n-# Try it now: `{new_prefix}play <song>` — works instantly, saved for this server."
            ));
            send_cv2_ephemeral(ctx, comps).await?;
        }
        Err(_) => {
            let comps = crate::ui::error_container("Failed to save prefix.");
            send_cv2_ephemeral(ctx, comps).await?;
        }
    }
    Ok(())
}

/// Show how long the bot has been running
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn uptime(ctx: Context<'_>) -> Result<(), Error> {
    let s = started_at().elapsed().as_secs();
    let d = s / 86400;
    let h = (s % 86400) / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;

    send_cv2_ephemeral(
        ctx,
        crate::ui::info_container(format!(
            "## {} Uptime\n-# Running for: `{d}d {h}h {m}m {sec}s`",
            config::emojis::ROCKET
        )),
    )
    .await
}

/// Browse every command, grouped by category
#[poise::command(rename = "help", slash_command, prefix_command, guild_only)]
pub async fn help_cmd(
    ctx: Context<'_>,
    #[description = "Category to browse"] category: Option<String>,
) -> Result<(), Error> {
    let core = ctx.data().core.clone();
    let guild_id = ctx.guild_id();
    let prefix = core.prefix(guild_id).await;
    let bot_name = ctx
        .guild()
        .and_then(|g| g.members.get(&core.bot_id).map(|m| m.display_name().to_string()))
        .unwrap_or_else(|| "Koaai".into());

    let comps = crate::ui::help_components(
        guild_id.map(|g| g.get()).unwrap_or(0),
        &prefix,
        category.as_deref(),
        &bot_name,
        22,
    );

    ctx.send(
        poise::CreateReply::default()
            .flags(crate::ui::CV2_FLAGS)
            .components(comps),
    )
    .await?;
    Ok(())
}

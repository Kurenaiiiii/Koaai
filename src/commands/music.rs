use std::time::Duration;


use crate::config;
use crate::memory;
use crate::player;
use crate::state::{LoopMode, Track};
use crate::{sources, Context, Error};

fn emoji_err(msg: &str) -> String {
    format!("{}  {msg}", config::emojis::ERROR)
}

/// Containers already paint a status emoji; messages that start with their
/// own action emoji would render two in a row. Strip the leading one.
fn dedup_emoji(msg: &str) -> &str {
    let t = msg.trim_start();
    if t.starts_with('<')
        && let Some((_, rest)) = t.split_once('>')
    {
        return rest.trim_start();
    }
    msg
}

async fn ok(ctx: Context<'_>, msg: &str) -> Result<(), Error> {
    let comps = crate::ui::success_container(dedup_emoji(msg));
    ctx.send(
        poise::CreateReply::default()
            .flags(crate::ui::CV2_EPHEMERAL)
            .components(comps),
    )
    .await?;
    Ok(())
}

async fn err(ctx: Context<'_>, msg: &str) -> Result<(), Error> {
    let comps = crate::ui::error_container(dedup_emoji(msg));
    ctx.send(
        poise::CreateReply::default()
            .flags(crate::ui::CV2_EPHEMERAL)
            .components(comps),
    )
    .await?;
    Ok(())
}

async fn requester_channel(ctx: &Context<'_>) -> Option<serenity::model::id::ChannelId> {
    let guild = ctx.guild()?;
    guild
        .voice_states
        .get(&ctx.author().id)
        .and_then(|vs| vs.channel_id)
}
async fn vc_guard(ctx: &Context<'_>) -> Result<serenity::model::id::ChannelId, Error> {
    let core = &ctx.data().core;
    let guild_id = ctx.guild_id().expect("guild only");
    let Some(bot_vc) = core.registry.get(guild_id).voice_channel_id else {
        return Err(Error::from(emoji_err("Not connected.")));
    };
    match requester_channel(ctx).await {
        Some(c) if c == bot_vc => Ok(c),
        _ => Err(Error::from(emoji_err("Join my voice channel!"))),
    }
}

fn parse_timestamp(input: &str) -> Option<u64> {
    if let Some((m, s)) = input.split_once(':') {
        let m: u64 = m.trim().parse().ok()?;
        let s: u64 = s.trim().parse().ok()?;
        Some(m * 60 + s)
    } else {
        input.trim().parse().ok()
    }
}

/// Play a song or playlist from a name, YouTube, Spotify or SoundCloud link
#[poise::command(slash_command, prefix_command, aliases("p"), guild_only)]
pub async fn play(
    ctx: Context<'_>,
    #[description = "Song name or URL"]
    #[rest]
    search: String,
) -> Result<(), Error> {
    let query = search;
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let Some(user_vc) = requester_channel(&ctx).await else {
        return err(ctx, "You need to be in a voice channel!").await;
    };

    let old_humans = {
        let old_vc = core.registry.get(guild_id).voice_channel_id;
        match (old_vc, ctx.guild()) {
            (Some(ov), Some(g)) => {
                ov != user_vc
                    && g.voice_states
                        .iter()
                        .any(|vs| vs.channel_id == Some(ov) && vs.user_id != core.bot_id)
            }
            _ => false,
        }
    };

    if let Err(e) = player::ensure_voice(&core, guild_id, user_vc, old_humans).await {
        // e.g. "I'm locked to 24/7 mode in <#...>" — the user must SEE this.
        return err(ctx, &e).await;
    }

    core.registry.get(guild_id).home_channel =
        Some(serenity::all::ChannelId::new(ctx.channel_id().get()));
    if let Some(t) = core.registry.get(guild_id).inactivity_task.take() {
        t.abort();
    }

    let is_playlist = sources::is_playlist_query(&query);
    let loading = crate::ui::info_container(format!(
        "{}  {}\n-# {}",
        config::emojis::LOADING,
        if is_playlist { "Loading playlist..." } else { "Searching for your track..." },
        if is_playlist {
            "Big playlists can take a moment — tracks stream in one by one."
        } else {
            "Tip: paste a YouTube / Spotify / SoundCloud link, or use `ytsearch:` `spsearch:` `scsearch:` prefixes."
        }
    ));
    let _status = ctx
        .send(poise::CreateReply::default().flags(crate::ui::CV2_FLAGS).components(loading))
        .await?;

    if sources::is_playlist_query(&query) {
        match sources::resolve_playlist(&query).await {
            Ok(metas) => {
                let count = metas.len();
                {
                    let mut st = core.registry.get(guild_id);
                    for m in metas {
                        st.queue
                            .push_back(Track::from_resolved(m.into(), ctx.author().name.clone()));
                    }
                }
                let comps = crate::ui::info_container(format!(
                    "## 📋 Playlist Queued\n**Tracks:** {count} • **Queued by:** <@{}>\n-# Tracks start playing automatically — type `+queue` to browse what's coming.",
                    ctx.author().id
                ));
                ctx.send(
                    poise::CreateReply::default()
                        .flags(crate::ui::CV2_FLAGS)
                        .components(comps),
                )
                .await?;
            }
            Err(e) => return err(ctx, &e).await,
        }
    } else {
        match sources::resolve(&query).await {
            Ok(meta) => {
                let track = Track::from_resolved(meta.into(), ctx.author().name.clone());
                let was_idle = !core.registry.get(guild_id).playing;
                let position = {
                    let mut st = core.registry.get(guild_id);
                    st.queue.push_back(track.clone());
                    st.queue.len()
                };
                let body = if was_idle {
                    format!(
                        "## {} Now starting\n**[{}]({})**\n-# Artist: **{}** • Duration: `{}`\n-# Control playback with the buttons under the NP card.",
                        config::emojis::PLAY,
                        track.title.replace('[', "(").replace(']', ")"),
                        track.link_for_ui(),
                        track.author,
                        track.duration_display()
                    )
                } else {
                    format!(
                        "## {} Added to Queue\n**[{}]({})**\n-# Artist: **{}** • Duration: `{}` • Position: `#{}`\n-# It plays automatically when the current track ends.",
                        player_source_emoji(&track),
                        track.title.replace('[', "(").replace(']', ")"),
                        track.link_for_ui(),
                        track.author,
                        track.duration_display(),
                        position
                    )
                };
                let comps = crate::ui::info_container(body);
                ctx.send(
                    poise::CreateReply::default()
                        .flags(crate::ui::CV2_FLAGS)
                        .components(comps),
                )
                .await?;
            }
            Err(e) => return err(ctx, &e).await,
        }
    }

    // Release large temp buffers (yt-dlp stdout, search JSON) back to OS.
    memory::trim();

    if !core.registry.get(guild_id).playing {
        tokio::spawn(async move {
            player::play_next(core, guild_id).await;
        });
    }
    Ok(())
}

fn player_source_emoji(t: &Track) -> &'static str {
    use crate::state::SourceTag::*;
    // Apple Music searches are matched onto YouTube but deserve their own badge
    if t.ui_link
        .as_deref()
        .is_some_and(|u| u.contains("music.apple.com"))
    {
        return config::emojis::APPLEMUSIC;
    }
    match t.source {
        Youtube => config::emojis::YOUTUBE,
        SoundCloud => config::emojis::SOUNDCLOUD,
        SpotifyMatched => config::emojis::SPOTIFY,
        Discord | File => config::emojis::FOLDER,
    }
}

/// Skip the current track, or jump ahead several tracks at once
#[poise::command(slash_command, prefix_command, aliases("s"), guild_only)]
pub async fn skip(
    ctx: Context<'_>,
    #[description = "Number of tracks to skip"] amount: Option<u32>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();
    vc_guard(&ctx).await?;

    let amount = amount.unwrap_or(1).max(1);
    let has_current = {
        let st = core.registry.get(guild_id);
        st.current.is_some() || st.playing
    };
    if !has_current {
        return err(ctx, "Nothing is playing.").await;
    }

    let handle = {
        let mut st = core.registry.get(guild_id);
        for _ in 0..amount.saturating_sub(1) {
            st.queue.pop_front();
        }
        if st.queue.is_empty() {
            st.queue.shrink_to_fit();
        }
        st.request_stop();
        st.playing = false;
        st.previous = st.current.take();
        st.current_is_cached = false;
        st.current_handle.take()
    };
    if let Some(h) = handle {
        let _ = h.stop();
    }
    memory::trim();
    let core2 = core.clone();
    tokio::spawn(async move { player::play_next(core2, guild_id).await });
    ok(
        ctx,
        &format!(
            "{}  Skipped **{amount}** track(s).\n-# Next track starts instantly — no gap.",
            config::emojis::SKIP
        ),
    )
    .await
}

/// Stop playback, clear the queue and leave voice (unless 24/7 mode)
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();
    vc_guard(&ctx).await?;

    let handle = {
        let mut st = core.registry.get(guild_id);
        st.request_stop();
        st.queue.clear();
        st.queue.shrink_to_fit();
        st.loop_mode = LoopMode::Off;
        st.playing = false;
        st.previous = None;
        st.current = None;
        st.current_is_cached = false;
        st.current_handle.take()
    };
    if let Some(h) = handle {
        let _ = h.stop();
    }
    memory::trim();

    if core.stay_channel(guild_id).await.is_some() {
        ok(
            ctx,
            &format!(
                "{} Stopped. 24/7 mode is active, so I'm staying in voice.\n-# Use `+leave` to release me.",
                config::emojis::STOP
            ),
        )
        .await
    } else {
        let _ = core.voice.remove(guild_id).await;
        {
            let mut st = core.registry.get(guild_id);
            st.voice_channel_id = None;
            if let Some(t) = st.inactivity_task.take() {
                t.abort();
            }
            if let Some(t) = st.stay_return_task.take() {
                t.abort();
            }
        }
        memory::trim();
        ok(
            ctx,
            &format!(
                "{} Stopped and left the channel.\n-# Queue cleared • Loop reset to off.",
                config::emojis::STOP
            ),
        )
        .await
    }
}

/// Pause the currently playing track
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn pause(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let handle = core.registry.get(guild_id).current_handle.clone();
    match handle {
        Some(h) => match h.pause() {
            Ok(()) => ok(
                ctx,
                &format!(
                    "{}  Paused.\n-# Resume with `+resume` or the play button on the NP card.",
                    config::emojis::PAUSE
                ),
            )
            .await,
            Err(_) => err(ctx, "Failed to pause.").await,
        },
        None => err(ctx, "Nothing is playing.").await,
    }
}

/// Resume the paused track
#[poise::command(slash_command, prefix_command, aliases("unpause"), guild_only)]
pub async fn resume(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let handle = core.registry.get(guild_id).current_handle.clone();
    match handle {
        Some(h) => {
            let paused = h
                .get_info()
                .await
                .map(|i| matches!(i.playing, songbird::tracks::PlayMode::Pause))
                .unwrap_or(false);
            if !paused {
                return err(ctx, "Not paused.").await;
            }
            match h.play() {
                Ok(()) => ok(
                    ctx,
                    &format!("{}  Resumed — right where you left off.\n-# Tip: `+seek <time>` jumps anywhere in the track.", config::emojis::PLAY),
                )
                .await,
                Err(_) => err(ctx, "Failed to resume.").await,
            }
        }
        None => err(ctx, "Nothing is playing.").await,
    }
}

/// Jump to a timestamp in the current track (e.g. 1:30 or 90)
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn seek(
    ctx: Context<'_>,
    #[description = "Timestamp like 1:30 or seconds like 90"] position: String,
) -> Result<(), Error> {
    seek_common(ctx, parse_timestamp(&position), false).await
}

/// Skip forward in the current track (default 15 seconds)
#[poise::command(slash_command, prefix_command, aliases("ff"), guild_only)]
pub async fn forward(
    ctx: Context<'_>,
    #[description = "Seconds to jump forward (default 15)"] seconds: Option<i64>,
) -> Result<(), Error> {
    seek_delta(ctx, seconds.unwrap_or(15).max(0) as u64, true).await
}

/// Skip backward in the current track (default 15 seconds)
#[poise::command(slash_command, prefix_command, aliases("rw"), guild_only)]
pub async fn rewind(
    ctx: Context<'_>,
    #[description = "Seconds to jump back (default 15)"] seconds: Option<i64>,
) -> Result<(), Error> {
    seek_delta(ctx, seconds.unwrap_or(15).max(0) as u64, false).await
}

async fn seek_common(ctx: Context<'_>, target: Option<u64>, _unused: bool) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let Some(secs) = target else {
        return err(ctx, "Invalid format. Use `1:30` or `90`.").await;
    };
    let dur = core.registry.get(guild_id).current.as_ref().and_then(|t| t.duration_secs);
    if core.registry.get(guild_id).current_handle.is_none() {
        return err(ctx, "Nothing is playing.").await;
    }
    let clamped = match dur {
        Some(d) => secs.min(d),
        None => secs,
    };
    perform_seek(ctx, clamped, None).await
}

async fn seek_delta(ctx: Context<'_>, secs: u64, forward: bool) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let (handle, duration) = {
        let st = core.registry.get(guild_id);
        (
            st.current_handle.clone(),
            st.current.as_ref().and_then(|t| t.duration_secs),
        )
    };
    let Some(handle) = handle else {
        return err(ctx, "Nothing is playing.").await;
    };
    let pos = handle
        .get_info()
        .await
        .ok()
        .map(|i| i.play_time.as_secs())
        .unwrap_or(0);

    let target = if forward {
        (pos + secs).min(duration.unwrap_or(u64::MAX).saturating_sub(1))
    } else {
        pos.saturating_sub(secs)
    };

    perform_seek(ctx, target, Some((secs, forward))).await
}

/// Seeks are structurally unreliable on HTTP streams (songbird marks them
/// non-seekable and a failed native seek kills the track). First seek on a
/// track re-buffers it into a seekable Opus cache; later ones are instant.
async fn perform_seek(
    ctx: Context<'_>,
    target: u64,
    delta: Option<(u64, bool)>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    // Slash interactions expire after 3 seconds; a first-seek re-buffer can
    // exceed that, so ack immediately (no-op for prefix invocations).
    ctx.defer_ephemeral().await?;

    let already_cached = core.registry.get(guild_id).current_is_cached;

    if already_cached {
        // Cache is seekable — native seek is instant and safe.
        if let Some(h) = core.registry.get(guild_id).current_handle.clone() {
            match h.seek_async(Duration::from_secs(target)).await {
                Ok(_) => return seek_reply(ctx, target, delta, false).await,
                Err(_) => {
                    // cache somehow unusable — fall through to re-buffer
                }
            }
        }
    }

    match player::restart_current_at(&core, guild_id, target).await {
        Ok(()) => seek_reply(ctx, target, delta, !already_cached).await,
        Err(_) => err(
            ctx,
            "Couldn't jump there — the stream refused to reposition. Try `+rewind`/`+forward` instead.",
        )
        .await,
    }
}

async fn seek_reply(
    ctx: Context<'_>,
    target: u64,
    delta: Option<(u64, bool)>,
    rebuffered: bool,
) -> Result<(), Error> {
    let mut note = String::new();
    let (emoji, lead) = match delta {
        Some((secs, forward)) => (
            if forward { config::emojis::FORWARD } else { config::emojis::REWIND },
            format!("Jumped {} `{secs}s`", if forward { "forward" } else { "back" }),
        ),
        None => (
            config::emojis::FORWARD,
            format!("Jumped to `{}`", crate::state::fmt_sec(target)),
        ),
    };
    if let Some((_, _)) = delta {
        note.push_str("\n-# Formats: `90` seconds or `1:30` / `1:02:30`. Defaults to 15s.");
    }
    if rebuffered {
        note.push_str("\n-# First seek re-buffered this track — every next seek is instant. 🎧");
    }
    ok(ctx, &format!("{emoji}  {lead}.{note}")).await
}

/// Set the playback volume (0-200%)
#[poise::command(slash_command, prefix_command, aliases("vol"), guild_only)]
pub async fn volume(
    ctx: Context<'_>,
    #[description = "Volume level 0-200"] vol: u16,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let max = core.cfg.audio.max_volume;
    if vol > max {
        return err(ctx, &format!("Volume must be 0–{max}.")).await;
    }
    core.registry.get(guild_id).volume = vol;
    if let Some(h) = core.registry.get(guild_id).current_handle.clone() {
        let _ = h.set_volume(f32::from(vol) / 100.0);
    }
    let passthrough_note = if vol == 100 {
        "100% = pure source quality (zero re-encoding)."
    } else {
        "Tip: 100% gives the cleanest audio — other values remix the stream."
    };
    ok(
        ctx,
        &format!(
            "{}  Volume → **{vol}%**\n-# {passthrough_note}",
            config::emojis::VOLUME
        ),
    )
    .await
}

/// Cycle loop mode: off -> single track -> entire queue
#[poise::command(rename = "loop", slash_command, prefix_command, aliases("repeat"), guild_only)]
pub async fn loop_mode(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let label = {
        let mut st = core.registry.get(guild_id);
        st.loop_mode = st.loop_mode.cycle();
        st.loop_mode.label()
    };
    ok(
        ctx,
        &format!(
            "{} Loop → **{label}**\n-# Cycles: off → single track → entire queue.",
            config::emojis::LOOP
        ),
    )
    .await
}

/// Shuffle all tracks in the queue
#[poise::command(slash_command, prefix_command, guild_only)]
pub async fn shuffle(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let mut st = core.registry.get(guild_id);
    if st.queue.is_empty() {
        return err(ctx, "Queue is empty.").await;
    }

    let len = st.queue.len();
    for i in (1..len).rev() {
        let j = rand_index(i + 1);
        st.queue.swap(i, j);
    }
    drop(st);
    ok(
        ctx,
        &format!(
            "{}  Queue shuffled!\n-# `+queue` shows the new order — shuffle again anytime.",
            config::emojis::SHUFFLE
        ),
    )
    .await
}

fn rand_index(bound: usize) -> usize {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    std::hint::black_box(&mut h);
    (h.finish() % bound as u64) as usize
}

/// Remove every track from the queue
#[poise::command(rename = "clear", slash_command, prefix_command, aliases("clearqueue"), guild_only)]
pub async fn clear_queue(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    {
        let mut st = core.registry.get(guild_id);
        st.queue.clear();
        st.queue.shrink_to_fit();
    }
    memory::trim();
    ok(
        ctx,
        &format!(
            "{}  Queue cleared.\n-# The current track keeps playing to the end.",
            config::emojis::CLEAR
        ),
    )
    .await
}

/// Remove a track from the queue by its position
#[poise::command(slash_command, prefix_command, aliases("rm"), guild_only)]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "Position in queue (1-based)"] pos: u32,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let removed = {
        let mut st = core.registry.get(guild_id);
        if pos == 0 || pos as usize > st.queue.len() {
            return err(ctx, "Invalid position.").await;
        }
        let r = st.queue.remove(pos as usize - 1).expect("bounds checked above");
        if st.queue.is_empty() {
            st.queue.shrink_to_fit();
        }
        r
    };
    if core.registry.get(guild_id).queue.is_empty() {
        memory::trim();
    }
    ok(
        ctx,
        &format!(
            "{}  Removed **{}** from the queue.\n-# Positions shift up — `+queue` shows the updated list.",
            config::emojis::CLEAR,
            removed.title
        ),
    )
    .await
}

/// Move a queued track to a different position
#[poise::command(rename = "move", slash_command, prefix_command, aliases("mv"), guild_only)]
pub async fn move_track(
    ctx: Context<'_>,
    #[description = "Current position"] from_pos: u32,
    #[description = "New position"] to_pos: u32,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let title = {
        let mut st = core.registry.get(guild_id);
        let n = st.queue.len();
        if from_pos == 0 || from_pos as usize > n || to_pos == 0 || to_pos as usize > n {
            return err(ctx, "Invalid positions.").await;
        }
        let track = st.queue.remove(from_pos as usize - 1).unwrap();
        st.queue.insert(to_pos as usize - 1, track.clone());
        track.title
    };
    ok(
        ctx,
        &format!(
            "{}  Moved **{title}** to position {to_pos}.\n-# It'll play in that spot — check `+queue` for the order.",
            config::emojis::SUCCESS
        ),
    )
    .await
}

/// Show the current queue with page navigation buttons
#[poise::command(slash_command, prefix_command, aliases("q"), guild_only)]
pub async fn queue(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    {
        let st = core.registry.get(guild_id);
        if st.current.is_none() && st.queue.is_empty() {
            return err(ctx, "Not connected.").await;
        }
        let comps =
            crate::ui::queue_components(guild_id.get(), &st, 0, core.cfg.bot.items_per_page);
        drop(st);

        ctx.send(
            poise::CreateReply::default()
                .flags(crate::ui::CV2_FLAGS)
                .components(comps),
        )
        .await?;
    }
    Ok(())
}

/// Show detailed info about the track that is playing right now
#[poise::command(slash_command, prefix_command, aliases("np"), guild_only)]
pub async fn nowplaying(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().expect("guild only");
    let core = ctx.data().core.clone();

    let st = core.registry.get(guild_id);
    match st.current.clone() {
        Some(t) => {
            let comps = crate::ui::now_playing_components(guild_id.get(), &st, &t);
            drop(st);
            ctx.send(
                poise::CreateReply::default()
                    .flags(crate::ui::CV2_FLAGS)
                    .components(comps),
            )
            .await?;
            Ok(())
        }
        None => err(ctx, "Nothing is playing.").await,
    }
}

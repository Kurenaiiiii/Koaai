use std::sync::Arc;
use std::time::Duration;

use serenity::builder::EditChannel;
use serenity::model::id::{ChannelId, GuildId};
use serenity::model::voice::VoiceState;
use songbird::events::{Event, EventContext, EventHandler as VoiceEventHandler, TrackEvent};
use songbird::input::{Compose, YoutubeDl};
use songbird::Call;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::core::Core;
use crate::log_info;
use crate::log_started;
use crate::state::{GuildState, LoopMode, Track};
use crate::config;

pub type CallHandle = Arc<Mutex<Call>>;


fn yt_input(core: &Core, url: String) -> YoutubeDl<'static> {
    YoutubeDl::new(core.http_client.clone(), url)
        .user_args(vec!["--js-runtimes".into(), "node".into()])
}

async fn say_to(
    core: &Core,
    channel: ChannelId,
    text: String,
) -> Option<serenity::model::channel::Message> {
    // Deliberately NO accent colour — plain notification bubble.
    let comps: Vec<serenity::builder::CreateComponent<'static>> =
        vec![serenity::builder::CreateComponent::Container(
            serenity::builder::CreateContainer::new(vec![
                serenity::builder::CreateContainerComponent::TextDisplay(
                    serenity::builder::CreateTextDisplay::new(text).into_owned(),
                ),
            ])
            .into_owned(),
        )];
    match serenity::all::GenericChannelId::new(channel.get())
        .send_message(
            &core.http_api,
            serenity::builder::CreateMessage::new()
                .flags(crate::ui::CV2_FLAGS)
                .components(comps),
        )
        .await {
        Ok(m) => Some(m),
        Err(e) => {
            warn!(error = %e, %channel, "message send failed");
            None
        }
    }
}



pub struct GuildTrackEvents {
    pub core: Arc<Core>,
    pub guild_id: GuildId,
    pub kind: EventKind,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EventKind {
    End,
    Error,
}

#[async_trait::async_trait]
impl VoiceEventHandler for GuildTrackEvents {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let EventContext::Track(_) = ctx else {
            return None;
        };
        match self.kind {
            EventKind::End => {
                let mut st = self.core.registry.get(self.guild_id);
                if st.take_fresh_stop() {
                    info!(guild = %self.guild_id, "end after intentional stop; command owns flow");
                    return None;
                }
                drop(st);
                play_next(self.core.clone(), self.guild_id).await;
            }
            EventKind::Error => {
                let mut st = self.core.registry.get(self.guild_id);
                if st.take_fresh_stop() {
                    return None;
                }
                st.playing = false;
                st.error_streak = st.error_streak.saturating_add(1);
                let streak = st.error_streak;
                let already_recovering = st.recovering;
                let home = st.home_channel;
                drop(st);

                error!(guild = %self.guild_id, streak, "track error");

                if streak >= self.core.cfg.audio.max_consecutive_errors {
                    warn!(guild = %self.guild_id, "too many consecutive failures; stopping");
                    let mut st = self.core.registry.get(self.guild_id);
                    st.queue.clear();
                    st.loop_mode = LoopMode::Off;
                    let home2 = st.home_channel;
                    drop(st);
                    if let Some(home2) = home2 {
                        say_to(
                            &self.core,
                            home2,
                            format!(
                                "{}  Too many consecutive track failures — stopped playback and cleared the queue.\n-# Check yt-dlp: `pip install --upgrade yt-dlp --pre`, then play again.",
                                config::emojis::ERROR
                            ),
                        )
                        .await;
                    }
                    return None;
                }

                // Dead-player recovery (port of music.js fix): several errors in a
                // row while still "connected" means something is systemically broken.
                // Force a real voice rejoin once and retry the failed track instead
                // of burning the whole queue.
                if streak >= self.core.cfg.audio.error_streak_rejoin_at && !already_recovering {
                    let mut st = self.core.registry.get(self.guild_id);
                    st.recovering = true;
                    let vc = st.voice_channel_id;
                    let home2 = st.home_channel;
                    drop(st);

                    warn!(guild = %self.guild_id, "attempting voice rejoin recovery");
                    if let Some(home2) = home2 {
                        say_to(
                            &self.core,
                            home2,
                            format!(
                                "{}  Playback kept failing — reconnecting voice and retrying this track...\n-# Automatic recovery, no action needed.",
                                config::emojis::SYNC
                            ),
                        )
                        .await;
                    }

                    let _ = self.core.voice.remove(self.guild_id).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    if let Some(vc) = vc {
                        match self.core.voice.join(self.guild_id, vc).await {
                            Ok(call) => {
                                {
                                    let mut handler = call.lock().await;
                                    let _ = handler.deafen(true).await;
                                    apply_bitrate(&mut handler, &self.core);
                                }
                                attach_track_events(&call, &self.core, self.guild_id).await;
                                self.core.registry.get(self.guild_id).voice_channel_id = Some(vc);
                                self.core.registry.get(self.guild_id).recovering = false;
                                // retry the track that just failed
                                let mut st = self.core.registry.get(self.guild_id);
                                if let Some(failed) = st.current.take() {
                                    st.queue.push_front(failed);
                                }
                                drop(st);
                                play_next(self.core.clone(), self.guild_id).await;
                                return None;
                            }
                            Err(e) => {
                                error!(guild = %self.guild_id, error = %e, "recovery rejoin failed");
                                let mut st = self.core.registry.get(self.guild_id);
                                st.recovering = false;
                                st.queue.clear();
                                st.playing = false;
                                st.current = None;
                                st.voice_channel_id = None;
                                drop(st);
                                if let Some(home2) = home {
                                    say_to(
                                        &self.core,
                                        home2,
                                        format!(
                                            "{}  Could not recover the voice connection — playback stopped.\n-# Try `join` and play again; if it persists, check yt-dlp.",
                                            config::emojis::ERROR
                                        ),
                                    )
                                    .await;
                                }
                                return None;
                            }
                        }
                    }
                }

                if let Some(home) = home {
                    say_to(
                        &self.core,
                        home,
                        format!(
                            "{}  Track failed to play — skipping to the next one.\n-# {}/{} consecutive errors before automatic recovery kicks in.",
                            config::emojis::WARN,
                            streak + 1,
                            self.core.cfg.audio.error_streak_rejoin_at
                        ),
                    )
                    .await;
                }
                play_next(self.core.clone(), self.guild_id).await;
            }
        }
        None
    }
}

pub async fn attach_track_events(call: &CallHandle, core: &Arc<Core>, guild_id: GuildId) {
    let mut handler = call.lock().await;
    handler.add_global_event(
        TrackEvent::End.into(),
        GuildTrackEvents {
            core: core.clone(),
            guild_id,
            kind: EventKind::End,
        },
    );
    handler.add_global_event(
        TrackEvent::Error.into(),
        GuildTrackEvents {
            core: core.clone(),
            guild_id,
            kind: EventKind::Error,
        },
    );
}

pub async fn ensure_voice(
    core: &Arc<Core>,
    guild_id: GuildId,
    user_channel: ChannelId,
    old_channel_has_humans: bool,
) -> Result<CallHandle, String> {
    let manager = core.voice.clone();

    if let Some(call) = manager.get(guild_id) {
        let current_vc = core.registry.get(guild_id).voice_channel_id;

        match current_vc {
            None => {
                let _ = manager.remove(guild_id).await;
            }
            Some(vc) if vc != user_channel => {
                if core.stay_channel(guild_id).await == Some(vc) {
                    return Err(format!(
                        "{}  I'm locked to 24/7 mode in <#{vc}>!\nJoin me there, or drag me to your VC and I'll return in 5 minutes.",
                        config::emojis::ERROR
                    ));
                }
                if old_channel_has_humans {
                    return Err(format!(
                        "{}  Already playing in <#{vc}>!",
                        config::emojis::ERROR
                    ));
                }
                let _ = manager.remove(guild_id).await;
            }
            Some(_) => return Ok(call),
        }
    }

    let call = manager
        .join(guild_id, user_channel)
        .await
        .map_err(|e| format!("{}  Could not join VC: `{e}`", config::emojis::ERROR))?;

    {
        let mut handler = call.lock().await;
        let _ = handler.deafen(true).await;
        apply_bitrate(&mut handler, core);
    }

    core.registry.get(guild_id).voice_channel_id = Some(user_channel);
    attach_track_events(&call, core, guild_id).await;
    Ok(call)
}

fn apply_bitrate(handler: &mut songbird::Call, core: &Core) {
    use songbird::driver::Bitrate;
    // Discord accepts up to 512 kbps; clamp so config typos can't exceed it.
    let kbps = core.cfg.audio.bitrate_kbps.min(512);
    handler.set_bitrate(if kbps == 0 {
        Bitrate::Auto
    } else {
        Bitrate::Bits((kbps * 1000) as i32)
    });
}

fn cancel_timers(st: &mut GuildState) {
    if let Some(t) = st.inactivity_task.take() {
        t.abort();
    }
    if let Some(t) = st.stay_return_task.take() {
        t.abort();
    }
}

async fn set_channel_status(core: &Core, guild_id: GuildId, track: &Track) {
    if !core.cfg.audio.channel_status_updates {
        return;
    }
    let vc = core.registry.get(guild_id).voice_channel_id;
    if let Some(vc) = vc {
        // "🎧  Now Playing · {title} — {author}" (Discord hard-caps at 500)
        let mut status = String::new();
        status.push_str(config::emojis::NP);
        status.push_str("  Now Playing · ");
        status.push_str(&track.title);
        if !track.author.is_empty() && track.author != "Unknown" {
            status.push_str(" — ");
            status.push_str(&track.author);
        }
        status = status.chars().take(490).collect();
        match core
            .http_api
            .edit_channel(
                serenity::all::GenericChannelId::new(vc.get()),
                &EditChannel::new().status(status),
                None,
            )
            .await
        {
            Ok(()) => {}
            Err(e) => warn!(
                error = %e,
                %vc,
                "voice channel status update failed (bot needs Manage Channels on the VC)"
            ),
        }
    }
}

async fn clear_channel_status(core: &Core, guild_id: GuildId) {
    if !core.cfg.audio.channel_status_updates {
        return;
    }
    let vc = core.registry.get(guild_id).voice_channel_id;
    if let Some(vc) = vc {
        let _ = core
            .http_api
            .edit_channel(
                serenity::all::GenericChannelId::new(vc.get()),
                &EditChannel::new().status(""),
                None,
            )
            .await;
    }
}

pub fn schedule_auto_leave(core: &Arc<Core>, guild_id: GuildId) {    let mut st = core.registry.get(guild_id);
    if let Some(t) = st.inactivity_task.take() {
        t.abort();
    }
    let core2 = core.clone();
    st.inactivity_task = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(core2.cfg.audio.auto_leave_secs)).await;
        let mut st = core2.registry.get(guild_id);
        if core2.stay_channel(guild_id).await.is_some() || !st.queue.is_empty() || st.playing {
            return;
        }
        let _ = core2.voice.remove(guild_id).await;
        st.voice_channel_id = None;
        cancel_timers(&mut st);
        let home = st.home_channel;
        drop(st);

        let prefix = core2.prefix(Some(guild_id)).await;
        if let Some(home) = home {
            say_to(
                &core2,
                home,
                format!(
                    "{}  Left voice after 5 minutes of inactivity.\n-# Use `{prefix}join` for 24/7 mode (never auto-leaves).",
                    config::emojis::SLEEP
                ),
            )
            .await;
        }
    }));
}

pub async fn schedule_stay_return(core: &Arc<Core>, guild_id: GuildId) {
    if core.stay_channel(guild_id).await.is_none() {
        return;
    }
    let core2 = core.clone();

    let mut st = core.registry.get(guild_id);
    if let Some(t) = st.stay_return_task.take() {
        t.abort();
    }
    st.stay_return_task = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(core2.cfg.audio.auto_leave_secs)).await;

        let mut st = core2.registry.get(guild_id);
        let target = match core2.stay_channel(guild_id).await {
            Some(t) => t,
            None => return,
        };
        if st.voice_channel_id == Some(target) || st.playing || !st.queue.is_empty() {
            return;
        }

        let _ = core2.voice.remove(guild_id).await;
        st.voice_channel_id = None;
        let home = st.home_channel;
        drop(st);

        tokio::time::sleep(Duration::from_millis(300)).await;
        match core2.voice.join(guild_id, target).await {
            Ok(call) => {
                {
                    let mut handler = call.lock().await;
                    let _ = handler.deafen(true).await;
                    apply_bitrate(&mut handler, &core2);
                }
                core2.registry.get(guild_id).voice_channel_id = Some(target);
                attach_track_events(&call, &core2, guild_id).await;
                if let Some(home) = home {
                    say_to(
                        &core2,
                        home,
                        format!(
                            "{}  Returned to the 24/7 channel after dragging me away.\n-# I'll head back to <#{target}> whenever playback stops.",
                            config::emojis::SUCCESS
                        ),
                    )
                    .await;
                }
            }
            Err(e) => error!(guild = %guild_id, error = %e, "stay-return rejoin failed"),
        }
    }));
}

pub async fn play_next(core: Arc<Core>, guild_id: GuildId) {
    let Some(call) = core.voice.get(guild_id) else {
        let mut st = core.registry.get(guild_id);
        st.current = None;
        st.playing = false;
        return;
    };
    let mut st = core.registry.get(guild_id);

    // Old NP card: delete it (default) or leave it as scrollback history.
    if core.cfg.audio.np_delete_previous {
        if let Some((channel, msg)) = st.np_message.take() {
            let core2 = core.clone();
            tokio::spawn(async move {
                let _ = serenity::all::GenericChannelId::new(channel.get())
                    .delete_message(&core2.http_api, msg, None)
                    .await;
            });
        }
    } else {
        st.np_message = None;
    }

    if let Some(cur) = st.current.take() {
        match st.loop_mode {
            LoopMode::Track => st.queue.push_front(cur),
            LoopMode::Queue => st.queue.push_back(cur),
            LoopMode::Off => {}
        }
    }

    let Some(track) = st.queue.pop_front() else {
        // Only announce conclusion when something actually just finished;
        // songbird can invoke this path twice for one ending (Error then
        // End) and duplicate messages read like a glitch.
        let had_activity = st.playing || st.current.is_some() || st.previous.is_some();
        st.previous = None;
        st.playing = false;
        st.loop_mode = LoopMode::Off;
        st.current_is_cached = false;
        cancel_timers(&mut st);
        let home = st.home_channel;
        drop(st);

        if had_activity {
            clear_channel_status(&core, guild_id).await;
            let prefix = core.prefix(Some(guild_id)).await;
            if let Some(home) = home {
                say_to(
                    &core,
                    home,
                    format!(
                        "## ⏹️ Queue Concluded\nUse `{prefix}play` to add more songs.\n-# 💤 Leaving voice in 5 minutes unless 24/7 mode is active.",
                    ),
                )
                .await;
            }
        }

        if core.stay_channel(guild_id).await.is_some() {
            schedule_stay_return(&core, guild_id).await;
        } else {
            schedule_auto_leave(&core, guild_id);
        }
        return;
    };

    st.previous = st.current.clone();
    st.current = Some(track.clone());
    st.playing = true;
    st.paused = false;
    let volume = st.volume;
    let home = st.home_channel;
    drop(st);

    cancel_timers(&mut core.registry.get(guild_id));

    // Spotify-matched entries store a lazy `ytsearch1:artist title` query.
    // Before playing, upgrade it via YouTube Music (official catalog) with a
    // plain-YouTube fallback — then the stored uri is rewritten to the exact
    // watch URL so seeks/restarts never re-match.
    let mut play_uri = track.uri.clone();
    if track.source == crate::state::SourceTag::SpotifyMatched
        && let Some(query) = track.uri.strip_prefix("ytsearch1:")
    {
        match crate::sources::match_on_youtube(query).await {
            Ok(m) => {
                play_uri = m.webpage_url.clone();
                if let Some(cur) = core.registry.get(guild_id).current.as_mut() {
                    cur.uri = play_uri.clone();
                    cur.ui_link = cur.ui_link.clone().or(Some(m.webpage_url));
                }
            }
            Err(_) => { /* keep the raw ytsearch query — yt-dlp handles it */ }
        }
    }

    // Probe the stream once for authoritative metadata — resolvers (ytmusic,
    // spotify-match, embed scrape) can return partial or stale info, but this
    // extraction is exactly what will be played.
    let input = yt_input(&core, play_uri);
    let mut probe = input.clone();
    if let Ok(aux) = probe.aux_metadata().await {
        let mut st = core.registry.get(guild_id);
        if let Some(cur) = st.current.as_mut() {
            if let Some(t) = aux.title {
                cur.title = crate::state::clean_title(&t);
            }
            if let Some(c) = aux.channel.or(aux.artist)
                && !c.is_empty() && c != "Unknown" {
                    cur.author = c;
                }
            if let Some(d) = aux.duration {
                cur.duration_secs = Some(d.as_secs());
                cur.is_live = false;
            }
            if let Some(th) = aux.thumbnail
                && !th.is_empty() {
                    cur.thumbnail = th;
                }
        }
        drop(st);
    }

    let handle = {
        let mut guard = call.lock().await;
        guard.play_input(input.into())
    };

    if let Err(e) = handle.set_volume(f32::from(volume) / 100.0) {
        warn!(error = ?e, "set_volume failed");
    }
    {
        let mut st = core.registry.get(guild_id);
        st.error_streak = 0;
        st.recovering = false;
        st.current_is_cached = false;
        st.current_handle = Some(handle.clone());
    }

    // Status + NP card AFTER the stream is rolling so the mixer never
    // competes with HTTP work in its first critical 20ms frames.
    set_channel_status(&core, guild_id, &track).await;

    if let Some(home) = home {
        let st_view = core.registry.get(guild_id);
        let comps = crate::ui::now_playing_components(guild_id.get(), &st_view, &track);
        drop(st_view);
        match serenity::all::GenericChannelId::new(home.get())
            .send_message(&core.http_api, crate::ui::container_msg(comps))
            .await
        {
            Ok(msg) => {
                log_info!(
                    "cv2",
                    "np card sent: id={} flags={:?} (Some(32768) = IS_COMPONENTS_V2)",
                    msg.id,
                    msg.flags.map(|f| f.bits())
                );
                core.registry.get(guild_id).np_message = Some((home, msg.id));
            }
            Err(e) => warn!(error = %e, "np message send failed"),
        }
    }
}

impl Track {
    pub fn uri_safe(&self) -> String {
        if self.uri.is_empty() {
            "https://discord.gg".into()
        } else {
            self.uri.clone()
        }
    }
}



pub async fn handle_voice_state_update(
    ctx: &serenity::all::Context,
    core: &Arc<Core>,
    old: &VoiceState,
    new: &VoiceState,
) {
    let Some(guild_id) = new.guild_id.or(old.guild_id) else {
        return;
    };

    if new.user_id == core.bot_id {
        let left = old.channel_id.is_some() && new.channel_id.is_none();
        let moved = matches!((old.channel_id, new.channel_id), (Some(a), Some(b)) if a != b);

        if left {
            let mut st = core.registry.get(guild_id);
            st.request_stop();
            st.playing = false;
            st.current = None;
            st.previous = None;
            st.voice_channel_id = None;
            let np = st.np_message.take();
            drop(st);

            if let Some((c, m)) = np {
                let _ = serenity::all::GenericChannelId::new(c.get())
                    .delete_message(&core.http_api, m, None)
                    .await;
            }
            let _ = core.voice.remove(guild_id).await;
            return;
        }

        if moved {
            let new_ch = new.channel_id.unwrap();
            let stay = core.stay_channel(guild_id).await;
            let mut st = core.registry.get(guild_id);
            st.voice_channel_id = Some(new_ch);
            if stay == Some(new_ch) {
                if let Some(t) = st.stay_return_task.take() {
                    t.abort();
                }
            } else if stay.is_some() && !st.playing && st.queue.is_empty() {
                let core2 = core.clone();
                drop(st);
                schedule_stay_return(&core2, guild_id).await;
            }
        }
        return;
    }

    let bot_vc = core.registry.get(guild_id).voice_channel_id;
    let Some(bot_vc) = bot_vc else { return };
    if old.channel_id != Some(bot_vc) || new.user_id == core.bot_id {
        return;
    }

    let humans_left = |ctx: &serenity::all::Context| {
        ctx.cache
            .guild(guild_id)
            .map(|g| {
                g.voice_states
                    .iter()
                    .filter(|vs| vs.channel_id == Some(bot_vc))
                    .any(|vs| vs.user_id != core.bot_id)
            })
            .unwrap_or(true)
    };

    if humans_left(ctx) {
        return;
    }

    tokio::time::sleep(Duration::from_secs(core.cfg.audio.empty_channel_grace_secs)).await;
    if humans_left(ctx) {
        return;
    }

    info!(guild = %guild_id, "channel empty; clearing queue and leaving/returning");

    let was_live = {
        let st = core.registry.get(guild_id);
        st.playing || st.current.is_some() || st.current_handle.is_some()
    };
    {
        let mut st = core.registry.get(guild_id);
        st.queue.clear();
        st.loop_mode = LoopMode::Off;
        st.playing = false;
        st.current = None;
        st.previous = None;
        // Only mark the intentional-stop flag when we are actually interrupting
        // live audio; otherwise no TrackEnd will ever consume it and a later
        // natural end would be swallowed (player wedged in "playing" state).
        if was_live {
            st.request_stop();
        }
    }

    if was_live
        && let Some(call) = core.voice.get(guild_id) {
            call.lock().await.stop();
        }

    if let Some(stay) = core.stay_channel(guild_id).await {
        if bot_vc != stay {
            let _ = core.voice.remove(guild_id).await;
            core.registry.get(guild_id).voice_channel_id = None;
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Ok(call) = core.voice.join(guild_id, stay).await {
                let mut handler = call.lock().await;
                let _ = handler.deafen(true).await;
                apply_bitrate(&mut handler, core);
                drop(handler);
                core.registry.get(guild_id).voice_channel_id = Some(stay);
                attach_track_events(&call, core, guild_id).await;
            }
        }
    } else {
        let _ = core.voice.remove(guild_id).await;
        core.registry.get(guild_id).voice_channel_id = None;
    }
}

/// Rejoins every configured 24/7 channel after boot so a restart never
/// requires anyone to summon the bot again.
pub async fn rejoin_stay_channels(core: &Arc<Core>) {
    let stays = core.stay_channels_snapshot().await;
    if stays.is_empty() {
        return;
    }
    log_info!("stay", "rejoining {} 24/7 channel(s)", stays.len());
    for (guild_id, channel) in stays {
        match core.voice.join(guild_id, channel).await {
            Ok(call) => {
                {
                    let mut handler = call.lock().await;
                    let _ = handler.deafen(true).await;
                    apply_bitrate(&mut handler, core);
                }
                attach_track_events(&call, core, guild_id).await;
                core.registry.get(guild_id).voice_channel_id = Some(channel);
                log_started!(
                    "stay",
                    "rejoined 24/7 channel <#{channel}> in guild {guild_id}"
                );
            }
            Err(e) => {
                error!(
                    guild = %guild_id,
                    channel = %channel,
                    error = %e,
                    "24/7 auto-rejoin failed (missing perms or deleted channel?)"
                );
            }
        }
    }
}

/// Seeks by rebuilding the current track into an in-memory Opus cache and
/// resuming at `target_secs`.
///
/// songbird cannot natively seek HTTP streams (`HttpStream::is_seekable` is
/// always false), and a failed seek KILLS the track. First seek therefore
/// re-extracts via yt-dlp once, caches the Opus frames (seekable), jumps to
/// the target, and marks the guild cached — every later seek on this track
/// is instant and native.
pub async fn restart_current_at(
    core: &Arc<Core>,
    guild_id: GuildId,
    target_secs: u64,
) -> Result<(), String> {
    use songbird::driver::Bitrate;
    use songbird::input::cached::Compressed;

    let Some(call) = core.voice.get(guild_id) else {
        return Err("not connected".into());
    };

    let (track, volume) = {
        let mut st = core.registry.get(guild_id);
        let track = st.current.clone().ok_or("nothing playing")?;
        // The old handle's End event must not advance the queue.
        st.request_stop();
        if let Some(h) = st.current_handle.take() {
            let _ = h.stop();
        }
        st.playing = false;
        (track, st.volume)
    };

    let kbps = core.cfg.audio.bitrate_kbps.clamp(64, 512);
    let bitrate = Bitrate::Bits((kbps * 1000) as i32);
    let input = yt_input(core, track.uri.clone());

    let cached = Compressed::new(input.into(), bitrate)
        .await
        .map_err(|e| format!("re-buffer failed: {e}"))?;

    let handle = {
        let mut guard = call.lock().await;
        guard.play_input(cached.into())
    };
    if let Err(e) = handle.set_volume(f32::from(volume) / 100.0) {
        warn!(error = ?e, "set_volume failed");
    }
    // The cache IS seekable, so this lands exactly and instantly.
    let _ = handle.seek_async(Duration::from_secs(target_secs)).await;

    {
        let mut st = core.registry.get(guild_id);
        st.current_handle = Some(handle);
        st.playing = true;
        st.paused = false;
        st.error_streak = 0;
        st.recovering = false;
        st.current_is_cached = true;
    }
    log_info!("seek", "rebuffered guild {guild_id} at {target_secs}s (cached)");
    Ok(())
}

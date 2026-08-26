use serenity::builder::{
    CreateActionRow, CreateButton, CreateComponent, CreateContainer, CreateContainerComponent,
    CreateMessage, CreateSection, CreateSectionAccessory, CreateSectionComponent,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, CreateTextDisplay,
    CreateThumbnail, CreateUnfurledMediaItem,
};
use serenity::model::application::ButtonStyle;
use serenity::model::channel::{MessageFlags, ReactionType};

use crate::config;
use crate::state::{GuildState, LoopMode, SourceTag, Track};

/// Platform badge for the NP header — Apple Music matches ride on YouTube
/// sources but deserve their own logo.
fn platform_logo(t: &Track) -> &'static str {
    if t.ui_link
        .as_deref()
        .is_some_and(|u| u.contains("music.apple.com"))
    {
        return config::emojis::APPLEMUSIC;
    }
    match t.source {
        SourceTag::Youtube => config::emojis::YOUTUBE,
        SourceTag::SoundCloud => config::emojis::SOUNDCLOUD,
        SourceTag::SpotifyMatched => config::emojis::SPOTIFY,
        SourceTag::Discord | SourceTag::File => config::emojis::FOLDER,
    }
}

pub const CV2_FLAGS: MessageFlags = MessageFlags::IS_COMPONENTS_V2;
pub const CV2_EPHEMERAL: MessageFlags =
    MessageFlags::from_bits_truncate(MessageFlags::IS_COMPONENTS_V2.bits() | MessageFlags::EPHEMERAL.bits());

pub type Comps = Vec<CreateComponent<'static>>;

pub fn container(components: Vec<CreateContainerComponent<'static>>, _accent: u32) -> Comps {
    vec![CreateComponent::Container(
        CreateContainer::new(components).into_owned(),
    )]
}

fn text(s: impl Into<String>) -> CreateContainerComponent<'static> {
    CreateContainerComponent::TextDisplay(CreateTextDisplay::new(s.into()).into_owned())
}

fn section_with_thumbnail(desc: String, thumb: String) -> CreateContainerComponent<'static> {
    CreateContainerComponent::Section(
        CreateSection::new(
            vec![CreateSectionComponent::TextDisplay(
                CreateTextDisplay::new(desc).into_owned(),
            )],
            CreateSectionAccessory::Thumbnail(
                CreateThumbnail::new(CreateUnfurledMediaItem::new(thumb)).into_owned(),
            ),
        )
        .into_owned(),
    )
}

pub fn container_msg(components: Comps) -> CreateMessage<'static> {
    CreateMessage::new().flags(CV2_FLAGS).components(components)
}

pub fn parse_emoji(s: &str) -> Option<ReactionType> {
    let s = s.trim();
    let inner = s.strip_prefix('<').and_then(|r| r.strip_suffix('>'))?;
    let mut parts = inner.split(':');
    let animated = parts.next()? == "a";
    let name = parts.next()?;
    let id = parts.next()?.parse::<u64>().ok()?;
    Some(ReactionType::Custom {
        animated,
        name: Some(small_fixed_array::FixedString::from_string_trunc(
            name.to_string(),
        )),
        id: serenity::model::id::EmojiId::new(id),
    })
}

fn e(s: &'static str) -> ReactionType {
    parse_emoji(s).unwrap_or_else(|| {
        ReactionType::Unicode(small_fixed_array::FixedString::from_string_trunc("🎵".into()))
    })
}

pub fn player_row(state: &GuildState, guild_id: u64) -> CreateActionRow<'static> {
    let paused = state.paused;
    let lm = state.loop_mode;

    CreateActionRow::Buttons(std::borrow::Cow::Owned(vec![
        CreateButton::new(format!("koaai:prev:{guild_id}"))
            .emoji(e(config::emojis::BACK))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("koaai:pp:{guild_id}"))
            .emoji(e(if paused {
                config::emojis::PLAY
            } else {
                config::emojis::PAUSE
            }))
            .style(ButtonStyle::Primary),
        CreateButton::new(format!("koaai:skip:{guild_id}"))
            .emoji(e(config::emojis::SKIP))
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("koaai:loop:{guild_id}"))
            .emoji(e(config::emojis::LOOP))
            .style(if lm != LoopMode::Off {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            }),
        CreateButton::new(format!("koaai:stop:{guild_id}"))
            .emoji(e(config::emojis::STOP))
            .style(ButtonStyle::Danger),
    ]))
}

fn md_title(t: &Track) -> String {
    t.title.replace('[', "(").replace(']', ")")
}

pub fn now_playing_components(guild_id: u64, st: &GuildState, track: &Track) -> Comps {
    let duration = track.duration_display();
    let safe_uri = track.link_for_ui();
    let title_md = md_title(track);

    let ansi = format!(
        "```ansi\n\x1b[34mArtist   \x1b[0m: \x1b[32m{}\x1b[0m\n\x1b[34mDuration \x1b[0m: \x1b[32m{}\x1b[0m\n\x1b[34mSource   \x1b[0m: \x1b[32m{}\x1b[0m\n```",
        track.author.replace('`', "'"),
        duration,
        track.source
    );

    let desc = format!(
        "## {} Now Playing\n**[{title_md}]({safe_uri})**\n{ansi}\n✅ **Requested by:** {}\n✅ **Queue:** {} track(s) remaining",
        platform_logo(track),
        track.requester,
        st.queue.len()
    );

    let mut inner: Vec<CreateContainerComponent<'static>> = Vec::new();
    if !track.thumbnail.is_empty() {
        inner.push(section_with_thumbnail(desc, track.thumbnail.clone()));
    } else {
        inner.push(text(desc));
    }
    inner.push(CreateContainerComponent::ActionRow(player_row(st, guild_id)));

    container(inner, config::C_NP)
}

pub fn queue_components(
    guild_id: u64,
    st: &GuildState,
    page: usize,
    per_page: usize,
) -> Comps {
    let per_page = per_page.max(1);
    let q: Vec<&Track> = st.queue.iter().collect();
    let max_page = q.len().saturating_sub(1) / per_page;
    let start = page * per_page;

    let mut desc = String::from("# 📋 Music Queue\nYour current music collection\n\n");

    if let Some(t) = &st.current {
        desc += &format!(
            "## Currently Playing\n**[{}]({})**\nRequested by **{}** • **{}**\n\n",
            md_title(t),
            t.link_for_ui(),
            t.requester,
            t.duration_display()
        );
    }

    let chunk: Vec<&Track> = q.iter().skip(start).take(per_page).copied().collect();
    if chunk.is_empty() {
        desc += "**Up Next**\nQueue is empty.\n";
    } else {
        desc += "## Up Next\n\n";
        desc += &chunk
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "**{}. [{}]({})**\n└ Added by **{}** • **{}**\n",
                    start + i + 1,
                    md_title(t),
                    t.link_for_ui(),
                    t.requester,
                    t.duration_display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    let total: u64 = q.iter().filter_map(|t| t.duration_secs).sum();
    desc += &format!(
        "\n-# Page {}/{}  •  {} tracks  •  {} total",
        page + 1,
        max_page + 1,
        q.len(),
        crate::state::fmt_sec(total)
    );

    let prev = CreateButton::new(format!("koaai:qprev:{guild_id}:{page}"))
        .emoji(e(config::emojis::BACK))
        .style(ButtonStyle::Secondary)
        .disabled(page == 0);
    let close = CreateButton::new(format!("koaai:qclose:{guild_id}"))
        .emoji(e(config::emojis::STOP))
        .style(ButtonStyle::Danger);
    let next = CreateButton::new(format!("koaai:qnext:{guild_id}:{page}"))
        .emoji(e(config::emojis::SKIP))
        .style(ButtonStyle::Secondary)
        .disabled(page >= max_page);

    let mut inner: Vec<CreateContainerComponent<'static>> = vec![text(desc)];
    inner.push(CreateContainerComponent::ActionRow(
        CreateActionRow::Buttons(std::borrow::Cow::Owned(vec![prev, close, next])),
    ));

    container(inner, config::C_QUEUE)
}

const HELP_PAGES: &[(&str, &str, &[&str])] = &[
    ("Music", "Playback commands", &["play","pause","resume","skip","stop","nowplaying","seek","forward","rewind"]),
    ("Playlist", "Queue management", &["queue","shuffle","clear","remove","move","loop"]),
    ("Audio", "Volume and filters", &["volume"]),
    ("Settings", "Bot configuration", &["setprefix","join","leave"]),
    ("Info", "Bot statistics", &["ping","uptime","help"]),
];

fn cat_emoji(name: &str) -> &'static str {
    match name {
        "Music" => config::emojis::CAT_MUSIC,
        "Playlist" => config::emojis::CAT_PLAYLIST,
        "Audio" => config::emojis::CAT_AUDIO,
        "Settings" => config::emojis::CAT_SETTINGS,
        "Info" => config::emojis::CAT_INFO,
        _ => "📁",
    }
}

fn help_home_body(prefix: &str, total_cmds: usize, bot_name: &str) -> String {
    let cats = HELP_PAGES
        .iter()
        .map(|(name, desc, _)| format!("{}  **{name}** — {desc}", cat_emoji(name)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# 🍵 {}\nYour Ultimate Music Companion\n\n✅ **Server Prefix:** `{prefix}`\n✅ **Total Commands:** `{total_cmds}`\n\n## Available Categories\n{cats}\n\n-# *Use the select menu below to browse a category.*",
        bot_name.to_uppercase()
    )
}

fn help_category_body(prefix: &str, category: &str) -> Option<String> {
    HELP_PAGES
        .iter()
        .find(|(name, _, _)| *name == category)
        .map(|(name, desc, cmds)| {
            let list = cmds
                .iter()
                .map(|c| format!("`{prefix}{c}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}  # {name}\n{desc}\n\n## Available Commands ({})\n{list}\n\n-# Found a bug? Use `{prefix}report` — it lands straight in the developer's DMs.",
                cat_emoji(name),
                cmds.len()
            )
        })
}

pub fn help_components(
    guild_id: u64,
    prefix: &str,
    category: Option<&str>,
    bot_name: &str,
    total_cmds: usize,
) -> Comps {
    let body = match category {
        Some(c) => {
            help_category_body(prefix, c).unwrap_or_else(|| help_home_body(prefix, total_cmds, bot_name))
        }
        None => help_home_body(prefix, total_cmds, bot_name),
    };

    let options: Vec<CreateSelectMenuOption> = HELP_PAGES
        .iter()
        .map(|(name, desc, _)| CreateSelectMenuOption::new(*name, *name).description(*desc))
        .collect();

    let select_row = CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            format!("koaai:helpcat:{guild_id}"),
            CreateSelectMenuKind::String {
                options: std::borrow::Cow::Owned(options),
            },
        )
        .placeholder("Browse command categories"),
    );

    let inner: Vec<CreateContainerComponent<'static>> =
        vec![text(body), CreateContainerComponent::ActionRow(select_row)];

    container(inner, config::C_MAIN)
}

pub mod router {
    use std::sync::Arc;

    use serenity::builder::{
        CreateInteractionResponse, CreateInteractionResponseMessage,
        EditMessage,
    };
    use serenity::model::application::ComponentInteraction;
    use serenity::model::id::{GuildId, UserId};
    use serenity::prelude::Context;
    use tracing::warn;

    use super::*;
    use crate::Core;

    fn err_comps(msg: String) -> Comps {
        let body = format!("{}  {msg}", config::emojis::ERROR);
        container(vec![text(body)], config::C_ERROR)
    }

    async fn ephemeral(core: &Core, interaction: &ComponentInteraction, msg: String) {
        let comps = err_comps(msg);
        let builder = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .flags(CV2_EPHEMERAL)
                .components(comps),
        );
        if let Err(e) = interaction.create_response(&core.http_api, builder).await {
            warn!(error = %e, "failed to respond to component");
        }
    }

    fn vc_ok(core: &Core, ctx: &Context, guild_id: GuildId, user_id: UserId) -> bool {
        let Some(bot_vc) = core.registry.get(guild_id).voice_channel_id else {
            return false;
        };
        match ctx.cache.guild(guild_id) {
            Some(g) => g.voice_states.get(&user_id).and_then(|vs| vs.channel_id),
            None => None,
        }
        .map(|c| c == bot_vc)
        .unwrap_or(false)
    }

    async fn refresh_np(core: &Core, guild_id: GuildId) {
        let (np_msg, track) = {
            let st = core.registry.get(guild_id);
            (st.np_message, st.current.clone())
        };
        let Some((channel, msg)) = np_msg else { return };
        let Some(track) = track else { return };

        let st = core.registry.get(guild_id);
        let comps = now_playing_components(guild_id.get(), &st, &track);
        drop(st);

        let _ = serenity::all::GenericChannelId::new(channel.get())
            .edit_message(
                &core.http_api,
                msg,
                EditMessage::new()
                    .flags(CV2_FLAGS)
                    .components(comps),
            )
            .await;
    }

    pub async fn component(
        core: &Arc<Core>,
        ctx: &Context,
        interaction: &ComponentInteraction,
    ) -> bool {
        let Some(rest) = interaction.data.custom_id.strip_prefix("koaai:") else {
            return false;
        };
        let parts: Vec<&str> = rest.split(':').collect();
        let (Some(action), Some(g_raw)) = (parts.first().copied(), parts.get(1).copied()) else {
            return false;
        };
        let Ok(gid) = g_raw.parse::<u64>() else {
            return false;
        };
        if interaction.guild_id != Some(GuildId::new(gid)) {
            return true;
        }
        let guild_id = GuildId::new(gid);

        match action {
            "helpcat" => return select_menu(core, interaction).await,
            "qclose" => {
                let _ = interaction
                    .create_response(&core.http_api, CreateInteractionResponse::Acknowledge)
                    .await;
                let _ = interaction
                    .channel_id
                    .delete_message(&core.http_api, interaction.message.id, None)
                    .await;
                return true;
            }
            "qprev" | "qnext" => {
                if !vc_ok(core, ctx, guild_id, interaction.user.id) {
                    ephemeral(
                        core,
                        interaction,
                        "Join my voice channel!".to_string(),
                    )
                    .await;
                    return true;
                }
                let per_page = core.cfg.bot.items_per_page;
                let page: i64 = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
                let max_page =
                    core.registry.get(guild_id).queue.len().saturating_sub(1) / per_page;
                let new_page = if action == "qprev" {
                    page.saturating_sub(1)
                } else {
                    (page + 1).min(max_page as i64)
                };
                let st = core.registry.get(guild_id);
                let comps = queue_components(gid, &st, new_page.max(0) as usize, per_page);
                drop(st);
                let _ = interaction
                    .create_response(
                        &core.http_api,
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .flags(CV2_FLAGS)
                                .components(comps),
                        ),
                    )
                    .await;
                return true;
            }
            _ => {}
        }

        if !vc_ok(core, ctx, guild_id, interaction.user.id) {
            ephemeral(core, interaction, "Join my voice channel!".to_string()).await;
            return true;
        }

        let core2 = core.clone();
        match action {
            "prev" => {
                {
                    let mut st = core.registry.get(guild_id);
                    if let Some(cur) = st.current.take() {
                        st.queue.push_front(cur);
                    }
                    st.request_stop();
                    st.playing = false;
                }
                let _ = interaction
                    .create_response(&core.http_api, CreateInteractionResponse::Acknowledge)
                    .await;
                if let Some(h) = core.registry.get(guild_id).current_handle.take() {
                    let _ = h.stop();
                }
                tokio::spawn(async move { crate::player::play_next(core2, guild_id).await });
            }
            "pp" => {
                let handle = core.registry.get(guild_id).current_handle.clone();
                let Some(h) = handle else {
                    ephemeral(core, interaction, "Nothing is playing.".to_string()).await;
                    return true;
                };
                let was_paused = core.registry.get(guild_id).paused;
                let res = if was_paused { h.play() } else { h.pause() };
                match res {
                    Ok(()) => {
                        core.registry.get(guild_id).paused = !was_paused;
                        let _ = interaction
                            .create_response(
                                &core.http_api,
                                CreateInteractionResponse::Acknowledge,
                            )
                            .await;
                        refresh_np(core, guild_id).await;
                    }
                    Err(_) => {
                        ephemeral(core, interaction, "Failed to toggle playback.".to_string())
                            .await;
                    }
                }
            }
            "skip" => {
                {
                    let mut st = core.registry.get(guild_id);
                    st.request_stop();
                    st.playing = false;
                    st.previous = st.current.take();
                }
                let _ = interaction
                    .create_response(&core.http_api, CreateInteractionResponse::Acknowledge)
                    .await;
                if let Some(h) = core.registry.get(guild_id).current_handle.take() {
                    let _ = h.stop();
                }
                tokio::spawn(async move { crate::player::play_next(core2, guild_id).await });
            }
            "loop" => {
                {
                    let mut st = core.registry.get(guild_id);
                    st.loop_mode = st.loop_mode.cycle();
                }
                let _ = interaction
                    .create_response(&core.http_api, CreateInteractionResponse::Acknowledge)
                    .await;
                refresh_np(core, guild_id).await;
            }
            "stop" => {
                {
                    let mut st = core.registry.get(guild_id);
                    st.request_stop();
                    st.queue.clear();
                    st.loop_mode = LoopMode::Off;
                    st.playing = false;
                    st.previous = None;
                    st.current = None;
                }
                if let Some(h) = core.registry.get(guild_id).current_handle.take() {
                    let _ = h.stop();
                }

                if core.stay_channel(guild_id).await.is_some() {
                    ephemeral(core, interaction, "Stopped. (24/7 active)".to_string()).await;
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
                    ephemeral(core, interaction, "Stopped and left.".to_string()).await;
                }
            }
            _ => {}
        }
        true
    }

    async fn select_menu(
        core: &Arc<Core>,
        interaction: &ComponentInteraction,
    ) -> bool {
        let Some(rest) = interaction.data.custom_id.strip_prefix("koaai:helpcat:") else {
            return false;
        };
        let Ok(gid) = rest.parse::<u64>() else {
            return false;
        };
        if interaction.guild_id != Some(GuildId::new(gid)) {
            return true;
        }
        let Some(category) = (match &interaction.data.kind {
            serenity::model::application::ComponentInteractionDataKind::StringSelect {
                values,
            } => values.first().cloned(),
            _ => None,
        }) else {
            return true;
        };

        let prefix = core.prefix(Some(GuildId::new(gid))).await;
        let comps = help_components(gid, &prefix, Some(&category), "Koaai", 22);

        let _ = interaction
            .create_response(
                &core.http_api,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .flags(CV2_FLAGS)
                        .components(comps),
                ),
            )
            .await;
        true
    }
}

pub fn error_container(msg: &str) -> Comps {
    let body = format!("{}  {msg}", config::emojis::ERROR);
    container(vec![text(body)], config::C_ERROR)
}

pub fn success_container(msg: &str) -> Comps {
    let body = format!("{}  {msg}", config::emojis::SUCCESS);
    container(vec![text(body)], config::C_SUCCESS)
}

pub fn info_container(msg: impl Into<String>) -> Comps {
    container(vec![text(msg.into())], config::C_INFO)
}
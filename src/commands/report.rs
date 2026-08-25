use std::fmt::Write as _;

use chrono::{SecondsFormat, Utc};
use serenity::builder::{
    CreateActionRow, CreateButton, CreateContainerComponent, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateLabel, CreateModal,
    CreateModalComponent, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    CreateSeparator, CreateTextDisplay,
};
use serenity::model::application::{
    ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, InputTextStyle,
    LabelComponent, ModalComponent, ModalInteraction, SeparatorSpacingSize,
};
use serenity::model::id::UserId;
use tracing::warn;

use crate::config;
use crate::db::{Db, ReportFilter, ReportRow};
use crate::ui::{container_msg, CV2_EPHEMERAL, CV2_FLAGS};
use crate::{Context, Error};

const REPORTS_PER_PAGE: usize = 4;

/// (value, emoji, name, description)
const CATEGORIES: &[(&str, &str, &str, &str)] = &[
    ("bug", "🐛", "Bug Report", "Something isn't working right"),
    ("feature", "💡", "Feature Request", "An idea you'd love to see added"),
    ("crash", "💥", "Bot Crash", "Bot crashed or stopped responding"),
    ("audio", "🎵", "Audio Issue", "Playback glitches or silence"),
    ("other", "📝", "Other", "Anything else on your mind"),
];

fn cat_label(value: &str) -> String {
    CATEGORIES
        .iter()
        .find(|(v, _, _, _)| *v == value)
        .map(|(_, emoji, name, _)| format!("{emoji} {name}"))
        .unwrap_or_else(|| format!("📋 {value}"))
}

fn cat_emoji(value: &str) -> &'static str {
    CATEGORIES
        .iter()
        .find(|(v, _, _, _)| *v == value)
        .map(|(_, emoji, _, _)| *emoji)
        .unwrap_or("📋")
}

pub fn is_owner(user_id: u64) -> bool {
    config::owner_id() == user_id.to_string()
}

fn iso_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 digits are ascii")
}

fn discord_ts(iso: &str, style: char) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| format!("<t:{}:{style}>", d.timestamp()))
        .unwrap_or_else(|_| "-".into())
}

// ─────────────────────────────────────────────────────
//  UI builders
// ─────────────────────────────────────────────────────

fn sep(divider: bool) -> CreateContainerComponent<'static> {
    CreateContainerComponent::Separator(
        CreateSeparator::new()
            .divider(divider)
            .spacing(SeparatorSpacingSize::Small),
    )
}

fn txt(s: impl Into<String>) -> CreateContainerComponent<'static> {
    CreateContainerComponent::TextDisplay(CreateTextDisplay::new(s.into()).into_owned())
}

fn report_form_components() -> crate::ui::Comps {
    let options: Vec<CreateSelectMenuOption> = CATEGORIES
        .iter()
        .map(|(value, emoji, name, desc)| {
            let label = format!("{emoji} {name}");
            CreateSelectMenuOption::new(label, *value).description(*desc)
        })
        .collect();

    let select_row = CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "koaai:reportcat",
            CreateSelectMenuKind::String {
                options: std::borrow::Cow::Owned(options),
            },
        )
        .placeholder("Choose a category..."),
    );

    let mut body =
        format!("## {} Report a Bug or Give Feedback\nGot a problem or a great idea? Tell the developer directly!\n\n### What can you report?\n", config::emojis::WRENCH);
    for (_value, emoji, name, desc) in CATEGORIES {
        let _ = writeln!(body, "{emoji}  **{name}** — {desc}");
    }

    let inner = vec![
        txt(body),
        sep(true),
        txt("*Select a category below to open the report form. You'll get a DM once your report is resolved.*"),
        CreateContainerComponent::ActionRow(select_row),
    ];

    crate::ui::container(inner, config::C_MAIN)
}

fn dashboard_components(db: &Db, page: i64, filter: ReportFilter) -> crate::ui::Comps {
    let all = db.reports(filter).unwrap_or_default();
    let total = all.len();
    let total_pages = total.div_ceil(REPORTS_PER_PAGE).max(1) as i64;
    let safe_page = page.clamp(0, total_pages - 1);
    let start = (safe_page as usize) * REPORTS_PER_PAGE;
    let slice: Vec<&ReportRow> = all.iter().skip(start).take(REPORTS_PER_PAGE).collect();

    let (total_n, open_n, resolved_n) = db.report_counts().unwrap_or((0, 0, 0));

    let mut inner: Vec<CreateContainerComponent<'static>> = Vec::new();
    inner.push(txt(format!(
        "{}  Report Dashboard\n📋 **Total:** `{total_n}`  •  🔴 **Open:** `{open_n}`  •  ✅ **Resolved:** `{resolved_n}`\n*Showing: {}  •  Page {} / {total_pages} — buttons below cycle filters & pages.*",
        config::emojis::WRENCH,
        filter.label(),
        safe_page + 1,
    )));
    inner.push(sep(true));

    if slice.is_empty() {
        inner.push(txt("*No reports found for this filter.*"));
    } else {
        for r in slice {
            let status = if r.resolved { "✅ Resolved" } else { "🔴 Open" };
            let preview: String = r.description.chars().take(110).collect();
            let preview = if r.description.chars().count() > 110 {
                format!("{preview}…")
            } else {
                preview
            };

            let mut card = format!(
                "### {emoji} {label}  ·  {status}\n**`{id}`**  •  **{user}** (<@{uid}>)\n**Server:** {guild}  •  **#**{chan}  •  {ts}\n\n> {preview}",
                emoji = cat_emoji(&r.category),
                label = cat_label(&r.category),
                id = r.report_id,
                user = r.username,
                uid = r.user_id,
                guild = r.guild_name.as_deref().unwrap_or("Unknown"),
                chan = r.channel_name.as_deref().unwrap_or("Unknown"),
                ts = discord_ts(&r.created_at, 'R'),
                preview = preview.replace('\n', "\n> "),
            );
            if r.steps.is_some() {
                card += "\n\n*Includes steps to reproduce*";
            }
            if let Some(ra) = &r.resolved_at
                && r.resolved {
                    card += &format!("\n\n✅ *Resolved {}*", discord_ts(ra, 'R'));
                }
            inner.push(txt(card));

            if !r.resolved {
                inner.push(CreateContainerComponent::ActionRow(
                    CreateActionRow::Buttons(std::borrow::Cow::Owned(vec![
                        CreateButton::new(format!("koaai:report_resolve:{}", r.report_id))
                            .label("Mark as Resolved")
                            .emoji(serenity::model::channel::ReactionType::Unicode(
                                small_fixed_array::FixedString::from_string_trunc("✅".into()),
                            ))
                            .style(ButtonStyle::Success),
                    ])),
                ));
            }
            inner.push(sep(true));
        }
    }

    let prev = CreateButton::new(format!("koaai:reports_nav:{}:{}", safe_page - 1, filter.key()))
        .label("← Prev")
        .style(ButtonStyle::Secondary)
        .disabled(safe_page == 0);
    let filter_btn = CreateButton::new(format!("koaai:reports_filter:{}:0", filter.next().key()))
        .label(filter.button_label())
        .style(ButtonStyle::Primary);
    let next = CreateButton::new(format!("koaai:reports_nav:{}:{}", safe_page + 1, filter.key()))
        .label("Next →")
        .style(ButtonStyle::Secondary)
        .disabled(safe_page >= total_pages - 1);

    inner.push(CreateContainerComponent::ActionRow(CreateActionRow::Buttons(
        std::borrow::Cow::Owned(vec![prev, filter_btn, next]),
    )));

    crate::ui::container(inner, config::C_MAIN)
}

fn owner_dm_components(r: &ReportRow) -> crate::ui::Comps {
    let mut dm = String::new();
    let _ = write!(
        dm,
        "## {emoji} New Report · {label}\n### `{id}`\n\n### 👤 Reporter\n**{username}** — `{uid}`\n\n### 🏠 Server\n{server}\n\n### 📍 Channel\n{channel}\n\n### ⏰ Submitted\n{ts}\n\n### 📝 Description\n{desc}",
        emoji = cat_emoji(&r.category),
        label = cat_label(&r.category),
        id = r.report_id,
        username = r.username,
        uid = r.user_id,
        server = match &r.guild_name {
            Some(g) => format!("**{g}** — `{}`", r.guild_id.as_deref().unwrap_or("?")),
            None => "*Direct Message*".into(),
        },
        channel = match &r.channel_name {
            Some(c) => format!("**#{c}** — `{}`", r.channel_id.as_deref().unwrap_or("?")),
            None => "*Unknown*".into(),
        },
        ts = discord_ts(&r.created_at, 'F'),
        desc = r.description,
    );
    if let Some(steps) = &r.steps {
        let _ = write!(dm, "\n\n### 🔢 Steps to Reproduce\n{steps}");
    }
    if let Some(extra) = &r.extra {
        let _ = write!(dm, "\n\n### 📎 Additional Info\n{extra}");
    }
    let _ = write!(
        dm,
        "\n\n**Mention:** <@{}>\n*Use `+reports` to view & resolve all reports*",
        r.user_id
    );

    crate::ui::container(vec![txt(dm)], config::C_WARN)
}

async fn dm_owner(core: &crate::Core, comps: crate::ui::Comps) {
    let Ok(owner_id) = config::owner_id().parse::<u64>() else {
        return;
    };
    if let Err(e) = UserId::new(owner_id)
        .direct_message(&core.http_api, container_msg(comps))
        .await
    {
        warn!(error = %e, "failed to DM owner about new report");
    }
}

async fn dm_reporter(core: &crate::Core, r: &ReportRow) {
    let Ok(uid) = r.user_id.parse::<u64>() else {
        return;
    };
    let body = format!(
        "## ✅ Your Report Has Been Resolved!\n\n### 📋 Report Details\n**ID:** `{}`\n**Category:** {}\n**Submitted:** {}\n\n### 📝 Your Description\n{}\n\nThe developer has reviewed and resolved your report. Thank you for helping improve the bot! 👋",
        r.report_id,
        cat_label(&r.category),
        discord_ts(&r.created_at, 'F'),
        r.description,
    );
    let comps = crate::ui::container(vec![txt(body)], config::C_SUCCESS);
    if let Err(e) = UserId::new(uid)
        .direct_message(&core.http_api, container_msg(comps))
        .await
    {
        warn!(error = %e, "failed to DM reporter about resolution");
    }
}

// ─────────────────────────────────────────────────────
//  Command
// ─────────────────────────────────────────────────────

/// Report a bug or send feedback to the developer; owners get a dashboard in DMs
#[poise::command(slash_command, prefix_command, aliases("bug", "feedback", "reports"))]
pub async fn report(ctx: Context<'_>) -> Result<(), Error> {
    let core = ctx.data().core.clone();

    // Owner in DM → reports dashboard instead of the report form
    if ctx.guild_id().is_none() && is_owner(ctx.author().id.get()) {
        let comps = dashboard_components(&core.db, 0, ReportFilter::All);
        ctx.send(poise::CreateReply::default().flags(CV2_FLAGS).components(comps))
            .await?;
        return Ok(());
    }


    // Everyone else → ephemeral report form
    let comps = report_form_components();
    ctx.send(
        poise::CreateReply::default()
            .flags(CV2_EPHEMERAL)
            .components(comps),
    )
    .await?;
    Ok(())
}

// ─────────────────────────────────────────────────────
//  Interaction handlers
// ─────────────────────────────────────────────────────

/// Handles report-related component interactions. Returns true when handled.
pub async fn component(
    core: &std::sync::Arc<crate::Core>,
    interaction: &ComponentInteraction,
) -> bool {
    let custom_id = interaction.data.custom_id.as_str();

    if custom_id == "koaai:reportcat" {
        let category = match &interaction.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => {
                values.first().cloned().unwrap_or_default()
            }
            _ => return true,
        };
        show_modal(core, interaction, category.as_str()).await;
        return true;
    }

    if custom_id.starts_with("koaai:reports_nav:") || custom_id.starts_with("koaai:reports_filter:")
    {
        handle_nav_filter(core, interaction).await;
        return true;
    }

    if let Some(report_id) = custom_id.strip_prefix("koaai:report_resolve:") {
        handle_resolve(core, interaction, report_id).await;
        return true;
    }

    false
}

async fn show_modal(
    core: &std::sync::Arc<crate::Core>,
    interaction: &ComponentInteraction,
    category: &str,
) {
    let modal = CreateModal::new(format!("koaai:reportmodal:{category}"), "📋 Submit Report")
        .components(vec![
            CreateModalComponent::Label(CreateLabel::input_text(
                "Description",
                CreateInputText::new(InputTextStyle::Paragraph, "report_desc")
                    .placeholder("Describe the issue or request in detail...")
                    .required(true)
                    .max_length(1000),
            )),
            CreateModalComponent::Label(CreateLabel::input_text(
                "Steps to Reproduce (bugs only)",
                CreateInputText::new(InputTextStyle::Paragraph, "report_steps")
                    .placeholder("1. Run +play ...\n2. Then do ...\n3. Bug happens when...")
                    .required(false)
                    .max_length(600),
            )),
            CreateModalComponent::Label(CreateLabel::input_text(
                "Additional Info (optional)",
                CreateInputText::new(InputTextStyle::Paragraph, "report_extra")
                    .placeholder("Error messages, timestamps, anything else useful...")
                    .required(false)
                    .max_length(400),
            )),
        ])
        .into_owned();

    if let Err(e) = interaction
        .create_response(&core.http_api, CreateInteractionResponse::Modal(modal))
        .await
    {
        warn!(error = %e, "failed to show report modal");
    }
}

fn modal_field(
    data: &serenity::model::application::ModalInteractionData,
    id: &str,
) -> Option<String> {
    data.components.iter().find_map(|c| match c {
        ModalComponent::Label(l) => match &l.component {
            LabelComponent::InputText(i) if i.custom_id.as_str() == id => {
                Some(i.value.trim().to_string())
            }
            _ => None,
        },
        _ => None,
    })
}

/// Handles report modal submissions. Returns true when handled.
pub async fn modal(
    core: &std::sync::Arc<crate::Core>,
    guild_name: Option<String>,
    interaction: &ModalInteraction,
) -> bool {
    let Some(category) = interaction
        .data
        .custom_id
        .as_str()
        .strip_prefix("koaai:reportmodal:")
    else {
        return false;
    };

    let Some(desc) = modal_field(&interaction.data, "report_desc").filter(|d| !d.is_empty()) else {
        let resp = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .flags(CV2_EPHEMERAL)
                .components(crate::ui::error_container("Description cannot be empty.")),
        );
        let _ = interaction.create_response(&core.http_api, resp).await;
        return true;
    };

    let channel = Some(interaction.channel_id.get().to_string());
    let row = ReportRow {
        report_id: format!("RPT-{}", base36(Utc::now().timestamp_millis() as u64)),
        user_id: interaction.user.id.get().to_string(),
        username: interaction.user.name.to_string(),
        guild_id: interaction.guild_id.map(|g| g.get().to_string()),
        guild_name,
        channel_id: channel.clone(),
        channel_name: channel,
        category: category.to_string(),
        description: desc,
        steps: modal_field(&interaction.data, "report_steps").filter(|s| !s.is_empty()),
        extra: modal_field(&interaction.data, "report_extra").filter(|s| !s.is_empty()),
        resolved: false,
        resolved_at: None,
        created_at: iso_now(),
    };

    if let Err(e) = core.db.insert_report(&row) {
        warn!(error = %e, "failed to insert report");
    }

    dm_owner(core, owner_dm_components(&row)).await;

    let confirm = format!(
        "## ✅ Report Submitted!\n\n**Category:** {}\n**Report ID:** `{}`\n\nYour report went straight to the developer's DMs.\n-# You'll get a DM here the moment it's resolved. Thanks for helping improve the bot! 👋",
        cat_label(&row.category),
        row.report_id,
    );

    let resp = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .flags(CV2_EPHEMERAL)
            .components(crate::ui::success_container(&confirm)),
    );
    if let Err(e) = interaction.create_response(&core.http_api, resp).await {
        warn!(error = %e, "failed to confirm report submission");
    }
    true
}

async fn ephemeral_err(core: &crate::Core, interaction: &ComponentInteraction, msg: &str) {
    let resp = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .flags(CV2_EPHEMERAL)
            .components(crate::ui::error_container(msg)),
    );
    let _ = interaction.create_response(&core.http_api, resp).await;
}

async fn handle_nav_filter(core: &std::sync::Arc<crate::Core>, interaction: &ComponentInteraction) {
    if !is_owner(interaction.user.id.get()) {
        ephemeral_err(core, interaction, "Only the bot owner can browse reports.").await;
        return;
    }

    let parts: Vec<&str> = interaction.data.custom_id.as_str().split(':').collect();
    let (page, filter) = match parts.as_slice() {
        ["koaai", "reports_nav", page, filter] | ["koaai", "reports_filter", filter, page] => {
            (
                page.parse::<i64>().unwrap_or(0),
                ReportFilter::parse(filter),
            )
        }
        _ => return,
    };

    let comps = dashboard_components(&core.db, page, filter);
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
}

async fn handle_resolve(
    core: &std::sync::Arc<crate::Core>,
    interaction: &ComponentInteraction,
    report_id: &str,
) {
    if !is_owner(interaction.user.id.get()) {
        ephemeral_err(core, interaction, "Only the bot owner can resolve reports.").await;
        return;
    }

    let Some(row) = core.db.report_by_id(report_id).ok().flatten() else {
        ephemeral_err(core, interaction, &format!("Report `{report_id}` not found.")).await;
        return;
    };

    if !core.db.resolve_report(report_id, &iso_now()).unwrap_or(false) {
        let resp = CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .flags(CV2_EPHEMERAL)
                .components(crate::ui::info_container("Already resolved.")),
        );
        let _ = interaction.create_response(&core.http_api, resp).await;
        return;
    }

    dm_reporter(core, &row).await;

    // Refresh the dashboard (stay on open filter so resolved ones disappear)
    let refreshed = dashboard_components(&core.db, 0, ReportFilter::Open);
    let _ = interaction
        .create_response(
            &core.http_api,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(CV2_FLAGS)
                    .components(refreshed),
            ),
        )
        .await;
}

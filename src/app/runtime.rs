use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use log::{debug, error, info, trace, warn};
use poise::serenity_prelude as serenity;
use sea_orm::{ConnectionTrait as _, DbBackend, Statement, Value};
use std::{collections::HashSet, time::Instant};
use tokio::task::JoinSet;

use crate::app::{
    db::close,
    discord::list_registered_channels,
    embed::{EmbedLine, EmbedTemplate},
    init::{InitError, init},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error("failed during initialization: {0}")]
    Init(#[from] InitError),

    #[error("failed to receive notification")]
    Recv(#[from] sea_orm::sqlx::Error),

    #[error("invalid ban id payload `{payload}`")]
    ParseBanId {
        payload: String,
        #[source]
        source: std::num::ParseIntError,
    },

    #[error("database query failed for ban id {ban_id}")]
    QueryBan {
        ban_id: i32,
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("ban {ban_id} not found")]
    BanNotFound { ban_id: i32 },

    #[error("failed to wait for Ctrl+C")]
    CtrlC(#[from] std::io::Error),

    #[error("failed to fetch pending ban events")]
    LoadPendingEvents {
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("failed to persist ban message for ban id {ban_id} and channel {channel_id}")]
    SaveBanMessage {
        ban_id: i32,
        channel_id: u64,
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("failed to load ban message for ban id {ban_id} and channel {channel_id}")]
    LoadBanMessage {
        ban_id: i32,
        channel_id: u64,
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("failed to clear event for ban id {ban_id}: {source}")]
    ClearEvent {
        ban_id: i32,
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("invalid column mapping in `{field}`: `{value}`")]
    InvalidColumnMapping { field: &'static str, value: String },
}

#[derive(Clone, Debug)]
pub(super) struct BanSource {
    pub(super) table: String,
    pub(super) id_col: String,
    pub(super) intruder_col: String,
    pub(super) admin_col: String,
    pub(super) kind_col: String,
    pub(super) round_id_col: String,
    pub(super) server_col: String,
    pub(super) created_at_col: String,
    pub(super) duration_end_col: String,
    pub(super) reason_col: String,
}

#[derive(Clone, Debug)]
struct BanRecord {
    id: i32,
    intruder: String,
    admin: String,
    kind: String,
    round_id: i32,
    server: String,
    created_at: String,
    duration_end: String,
    reason: String,
}

/// Returns a localized fallback when the ban reason is not set.
fn reason_display(ban: &BanRecord, no_reason_text: &str) -> String {
    if ban.reason.is_empty() {
        no_reason_text.to_owned()
    } else {
        ban.reason.to_string()
    }
}

#[derive(Clone, Debug)]
struct TimePlaceholders {
    date: String,
    time: String,
    date_time: String,
    time_left: String,
}

fn parse_duration_end(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f%#z") {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(dt.and_utc());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        && let Some(dt) = date.and_hms_opt(0, 0, 0)
    {
        return Some(dt.and_utc());
    }

    None
}

fn format_time_left(duration: Duration) -> String {
    if duration <= Duration::zero() {
        return String::new();
    }
    let mut remaining_minutes = duration.num_minutes();
    if remaining_minutes <= 0 {
        return String::new();
    }

    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;

    let units = [
        ("month", MINUTES_PER_MONTH),
        ("week", MINUTES_PER_WEEK),
        ("day", MINUTES_PER_DAY),
        ("hour", MINUTES_PER_HOUR),
        ("minute", 1),
    ];

    let mut parts = Vec::new();
    for (name, unit_minutes) in units {
        if remaining_minutes < unit_minutes {
            continue;
        }
        let value = remaining_minutes / unit_minutes;
        remaining_minutes %= unit_minutes;
        let suffix = if value == 1 { "" } else { "s" };
        parts.push(format!("{value} {name}{suffix}"));
    }
    parts.join(" ")
}

fn resolve_time_placeholders(created_at: &str, duration_end: &str) -> TimePlaceholders {
    let created_at_dt = parse_duration_end(created_at);
    let duration_end_dt = parse_duration_end(duration_end);
    TimePlaceholders {
        date: created_at_dt
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        time: created_at_dt
            .map(|dt| dt.format("%H:%M UTC").to_string())
            .unwrap_or_default(),
        date_time: created_at_dt
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
        time_left: duration_end_dt
            .map(|dt| format_time_left(dt - Utc::now()))
            .unwrap_or_default(),
    }
}

/// Expands a template fragment by replacing known `{field}` placeholders.
fn render_template_text(
    template: &str,
    ban: &BanRecord,
    no_reason_text: &str,
    time_placeholders: &TimePlaceholders,
) -> String {
    template
        .replace("{id}", &ban.id.to_string())
        .replace("{intruder}", &ban.intruder)
        .replace("{admin}", &ban.admin)
        .replace("{type}", &ban.kind)
        .replace("{round_id}", &ban.round_id.to_string())
        .replace("{server}", &ban.server)
        .replace("{created_at}", &ban.created_at)
        .replace("{duration_end}", &ban.duration_end.to_string())
        .replace("{date}", &time_placeholders.date)
        .replace("{time}", &time_placeholders.time)
        .replace("{date_time}", &time_placeholders.date_time)
        .replace("{time_left}", &time_placeholders.time_left)
        .replace("{reason}", &ban.reason)
        .replace("{reason_display}", &reason_display(ban, no_reason_text))
        .replace("\\n", "\n")
}

/// Event variants supported by database notifications and backlog rows.
///
/// # Examples
///
/// ```ignore
/// // Runtime example
/// let enabled = BanEventType::parse_enabled("ban_added,edited");
/// assert!(enabled.contains(&BanEventType::Added));
/// assert!(enabled.contains(&BanEventType::Edited));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum BanEventType {
    Added,
    Edited,
}

#[derive(Clone, Copy, Debug)]
struct BanEventSpec {
    event_type: BanEventType,
    channel: &'static str,
    db_value: &'static str,
}

const BAN_EVENT_SPECS: [BanEventSpec; 2] = [
    BanEventSpec {
        event_type: BanEventType::Added,
        channel: "ban_added",
        db_value: "added",
    },
    BanEventSpec {
        event_type: BanEventType::Edited,
        channel: "ban_edited",
        db_value: "edited",
    },
];

impl BanEventType {
    fn from_channel(channel: &str) -> Option<Self> {
        BAN_EVENT_SPECS
            .iter()
            .find(|spec| spec.channel == channel)
            .map(|spec| spec.event_type)
    }

    fn from_db_value(event: &str) -> Option<Self> {
        BAN_EVENT_SPECS
            .iter()
            .find(|spec| spec.db_value == event)
            .map(|spec| spec.event_type)
    }

    pub(super) fn listener_channels() -> Vec<&'static str> {
        BAN_EVENT_SPECS.iter().map(|spec| spec.channel).collect()
    }

    /// Parses `EVENT_NAMES` into an enabled event set.
    ///
    /// Supports both notification channel names (`ban_added`) and enum values
    /// (`added`) for backwards compatibility.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let channel_style = BanEventType::parse_enabled("ban_added");
    /// assert!(channel_style.contains(&BanEventType::Added));
    ///
    /// let db_style = BanEventType::parse_enabled("edited");
    /// assert!(db_style.contains(&BanEventType::Edited));
    /// ```
    pub(super) fn parse_enabled(value: &str) -> HashSet<Self> {
        let configured_events: HashSet<String> = value
            .split([',', ' ', '\n', '\t'])
            .map(str::trim)
            .filter(|event| !event.is_empty())
            .map(|event| event.to_ascii_lowercase())
            .collect();
        BAN_EVENT_SPECS
            .iter()
            .filter(|spec| {
                configured_events.contains(spec.db_value)
                    || configured_events.contains(spec.channel)
            })
            .map(|spec| spec.event_type)
            .collect()
    }
}

async fn save_ban_message(
    newsletter_db: &sea_orm::DatabaseConnection,
    ban_id: i32,
    channel_id: u64,
    message_id: u64,
) -> Result<(), sea_orm::DbErr> {
    newsletter_db
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO ban_messages (ban_id, channel_id, message_id) VALUES (?, ?, ?)
             ON CONFLICT (ban_id, channel_id) DO UPDATE SET message_id = excluded.message_id",
            vec![
                Value::Int(Some(ban_id)),
                Value::BigUnsigned(Some(channel_id)),
                Value::BigUnsigned(Some(message_id)),
            ],
        ))
        .await?;
    Ok(())
}

async fn get_ban_message_id(
    newsletter_db: &sea_orm::DatabaseConnection,
    ban_id: i32,
    channel_id: u64,
) -> Result<Option<u64>, sea_orm::DbErr> {
    let rows = newsletter_db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT message_id FROM ban_messages WHERE ban_id = ? AND channel_id = ?",
            vec![
                Value::Int(Some(ban_id)),
                Value::BigUnsigned(Some(channel_id)),
            ],
        ))
        .await?;
    if let Some(row) = rows.first() {
        let value: i64 = row.try_get_by_index(0)?;
        return Ok(u64::try_from(value).ok());
    }
    Ok(None)
}

async fn clear_event(db: &sea_orm::DatabaseConnection, ban_id: i32) -> Result<(), sea_orm::DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM ban_events WHERE ban_id = $1",
        vec![Value::Int(Some(ban_id))],
    ))
    .await?;
    Ok(())
}

async fn load_pending_events(
    db: &sea_orm::DatabaseConnection,
    enabled_event_types: &HashSet<BanEventType>,
) -> Result<Vec<(i32, BanEventType)>, sea_orm::DbErr> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT ban_id, event_type::text FROM ban_events ORDER BY created_at ASC".to_owned(),
        ))
        .await?;
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let ban_id: i32 = row.try_get_by_index(0)?;
        let event_type: String = row.try_get_by_index(1)?;
        if let Some(event) = BanEventType::from_db_value(&event_type) {
            if enabled_event_types.contains(&event) {
                events.push((ban_id, event));
            } else {
                debug!(
                    "pending event {event:?} for ban {ban_id} is disabled by EVENT_NAMES, skipping"
                );
            }
        } else {
            warn!("unknown pending event `{event_type}` for ban {ban_id}, skipping");
        }
    }
    Ok(events)
}

/// Formats a rich ban announcement embed for Discord newsletters using locale preferences.
fn format_ban_embed_for_locale(
    template: &EmbedTemplate,
    ban: &BanRecord,
    channel_locale: Option<&str>,
    user_locale: Option<&str>,
    guild_locale: Option<&str>,
) -> serenity::CreateEmbed {
    let started_at = Instant::now();
    trace!(
        "format_ban_embed started at {started_at:?} for ban {}",
        ban.id
    );
    let translations =
        crate::app::i18n::resolve_translations(channel_locale.or(user_locale), guild_locale);
    let localized_template = if template == &EmbedTemplate::default_template() {
        EmbedTemplate::default_template_for(translations.clone())
    } else {
        template.clone()
    };
    let time_placeholders = resolve_time_placeholders(&ban.created_at, &ban.duration_end);

    let mut embed = serenity::CreateEmbed::new()
        .title(render_template_text(
            &localized_template.title,
            ban,
            &translations.no_reason,
            &time_placeholders,
        ))
        .color(serenity::Color::new(localized_template.color));
    let mut compact_blocks: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < localized_template.lines.len() {
        let line = &localized_template.lines[idx];
        if line.spacer {
            compact_blocks.push(String::new());
            idx += 1;
            continue;
        }
        if line.row_group
            && let Some(group_id) = line.group_id
        {
            let mut group_lines = Vec::new();
            while idx < localized_template.lines.len() {
                let next = &localized_template.lines[idx];
                if next.group_id != Some(group_id) {
                    break;
                }
                group_lines.push(render_line_for_compact_group(
                    next,
                    ban,
                    &translations.no_reason,
                    &time_placeholders,
                ));
                idx += 1;
            }
            compact_blocks.push(group_lines.join("\n"));
            continue;
        }
        if line.inline {
            compact_blocks.push(render_line_for_compact_group(
                line,
                ban,
                &translations.no_reason,
                &time_placeholders,
            ));
            idx += 1;
            continue;
        }
        if line.group_id.is_none() && line.title.as_deref().is_none_or(str::is_empty) {
            compact_blocks.push(render_line_for_compact_group(
                line,
                ban,
                &translations.no_reason,
                &time_placeholders,
            ));
            idx += 1;
            continue;
        }
        let (field_name, field_value) =
            render_line_as_field(line, ban, &translations.no_reason, &time_placeholders);
        embed = embed.field(field_name, field_value, line.field_inline);
        idx += 1;
    }
    if !compact_blocks.is_empty() {
        embed = embed.description(compact_blocks.join("\n"));
    }
    if let Some(footer_text) = localized_template.footer.as_deref() {
        let rendered_footer = render_template_text(
            footer_text,
            ban,
            &translations.no_reason,
            &time_placeholders,
        );
        if !rendered_footer.trim().is_empty() {
            embed = embed.footer(serenity::CreateEmbedFooter::new(rendered_footer));
        }
    }

    debug!(
        "formatted ban embed for {} in {:?}",
        ban.id,
        started_at.elapsed()
    );
    embed
}

fn render_line_as_field(
    line: &EmbedLine,
    ban: &BanRecord,
    no_reason: &str,
    time_placeholders: &TimePlaceholders,
) -> (String, String) {
    let rendered_title = line
        .title
        .as_deref()
        .map(|title| render_template_text(title, ban, no_reason, time_placeholders))
        .unwrap_or_default();
    let rendered_value = render_template_text(&line.value, ban, no_reason, time_placeholders);
    if line.inline {
        return (
            "\u{200B}".to_owned(),
            inline_text_with_bold_title(&rendered_title, &rendered_value),
        );
    }
    let field_name = if rendered_title.trim().is_empty() {
        "\u{200B}".to_owned()
    } else {
        rendered_title
    };
    (field_name, rendered_value)
}

fn render_line_for_compact_group(
    line: &EmbedLine,
    ban: &BanRecord,
    no_reason: &str,
    time_placeholders: &TimePlaceholders,
) -> String {
    let rendered_title = line
        .title
        .as_deref()
        .map(|title| render_template_text(title, ban, no_reason, time_placeholders))
        .unwrap_or_default();
    let rendered_value = render_template_text(&line.value, ban, no_reason, time_placeholders);
    if line.inline {
        return inline_text_with_bold_title(&rendered_title, &rendered_value);
    }
    if rendered_title.trim().is_empty() {
        return rendered_value;
    }
    format!("**{}**\n{}", rendered_title.trim(), rendered_value)
}

fn inline_text_with_bold_title(title: &str, value: &str) -> String {
    if title.trim().is_empty() {
        return value.to_owned();
    }
    format!("**{}** {}", title.trim(), value)
}

fn quoted_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quoted_table_name(table: &str) -> String {
    table
        .split('.')
        .map(quoted_identifier)
        .collect::<Vec<_>>()
        .join(".")
}

#[derive(Debug, Clone)]
enum ColumnMapping {
    Direct {
        column: String,
    },
    Related {
        local_key: String,
        table: String,
        remote_key: String,
        value_column: String,
    },
}

fn parse_column_mapping(raw: &str) -> Option<ColumnMapping> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.matches("->").count() > 1 {
        return None;
    }

    if let Some((local_key, relation)) = trimmed.split_once("->") {
        let local_key = local_key.trim().to_owned();
        let (table_and_remote, value_column) = relation.split_once("::")?;
        let (table_name_raw, remote_key) = table_and_remote.rsplit_once('.')?;
        let table = table_name_raw.trim().trim_matches('"').to_owned();
        return Some(ColumnMapping::Related {
            local_key,
            table,
            remote_key: remote_key.trim().to_owned(),
            value_column: value_column.trim().to_owned(),
        });
    }

    Some(ColumnMapping::Direct {
        column: trimmed.to_owned(),
    })
}

fn select_expression(
    table_alias: &str,
    raw_mapping: &str,
    relation_alias_index: &mut usize,
    joins: &mut Vec<String>,
) -> Option<String> {
    match parse_column_mapping(raw_mapping)? {
        ColumnMapping::Direct { column } => Some(format!(
            "{table_alias}.{}",
            quoted_identifier(column.as_str())
        )),
        ColumnMapping::Related {
            local_key,
            table,
            remote_key,
            value_column,
        } => {
            let relation_alias = format!("rel_{}", *relation_alias_index);
            *relation_alias_index += 1;
            joins.push(format!(
                "LEFT JOIN {} AS {} ON {table_alias}.{} = {}.{}",
                quoted_table_name(&table),
                relation_alias,
                quoted_identifier(&local_key),
                relation_alias,
                quoted_identifier(&remote_key),
            ));
            Some(format!(
                "{}.{}",
                relation_alias,
                quoted_identifier(&value_column)
            ))
        }
    }
}

fn base_filter_column(raw_mapping: &str) -> Option<String> {
    match parse_column_mapping(raw_mapping)? {
        ColumnMapping::Direct { column } => Some(column),
        ColumnMapping::Related { local_key, .. } => Some(local_key),
    }
}

async fn load_ban_record(
    db: &sea_orm::DatabaseConnection,
    source: &BanSource,
    ban_id: i32,
) -> Result<Option<BanRecord>, AppError> {
    let base_alias = "bans_src";
    let mut joins = Vec::new();
    let mut relation_alias_index = 0;
    let id_expr = select_expression(
        base_alias,
        &source.id_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_ID",
        value: source.id_col.clone(),
    })?;
    let intruder_expr = select_expression(
        base_alias,
        &source.intruder_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_INTRUDER",
        value: source.intruder_col.clone(),
    })?;
    let admin_expr = select_expression(
        base_alias,
        &source.admin_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_ADMIN",
        value: source.admin_col.clone(),
    })?;
    let kind_expr = select_expression(
        base_alias,
        &source.kind_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_TYPE",
        value: source.kind_col.clone(),
    })?;
    let round_id_expr = select_expression(
        base_alias,
        &source.round_id_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_ROUND_ID",
        value: source.round_id_col.clone(),
    })?;
    let server_expr = select_expression(
        base_alias,
        &source.server_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_SERVER",
        value: source.server_col.clone(),
    })?;
    let created_at_expr = select_expression(
        base_alias,
        &source.created_at_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_CREATED_AT",
        value: source.created_at_col.clone(),
    })?;
    let duration_end_expr = select_expression(
        base_alias,
        &source.duration_end_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_DURATION_END",
        value: source.duration_end_col.clone(),
    })?;
    let reason_expr = select_expression(
        base_alias,
        &source.reason_col,
        &mut relation_alias_index,
        &mut joins,
    )
    .ok_or_else(|| AppError::InvalidColumnMapping {
        field: "BANS_COL_REASON",
        value: source.reason_col.clone(),
    })?;
    let id_filter_col =
        base_filter_column(&source.id_col).ok_or_else(|| AppError::InvalidColumnMapping {
            field: "BANS_COL_ID",
            value: source.id_col.clone(),
        })?;
    let joins_sql = joins.join("\n         ");

    let query = format!(
        "SELECT
            {id_expr}::int4 AS id,
            COALESCE({intruder_expr}::text, '') AS intruder,
            COALESCE({admin_expr}::text, '') AS admin,
            COALESCE({kind_expr}::text, '') AS kind,
            COALESCE({round_id_expr}::int4, 0) AS round_id,
            COALESCE({server_expr}::text, '') AS server,
            COALESCE({created_at_expr}::text, '') AS created_at,
            COALESCE({duration_end_expr}::text, '') AS duration_end,
            COALESCE({reason_expr}::text, '') AS reason
         FROM {table_name} AS {base_alias}
         {joins_sql}
         WHERE {base_alias}.{id_col} = $1
         LIMIT 1",
        id_expr = id_expr,
        intruder_expr = intruder_expr,
        admin_expr = admin_expr,
        kind_expr = kind_expr,
        round_id_expr = round_id_expr,
        server_expr = server_expr,
        created_at_expr = created_at_expr,
        duration_end_expr = duration_end_expr,
        reason_expr = reason_expr,
        id_col = quoted_identifier(&id_filter_col),
        table_name = quoted_table_name(&source.table),
        base_alias = base_alias,
        joins_sql = joins_sql,
    );

    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            vec![Value::Int(Some(ban_id))],
        ))
        .await
        .map_err(|source| AppError::QueryBan { ban_id, source })?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    Ok(Some(BanRecord {
        id: row
            .try_get_by_index(0)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        intruder: row
            .try_get_by_index(1)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        admin: row
            .try_get_by_index(2)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        kind: row
            .try_get_by_index(3)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        round_id: row
            .try_get_by_index(4)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        server: row
            .try_get_by_index(5)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        created_at: row
            .try_get_by_index(6)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        duration_end: row
            .try_get_by_index(7)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
        reason: row
            .try_get_by_index(8)
            .map_err(|source| AppError::QueryBan { ban_id, source })?,
    }))
}

/// Sends the latest ban information to every registered newsletter channel.
async fn handle_ban_event(
    http: std::sync::Arc<serenity::Http>,
    newsletter_db: &sea_orm::DatabaseConnection,
    template: &EmbedTemplate,
    ban: &BanRecord,
    event_type: BanEventType,
) -> Result<(), AppError> {
    let started_at = Instant::now();
    trace!("broadcast_ban started at {started_at:?} for ban {}", ban.id);

    let channels = match list_registered_channels(newsletter_db).await {
        Ok(channels) => channels,
        Err(source) => {
            error!("failed to list registered channels: {source}");
            return Ok(());
        }
    };

    if channels.is_empty() {
        warn!("no registered channels available for ban {}", ban.id);
        return Ok(());
    }

    let mut tasks = JoinSet::new();
    for channel in channels {
        let http = http.clone();
        let newsletter_db = newsletter_db.clone();
        let template = template.clone();
        let ban = ban.clone();
        tasks.spawn(async move {
            let channel_id = channel.channel_id;
            let embed = format_ban_embed_for_locale(
                &template,
                &ban,
                channel.channel_locale.as_deref(),
                channel.user_locale.as_deref(),
                channel.guild_locale.as_deref(),
            );
            match event_type {
                BanEventType::Added => {
                    debug!("sending new ban {} to channel {}", ban.id, channel_id);
                    match serenity::ChannelId::new(channel_id)
                        .send_message(&http, serenity::CreateMessage::new().embed(embed.clone()))
                        .await
                    {
                        Ok(message) => {
                            save_ban_message(&newsletter_db, ban.id, channel_id, message.id.get())
                                .await
                                .map_err(|source| AppError::SaveBanMessage {
                                    ban_id: ban.id,
                                    channel_id,
                                    source,
                                })?;
                            info!("sent ban {} embed to channel {}", ban.id, channel_id);
                        }
                        Err(source) => error!(
                            "failed to send ban {} embed to channel {}: {source}",
                            ban.id, channel_id
                        ),
                    }
                }
                BanEventType::Edited => {
                    let message_id = match get_ban_message_id(&newsletter_db, ban.id, channel_id)
                        .await
                        .map_err(|source| AppError::LoadBanMessage {
                            ban_id: ban.id,
                            channel_id,
                            source,
                        })? {
                        Some(message_id) => message_id,
                        None => {
                            warn!(
                                "no existing message mapping for edited ban {} in channel {}",
                                ban.id, channel_id
                            );
                            return Ok(());
                        }
                    };
                    debug!(
                        "editing ban {} message {} in channel {}",
                        ban.id, message_id, channel_id
                    );
                    if let Err(source) = serenity::ChannelId::new(channel_id)
                        .edit_message(
                            &http,
                            serenity::MessageId::new(message_id),
                            serenity::EditMessage::new().embed(embed.clone()),
                        )
                        .await
                    {
                        error!(
                            "failed to edit ban {} message {} in channel {}: {source}",
                            ban.id, message_id, channel_id
                        );
                    } else {
                        info!("edited ban {} embed in channel {}", ban.id, channel_id);
                    }
                }
            }
            Ok::<(), AppError>(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(source)) => return Err(source),
            Err(source) => error!("channel handler task failed to join: {source}"),
        }
    }
    debug!("broadcast_ban finished in {:?}", started_at.elapsed());
    Ok(())
}

async fn process_ban_event(
    connection: &crate::app::types::Connection,
    ban_id: i32,
    event_type: BanEventType,
) -> Result<(), AppError> {
    let ban = load_ban_record(&connection.db, &connection.ban_source, ban_id)
        .await?
        .ok_or_else(|| {
            warn!("ban {ban_id} not found");
            AppError::BanNotFound { ban_id }
        })?;
    let template = connection.embed_template.read().await.clone();
    handle_ban_event(
        connection.discord_http.clone(),
        &connection.newsletter_db,
        &template,
        &ban,
        event_type,
    )
    .await?;
    clear_event(&connection.db, ban_id)
        .await
        .map_err(|source| AppError::ClearEvent { ban_id, source })?;
    Ok(())
}

/// Runs the main event loop that waits for shutdown or new ban notifications.
pub(crate) async fn run() -> Result<(), AppError> {
    let started_at = Instant::now();
    trace!("run started at {started_at:?}");

    trace!("initializing logger");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .init();

    info!("starting main event loop");
    let mut connection = init().await?;
    let pending_events = load_pending_events(&connection.db, &connection.enabled_event_types)
        .await
        .map_err(|source| AppError::LoadPendingEvents { source })?;
    for (ban_id, event_type) in pending_events {
        info!("processing pending event {event_type:?} for ban id {ban_id}");
        process_ban_event(&connection, ban_id, event_type).await?;
    }
    loop {
        trace!("waiting for shutdown signal or notification");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                trace!("shutdown signal task completed");
                result?;
                info!("shutdown signal received");
                break;
            }
            notif = connection.listener.recv() => {
                let notif_started_at = Instant::now();
                debug!("notification received from listener at {notif_started_at:?}");
                let notif = notif?;
                trace!("channel: {}", notif.channel());
                let Some(event_type) = BanEventType::from_channel(notif.channel()) else {
                    warn!(
                        "received notification on unsupported channel `{}`, ignoring",
                        notif.channel()
                    );
                    continue;
                };
                if !connection.enabled_event_types.contains(&event_type) {
                    debug!(
                        "received {event_type:?} on `{}` but event is disabled by EVENT_NAMES, skipping",
                        notif.channel()
                    );
                    continue;
                }
                let payload = notif.payload();
                debug!("notification payload received");
                let ban_id: i32 = payload.parse().map_err(|source| {
                    warn!("failed to parse ban id payload `{payload}`");
                    AppError::ParseBanId {
                        payload: payload.to_owned(),
                        source,
                    }
                })?;
                info!("processing {event_type:?} event for ban id {ban_id}");
                process_ban_event(&connection, ban_id, event_type).await?;
                debug!(
                    "ban {} handled successfully in {:?}",
                    ban_id,
                    notif_started_at.elapsed()
                );
            }
        }
    }
    info!("leaving main event loop");
    info!("shutting down Discord shards");
    connection.discord_shard_manager.shutdown_all().await;
    if let Err(source) = connection.discord_task.await {
        error!("Discord task join failed: {source}");
    }
    close(&connection.newsletter_db).await;
    close(&connection.db).await;
    debug!("run completed in {:?}", started_at.elapsed());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::BanEventType;

    #[test]
    fn parse_enabled_accepts_notification_channel_names() {
        let enabled = BanEventType::parse_enabled("ban_added");
        assert!(enabled.contains(&BanEventType::Added));
        assert!(!enabled.contains(&BanEventType::Edited));
    }

    #[test]
    fn parse_enabled_accepts_enum_values_and_is_case_insensitive() {
        let enabled = BanEventType::parse_enabled("Added, BAN_EDITED");
        assert!(enabled.contains(&BanEventType::Added));
        assert!(enabled.contains(&BanEventType::Edited));
    }
}

use crate::entity::{self, prelude::Bans};
use log::{debug, error, info, trace, warn};
use poise::serenity_prelude as serenity;
use sea_orm::{ConnectionTrait as _, DbBackend, EntityTrait as _, Statement, Value};
use std::time::Instant;

use crate::app::{
    db::close,
    discord::list_registered_channels,
    embed::EmbedTemplate,
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
}

fn reason_display(ban: &entity::bans::Model) -> String {
    if ban.reason.is_empty() {
        "No reason provided".to_owned()
    } else {
        ban.reason.to_string()
    }
}

/// Expands a template fragment by replacing known `{field}` placeholders.
fn render_template_text(template: &str, ban: &entity::bans::Model) -> String {
    template
        .replace("{id}", &ban.id.to_string())
        .replace("{intruder}", &ban.intruder)
        .replace("{admin}", &ban.admin)
        .replace("{type}", &ban.r#type)
        .replace("{round_id}", &ban.round_id.to_string())
        .replace("{server}", &ban.server)
        .replace("{duration_end}", &ban.duration_end.to_string())
        .replace("{reason}", &ban.reason)
        .replace("{reason_display}", &reason_display(ban))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BanEventType {
    Added,
    Edited,
}

impl BanEventType {
    fn from_channel(channel: &str) -> Option<Self> {
        match channel {
            "ban_added" => Some(Self::Added),
            "ban_edited" => Some(Self::Edited),
            _ => None,
        }
    }

    fn from_db_value(event: &str) -> Option<Self> {
        match event {
            "added" => Some(Self::Added),
            "edited" => Some(Self::Edited),
            _ => None,
        }
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
            events.push((ban_id, event));
        } else {
            warn!("unknown pending event `{event_type}` for ban {ban_id}, skipping");
        }
    }
    Ok(events)
}

/// Formats a rich ban announcement embed for Discord newsletters.
fn format_ban_embed(template: &EmbedTemplate, ban: &entity::bans::Model) -> serenity::CreateEmbed {
    let started_at = Instant::now();
    trace!(
        "format_ban_embed started at {started_at:?} for ban {}",
        ban.id
    );

    let embed = serenity::CreateEmbed::new()
        .title(render_template_text(&template.title, ban))
        .description(render_template_text(&template.description, ban))
        .color(serenity::Color::new(template.color));
    let embed = template.lines.iter().fold(embed, |embed, line| {
        embed.field(
            render_template_text(&line.title, ban),
            render_template_text(&line.value, ban),
            false,
        )
    });

    debug!(
        "formatted ban embed for {} in {:?}",
        ban.id,
        started_at.elapsed()
    );
    embed
}

/// Sends the latest ban information to every registered newsletter channel.
async fn handle_ban_event(
    http: &serenity::Http,
    newsletter_db: &sea_orm::DatabaseConnection,
    template: &EmbedTemplate,
    ban: &entity::bans::Model,
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

    let embed = format_ban_embed(template, ban);
    for channel_id in channels {
        match event_type {
            BanEventType::Added => {
                debug!("sending new ban {} to channel {}", ban.id, channel_id);
                match serenity::ChannelId::new(channel_id)
                    .send_message(http, serenity::CreateMessage::new().embed(embed.clone()))
                    .await
                {
                    Ok(message) => {
                        save_ban_message(newsletter_db, ban.id, channel_id, message.id.get())
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
                let Some(message_id) = get_ban_message_id(newsletter_db, ban.id, channel_id)
                    .await
                    .map_err(|source| AppError::LoadBanMessage {
                        ban_id: ban.id,
                        channel_id,
                        source,
                    })?
                else {
                    warn!(
                        "no existing message mapping for edited ban {} in channel {}",
                        ban.id, channel_id
                    );
                    continue;
                };
                debug!(
                    "editing ban {} message {} in channel {}",
                    ban.id, message_id, channel_id
                );
                if let Err(source) = serenity::ChannelId::new(channel_id)
                    .edit_message(
                        http,
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
    }
    debug!("broadcast_ban finished in {:?}", started_at.elapsed());
    Ok(())
}

async fn process_ban_event(
    connection: &crate::app::types::Connection,
    ban_id: i32,
    event_type: BanEventType,
) -> Result<(), AppError> {
    let ban = Bans::find_by_id(ban_id)
        .one(&connection.db)
        .await
        .map_err(|source| {
            error!("database query failed for ban id {ban_id}");
            AppError::QueryBan { ban_id, source }
        })?
        .ok_or_else(|| {
            warn!("ban {ban_id} not found");
            AppError::BanNotFound { ban_id }
        })?;
    handle_ban_event(
        &connection.discord_http,
        &connection.newsletter_db,
        &connection.embed_template,
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
    let pending_events = load_pending_events(&connection.db)
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
                    warn!("received notification on unexpected channel `{}`", notif.channel());
                    continue;
                };
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

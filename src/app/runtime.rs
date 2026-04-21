use crate::entity::{self, prelude::Bans};
use blake3::Hasher;
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

    #[error("failed to persist ban hash for ban id {ban_id}")]
    BanHashStore {
        ban_id: i32,
        #[source]
        source: sea_orm::DbErr,
    },
}

fn reason_display(ban: &entity::bans::Model) -> String {
    if ban.reason.is_empty() {
        "No reason provided".to_owned()
    } else {
        format!("{}", ban.reason)
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

fn hash_field(hasher: &mut Hasher, value: &[u8]) {
    let len = value.len() as u64;
    hasher.update(&len.to_le_bytes());
    hasher.update(value);
}

fn compute_ban_hash(ban: &entity::bans::Model) -> String {
    let mut hasher = Hasher::new();
    hash_field(&mut hasher, ban.id.to_string().as_bytes());
    hash_field(&mut hasher, ban.intruder.as_bytes());
    hash_field(&mut hasher, ban.admin.as_bytes());
    hash_field(&mut hasher, ban.r#type.as_bytes());
    hash_field(&mut hasher, ban.round_id.to_string().as_bytes());
    hash_field(&mut hasher, ban.server.as_bytes());
    hash_field(&mut hasher, ban.duration_end.to_string().as_bytes());
    hash_field(&mut hasher, ban.reason.as_bytes());
    hasher.finalize().to_hex().to_string()
}

async fn record_ban_hash_if_new(
    newsletter_db: &sea_orm::DatabaseConnection,
    ban: &entity::bans::Model,
) -> Result<bool, sea_orm::DbErr> {
    let ban_hash = compute_ban_hash(ban);
    let result = newsletter_db
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT OR IGNORE INTO sent_ban_hashes (ban_hash) VALUES (?)",
            vec![Value::String(Some(Box::new(ban_hash)))],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
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
async fn broadcast_ban(
    http: &serenity::Http,
    newsletter_db: &sea_orm::DatabaseConnection,
    template: &EmbedTemplate,
    ban: &entity::bans::Model,
) {
    let started_at = Instant::now();
    trace!("broadcast_ban started at {started_at:?} for ban {}", ban.id);

    let channels = match list_registered_channels(newsletter_db).await {
        Ok(channels) => channels,
        Err(source) => {
            error!("failed to list registered channels: {source}");
            return;
        }
    };

    if channels.is_empty() {
        warn!("no registered channels available for ban {}", ban.id);
        return;
    }

    let embed = format_ban_embed(template, ban);
    for channel_id in channels {
        debug!("sending ban {} to channel {}", ban.id, channel_id);
        if let Err(source) = serenity::ChannelId::new(channel_id)
            .send_message(http, serenity::CreateMessage::new().embed(embed.clone()))
            .await
        {
            error!(
                "failed to send ban {} embed to channel {}: {source}",
                ban.id, channel_id
            );
        } else {
            info!("sent ban {} embed to channel {}", ban.id, channel_id);
        }
    }
    debug!("broadcast_ban finished in {:?}", started_at.elapsed());
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
                let payload = notif.payload();
                debug!("notification payload received");
                let ban_id: i32 = payload.parse().map_err(|source| {
                    warn!("failed to parse ban id payload `{payload}`");
                    AppError::ParseBanId {
                        payload: payload.to_owned(),
                        source,
                    }
                })?;
                info!("processing ban id {ban_id}");
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
                info!("new ban: {:#?}", ban);
                let is_new = record_ban_hash_if_new(&connection.newsletter_db, &ban)
                    .await
                    .map_err(|source| AppError::BanHashStore { ban_id, source })?;
                if !is_new {
                    info!("duplicate ban {} detected from hash table; skipping broadcast", ban_id);
                    continue;
                }
                broadcast_ban(
                    &connection.discord_http,
                    &connection.newsletter_db,
                    &connection.embed_template,
                    &ban,
                )
                .await;
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

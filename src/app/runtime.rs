use crate::entity::{self, prelude::Bans};
use log::{debug, error, info, trace, warn};
use poise::serenity_prelude as serenity;
use sea_orm::EntityTrait as _;
use std::fmt::Write as _;

use crate::app::{
    db::close,
    discord::list_registered_channels,
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
}

/// Formats a detailed ban announcement for Discord newsletters.
fn format_ban_message(ban: &entity::bans::Model) -> String {
    trace!("formatting ban message for ban {}", ban.id);
    let mut text = String::new();
    let _ = writeln!(&mut text, "🚨 **new ban**");
    let _ = writeln!(&mut text, "- id: `{}`", ban.id);
    let _ = writeln!(&mut text, "- intruder: `{}`", ban.intruder);
    let _ = writeln!(&mut text, "- admin: `{}`", ban.admin);
    let _ = writeln!(&mut text, "- type: `{}`", ban.r#type);
    let _ = writeln!(&mut text, "- round: `{}`", ban.round_id);
    let _ = writeln!(&mut text, "- server: `{}`", ban.server);
    let _ = writeln!(&mut text, "- ends: `{}`", ban.duration_end);
    let _ = writeln!(&mut text, "- reason: `{}`", ban.reason);
    text
}

/// Sends the latest ban information to every registered newsletter channel.
async fn broadcast_ban(
    http: &serenity::Http,
    newsletter_db: &sea_orm::DatabaseConnection,
    ban: &entity::bans::Model,
) {
    trace!("broadcasting ban {} to registered channels", ban.id);
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

    let message = format_ban_message(ban);
    for channel_id in channels {
        debug!("sending ban {} to channel {}", ban.id, channel_id);
        if let Err(source) = serenity::ChannelId::new(channel_id)
            .say(http, &message)
            .await
        {
            error!(
                "failed to send ban {} message to channel {}: {source}",
                ban.id, channel_id
            );
        } else {
            info!("sent ban {} message to channel {}", ban.id, channel_id);
        }
    }
}

/// Runs the main event loop that waits for shutdown or new ban notifications.
pub(crate) async fn run() -> Result<(), AppError> {
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
                debug!("notification received from listener");
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
                broadcast_ban(&connection.discord_http, &connection.newsletter_db, &ban).await;
                trace!("ban {ban_id} handled successfully");
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
    Ok(())
}

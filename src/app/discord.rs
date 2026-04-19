use std::sync::Arc;

use log::{debug, error, info, trace, warn};
use poise::serenity_prelude as serenity;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use tokio::sync::oneshot;

/// Error type used by Discord command handlers.
type DiscordCommandError = Box<dyn std::error::Error + Send + Sync>;

/// Shared data available to all Discord slash commands.
#[derive(Clone)]
pub(super) struct DiscordData {
    /// SQLite connection for newsletter registration records.
    pub(super) newsletter_db: DatabaseConnection,
}

/// Runtime handles exported by the Discord bot bootstrap code.
pub(super) struct DiscordRuntime {
    /// HTTP client used to send messages to channels.
    pub(super) http: Arc<serenity::Http>,
    /// Shard manager used for graceful shutdown.
    pub(super) shard_manager: Arc<serenity::ShardManager>,
    /// Background task running the Serenity client.
    pub(super) task: tokio::task::JoinHandle<()>,
}

/// Poise command context type alias.
type DiscordContext<'a> = poise::Context<'a, DiscordData, DiscordCommandError>;

/// Registers the current channel in the SQLite newsletter table.
#[poise::command(slash_command)]
pub(super) async fn register(ctx: DiscordContext<'_>) -> Result<(), DiscordCommandError> {
    trace!("register command invoked");
    let channel_id = ctx.channel_id().get();
    debug!("attempting to register channel id {channel_id}");
    register_channel(&ctx.data().newsletter_db, channel_id).await?;
    info!("registered channel {channel_id} for newsletters");
    ctx.say("✅ This channel is now registered for ban newsletters.")
        .await?;
    Ok(())
}

/// Removes the current channel from the SQLite newsletter table.
#[poise::command(slash_command)]
pub(super) async fn unregister(ctx: DiscordContext<'_>) -> Result<(), DiscordCommandError> {
    trace!("unregister command invoked");
    let channel_id = ctx.channel_id().get();
    debug!("attempting to unregister channel id {channel_id}");
    unregister_channel(&ctx.data().newsletter_db, channel_id).await?;
    info!("unregistered channel {channel_id} from newsletters");
    ctx.say("✅ This channel has been removed from ban newsletters.")
        .await?;
    Ok(())
}

/// Starts the Discord bot, registers slash commands, and returns runtime handles.
pub(super) async fn start_discord_bot(
    token: &str,
    newsletter_db: DatabaseConnection,
) -> Result<DiscordRuntime, serenity::Error> {
    info!("creating Discord framework");
    let (http_tx, http_rx) = oneshot::channel();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![register(), unregister()],
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let db_for_setup = newsletter_db.clone();
            let http_sender = std::sync::Mutex::new(Some(http_tx));
            Box::pin(async move {
                info!("Discord bot logged in as {}", ready.user.name);
                trace!("registering slash commands globally");
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                debug!("slash commands registered globally");
                if let Some(sender) = http_sender.lock().expect("poisoned mutex").take() {
                    if sender.send(ctx.http().clone()).is_err() {
                        warn!("failed to send Discord HTTP handle to runtime receiver");
                    }
                }
                Ok(DiscordData {
                    newsletter_db: db_for_setup,
                })
            })
        })
        .build();

    debug!("building serenity client");
    let intents = serenity::GatewayIntents::non_privileged();
    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await?;

    let shard_manager = client.shard_manager.clone();
    let task = tokio::spawn(async move {
        info!("starting Discord client task");
        if let Err(source) = client.start().await {
            error!("Discord client stopped with error: {source}");
        }
        info!("Discord client task terminated");
    });

    let http = http_rx.await.map_err(|source| {
        error!("failed to receive Discord HTTP handle from setup: {source}");
        serenity::Error::Other("discord setup channel unexpectedly closed")
    })?;

    info!("Discord runtime handles ready");
    Ok(DiscordRuntime {
        http,
        shard_manager,
        task,
    })
}

/// Inserts a channel into the newsletter registration table.
pub(super) async fn register_channel(
    db: &DatabaseConnection,
    channel_id: u64,
) -> Result<(), sea_orm::DbErr> {
    trace!("inserting channel {channel_id} into newsletter_channels");
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT OR IGNORE INTO newsletter_channels (channel_id) VALUES (?)",
        vec![Value::BigUnsigned(Some(channel_id))],
    ))
    .await?;
    debug!("channel {channel_id} insert completed");
    Ok(())
}

/// Deletes a channel from the newsletter registration table.
pub(super) async fn unregister_channel(
    db: &DatabaseConnection,
    channel_id: u64,
) -> Result<(), sea_orm::DbErr> {
    trace!("deleting channel {channel_id} from newsletter_channels");
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "DELETE FROM newsletter_channels WHERE channel_id = ?",
        vec![Value::BigUnsigned(Some(channel_id))],
    ))
    .await?;
    debug!("channel {channel_id} delete completed");
    Ok(())
}

/// Reads all registered channel IDs from SQLite.
pub(super) async fn list_registered_channels(
    db: &DatabaseConnection,
) -> Result<Vec<u64>, sea_orm::DbErr> {
    trace!("querying registered newsletter channels");
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT channel_id FROM newsletter_channels".to_owned(),
        ))
        .await?;

    let mut channels = Vec::with_capacity(rows.len());
    for row in rows {
        let value: i64 = row.try_get_by_index(0)?;
        match u64::try_from(value) {
            Ok(channel) => channels.push(channel),
            Err(source) => warn!("skipping invalid channel id {value}: {source}"),
        }
    }

    debug!("loaded {} registered newsletter channels", channels.len());
    Ok(channels)
}

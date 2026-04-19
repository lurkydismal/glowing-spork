use log::{debug, error, info, trace};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};

use crate::app::{
    db::db_connect,
    discord::start_discord_bot,
    listener::{ListenerCreateError, listener_create},
    types::Connection,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("DATABASE_URL is missing: {0}")]
    MissingDatabaseUrl(#[source] std::env::VarError),

    #[error("EVENT_NAMES is missing: {0}")]
    MissingEventNames(#[source] std::env::VarError),

    #[error("DISCORD_TOKEN is missing: {0}")]
    MissingDiscordToken(#[source] std::env::VarError),

    #[error("NEWSLETTER_DATABASE_URL is missing: {0}")]
    MissingNewsletterDatabaseUrl(#[source] std::env::VarError),

    #[error("failed to connect to database")]
    Db(#[from] DbErr),

    #[error("failed to create listener")]
    Listener(#[from] ListenerCreateError),

    #[error("failed to start discord bot")]
    Discord(#[from] poise::serenity_prelude::Error),
}

/// Ensures the SQLite table used by newsletter commands exists.
async fn ensure_newsletter_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    trace!("ensuring newsletter schema exists");
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "CREATE TABLE IF NOT EXISTS newsletter_channels (channel_id INTEGER PRIMARY KEY NOT NULL)"
            .to_owned(),
    ))
    .await?;
    debug!("newsletter schema is ready");
    Ok(())
}

/// Sets up the environment and constructs the runtime connection bundle.
pub(super) async fn init() -> Result<Connection, InitError> {
    info!("initializing application environment");

    // Read .env
    let _ = dotenvy::dotenv();

    debug!("loaded environment file if present");
    let url = std::env::var("DATABASE_URL").map_err(InitError::MissingDatabaseUrl)?;
    debug!("DATABASE_URL found");
    let events = std::env::var("EVENT_NAMES").map_err(InitError::MissingEventNames)?;
    debug!("EVENT_NAMES found");
    let discord_token = std::env::var("DISCORD_TOKEN").map_err(InitError::MissingDiscordToken)?;
    debug!("DISCORD_TOKEN found");
    let newsletter_url = std::env::var("NEWSLETTER_DATABASE_URL")
        .map_err(InitError::MissingNewsletterDatabaseUrl)?;
    debug!("NEWSLETTER_DATABASE_URL found");

    let events: Vec<String> = events.split_whitespace().map(str::to_owned).collect();
    debug!("parsed {} event names", events.len());

    let db = db_connect(&url).await?;
    let newsletter_db = db_connect(&newsletter_url).await?;
    if let Err(source) = ensure_newsletter_schema(&newsletter_db).await {
        error!("failed to prepare newsletter schema: {source}");
        return Err(source.into());
    }

    let listener = listener_create(&url, events).await?;
    let discord = start_discord_bot(&discord_token, newsletter_db.clone()).await?;

    info!("application environment initialized");
    Ok(Connection {
        db,
        newsletter_db,
        listener,
        discord_http: discord.http,
        discord_shard_manager: discord.shard_manager,
        discord_task: discord.task,
    })
}

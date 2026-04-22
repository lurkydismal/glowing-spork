use log::{debug, error, info, trace};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use std::{path::PathBuf, time::Instant};

use crate::app::{
    db::db_connect,
    discord::start_discord_bot,
    embed::{EmbedTemplate, EmbedTemplateError},
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

    #[error("failed to read embed XML file `{path}`: {source}")]
    ReadEmbedFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid embed XML in `{path}`: {source}")]
    InvalidEmbedXml {
        path: PathBuf,
        #[source]
        source: EmbedTemplateError,
    },

    #[error("failed to connect to database: {0}")]
    Db(#[from] DbErr),

    #[error("failed to create listener: {0}")]
    Listener(#[from] ListenerCreateError),

    #[error("failed to start discord bot: {0}")]
    Discord(#[from] poise::serenity_prelude::Error),
}

/// Ensures the SQLite table used by newsletter commands exists.
async fn ensure_newsletter_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    let started_at = Instant::now();
    trace!("ensure_newsletter_schema started at {started_at:?}");
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "CREATE TABLE IF NOT EXISTS newsletter_channels (
            channel_id INTEGER PRIMARY KEY NOT NULL,
            user_locale TEXT,
            guild_locale TEXT
        )"
        .to_owned(),
    ))
    .await?;
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "ALTER TABLE newsletter_channels ADD COLUMN user_locale TEXT".to_owned(),
    ))
    .await
    .ok();
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "ALTER TABLE newsletter_channels ADD COLUMN guild_locale TEXT".to_owned(),
    ))
    .await
    .ok();
    db.execute(Statement::from_string(
        DbBackend::Sqlite,
        "CREATE TABLE IF NOT EXISTS ban_messages (ban_id INTEGER NOT NULL, channel_id INTEGER NOT NULL, message_id INTEGER NOT NULL, PRIMARY KEY (ban_id, channel_id))"
            .to_owned(),
    ))
    .await?;
    debug!("newsletter schema is ready in {:?}", started_at.elapsed());
    Ok(())
}

/// Ensures PostgreSQL objects for ban event delivery are present.
async fn ensure_ban_events_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        "DO $$
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'ban_event_type') THEN
                CREATE TYPE ban_event_type AS ENUM ('added', 'edited');
            END IF;
        END;
        $$"
        .to_owned(),
    ))
    .await?;

    db.execute(Statement::from_string(
        DbBackend::Postgres,
        "CREATE TABLE IF NOT EXISTS ban_events (
            ban_id INTEGER PRIMARY KEY,
            event_type ban_event_type NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
        .to_owned(),
    ))
    .await?;

    db.execute(Statement::from_string(
        DbBackend::Postgres,
        "CREATE OR REPLACE FUNCTION notify_ban_events()
        RETURNS TRIGGER
        LANGUAGE plpgsql
        AS $$
        DECLARE
            event_name ban_event_type;
        BEGIN
            IF TG_OP = 'INSERT' THEN
                event_name := 'added'::ban_event_type;
            ELSIF TG_OP = 'UPDATE' THEN
                event_name := 'edited'::ban_event_type;
            ELSE
                RETURN NEW;
            END IF;
            INSERT INTO ban_events (ban_id, event_type)
            VALUES (NEW.id, event_name)
            ON CONFLICT (ban_id) DO NOTHING;
            IF event_name = 'added' THEN
                PERFORM pg_notify('ban_added', NEW.id::text);
            ELSE
                PERFORM pg_notify('ban_edited', NEW.id::text);
            END IF;
            RETURN NEW;
        END;
        $$"
        .to_owned(),
    ))
    .await?;

    db.execute(Statement::from_string(
        DbBackend::Postgres,
        "DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1
                FROM pg_trigger
                WHERE tgname = 'bans_notify_events_trigger'
            ) THEN
                CREATE TRIGGER bans_notify_events_trigger
                AFTER INSERT OR UPDATE ON bans
                FOR EACH ROW
                EXECUTE FUNCTION notify_ban_events();
            END IF;
        END;
        $$"
        .to_owned(),
    ))
    .await?;

    Ok(())
}

/// Loads and validates the embed XML template from the `EMBED_FILE` environment variable.
///
/// Falls back to the default template when `EMBED_FILE` is not set.
fn load_embed_template() -> Result<EmbedTemplate, InitError> {
    let started_at = Instant::now();
    trace!("load_embed_template started at {started_at:?}");

    let embed_path = match std::env::var("EMBED_FILE") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            debug!(
                "EMBED_FILE was not provided; using default template (loaded in {:?})",
                started_at.elapsed()
            );
            return Ok(EmbedTemplate::default_template());
        }
    };

    let xml = std::fs::read_to_string(&embed_path).map_err(|source| InitError::ReadEmbedFile {
        path: embed_path.clone(),
        source,
    })?;

    let template = EmbedTemplate::from_xml(&xml).map_err(|source| InitError::InvalidEmbedXml {
        path: embed_path,
        source,
    })?;
    debug!(
        "loaded and validated EMBED_FILE in {:?}",
        started_at.elapsed()
    );
    Ok(template)
}

/// Sets up the environment and constructs the runtime connection bundle.
pub(super) async fn init() -> Result<Connection, InitError> {
    let started_at = Instant::now();
    info!("init started at {started_at:?}");

    // Read .env
    let _ = dotenvy::dotenv();

    debug!("DB: {:#?}", std::env::var("DATABASE_URL"));

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

    let embed_template = load_embed_template()?;

    let mut events: Vec<String> = events.split_whitespace().map(str::to_owned).collect();
    for channel in ["ban_added", "ban_edited"] {
        if !events.iter().any(|event| event == channel) {
            events.push(channel.to_owned());
        }
    }
    debug!("parsed {} event names", events.len());

    let db = db_connect(&url).await?;
    ensure_ban_events_schema(&db).await?;
    let newsletter_db = db_connect(&newsletter_url).await?;
    if let Err(source) = ensure_newsletter_schema(&newsletter_db).await {
        error!("failed to prepare newsletter schema: {source}");
        return Err(source.into());
    }

    let listener = listener_create(&url, events).await?;
    let discord = start_discord_bot(&discord_token, newsletter_db.clone()).await?;

    info!(
        "application environment initialized in {:?}",
        started_at.elapsed()
    );
    Ok(Connection {
        db,
        newsletter_db,
        listener,
        discord_http: discord.http,
        discord_shard_manager: discord.shard_manager,
        discord_task: discord.task,
        embed_template,
    })
}

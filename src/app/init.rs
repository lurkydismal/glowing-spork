use log::{debug, error, info, trace};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use tokio::sync::RwLock;

use crate::app::{
    db::db_connect,
    discord::start_discord_bot,
    embed::{EmbedTemplate, EmbedTemplateError},
    listener::{ListenerCreateError, listener_create},
    runtime::{BanEventType, BanSource},
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

    #[error("invalid SQL identifier in {field}: `{value}`")]
    InvalidIdentifier { field: &'static str, value: String },

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

    #[error("failed to watch embed XML file `{path}`: {source}")]
    WatchEmbedFile {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },

    #[error("failed to connect to database: {0}")]
    Db(#[from] DbErr),

    #[error("failed to create listener: {0}")]
    Listener(#[from] ListenerCreateError),

    #[error("failed to start discord bot: {0}")]
    Discord(#[from] poise::serenity_prelude::Error),

    #[error("failed to prompt for schema creation: {0}")]
    PromptIo(#[from] io::Error),

    #[error("schema creation canceled by user for object `{object_name}`")]
    SchemaCreationDeclined { object_name: String },
}

/// Prompt operator confirmation before creating a missing DB object.
///
/// This prompt can be skipped with `AUTO_CONFIRM_SCHEMA_CHANGES=true`.
#[allow(clippy::result_large_err)]
fn confirm_schema_create(object_name: &str) -> Result<(), InitError> {
    if std::env::var("AUTO_CONFIRM_SCHEMA_CHANGES")
        .is_ok_and(|value| value.eq_ignore_ascii_case("true") || value == "1")
    {
        debug!("AUTO_CONFIRM_SCHEMA_CHANGES=true; auto-approving creation of `{object_name}`");
        return Ok(());
    }

    print!("Create missing database object `{object_name}`? [y/n]: ");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(());
    }

    Err(InitError::SchemaCreationDeclined {
        object_name: object_name.to_owned(),
    })
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
            guild_locale TEXT,
            channel_locale TEXT
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
        "ALTER TABLE newsletter_channels ADD COLUMN channel_locale TEXT".to_owned(),
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

/// Ensures the SQLite table used by newsletter commands exists with user confirmation.
async fn ensure_newsletter_schema_with_prompt(db: &DatabaseConnection) -> Result<(), InitError> {
    let existing = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type='table' AND name='newsletter_channels'"
                .to_owned(),
        ))
        .await?;
    if existing.is_empty() {
        confirm_schema_create("sqlite table newsletter_channels")?;
    }

    let existing = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT name FROM sqlite_master WHERE type='table' AND name='ban_messages'".to_owned(),
        ))
        .await?;
    if existing.is_empty() {
        confirm_schema_create("sqlite table ban_messages")?;
    }

    ensure_newsletter_schema(db).await?;
    Ok(())
}

/// Ensures PostgreSQL objects for ban event delivery are present.
async fn ensure_ban_events_schema(
    db: &DatabaseConnection,
    ban_source: &BanSource,
) -> Result<(), InitError> {
    let id_col = quote_identifier(&ban_source.id_col);
    let table = quote_table_name(&ban_source.table);
    let trigger_name = format!(
        "{}_notify_events_trigger",
        ban_source.table.replace('.', "_")
    );
    let event_type_exists = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 FROM pg_type WHERE typname = 'ban_event_type'".to_owned(),
        ))
        .await?;
    if event_type_exists.is_empty() {
        confirm_schema_create("postgres enum ban_event_type")?;
    }

    let event_table_exists = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = 'ban_events'"
                .to_owned(),
        ))
        .await?;
    if event_table_exists.is_empty() {
        confirm_schema_create("postgres table ban_events")?;
    }

    let event_fn_exists = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 FROM pg_proc WHERE proname = 'notify_ban_events'".to_owned(),
        ))
        .await?;
    if event_fn_exists.is_empty() {
        confirm_schema_create("postgres function notify_ban_events")?;
    }

    let trigger_exists = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT 1 FROM pg_trigger WHERE tgname = '{trigger_name}'"),
        ))
        .await?;
    if trigger_exists.is_empty() {
        confirm_schema_create(&format!("postgres trigger {trigger_name}"))?;
    }

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
        format!(
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
            VALUES (NEW.{id_col}, event_name)
            ON CONFLICT (ban_id) DO NOTHING;
            IF event_name = 'added' THEN
                PERFORM pg_notify('ban_added', NEW.{id_col}::text);
            ELSE
                PERFORM pg_notify('ban_edited', NEW.{id_col}::text);
            END IF;
            RETURN NEW;
        END;
        $$"
        ),
    ))
    .await?;

    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(
            "DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1
                FROM pg_trigger
                WHERE tgname = '{trigger_name}'
            ) THEN
                CREATE TRIGGER {trigger_name}
                AFTER INSERT OR UPDATE ON {table}
                FOR EACH ROW
                EXECUTE FUNCTION notify_ban_events();
            END IF;
        END;
        $$"
        ),
    ))
    .await?;

    Ok(())
}

fn validate_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_table_name(table: &str) -> String {
    table
        .split('.')
        .map(quote_identifier)
        .collect::<Vec<_>>()
        .join(".")
}

fn read_env_or_default(key: &'static str, default: &'static str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[allow(clippy::result_large_err)]
fn load_ban_source_from_env() -> Result<BanSource, InitError> {
    let source = BanSource {
        table: read_env_or_default("BANS_TABLE", "bans"),
        id_col: read_env_or_default("BANS_COL_ID", "id"),
        intruder_col: read_env_or_default("BANS_COL_INTRUDER", "intruder"),
        admin_col: read_env_or_default("BANS_COL_ADMIN", "admin"),
        kind_col: read_env_or_default("BANS_COL_TYPE", "type"),
        round_id_col: read_env_or_default("BANS_COL_ROUND_ID", "round_id"),
        server_col: read_env_or_default("BANS_COL_SERVER", "server"),
        duration_end_col: read_env_or_default("BANS_COL_DURATION_END", "duration_end"),
        reason_col: read_env_or_default("BANS_COL_REASON", "reason"),
    };

    for (field, value) in [
        ("BANS_TABLE", source.table.as_str()),
        ("BANS_COL_ID", source.id_col.as_str()),
        ("BANS_COL_INTRUDER", source.intruder_col.as_str()),
        ("BANS_COL_ADMIN", source.admin_col.as_str()),
        ("BANS_COL_TYPE", source.kind_col.as_str()),
        ("BANS_COL_ROUND_ID", source.round_id_col.as_str()),
        ("BANS_COL_SERVER", source.server_col.as_str()),
        ("BANS_COL_DURATION_END", source.duration_end_col.as_str()),
        ("BANS_COL_REASON", source.reason_col.as_str()),
    ] {
        if !validate_identifier(value) {
            return Err(InitError::InvalidIdentifier {
                field,
                value: value.to_owned(),
            });
        }
    }

    Ok(source)
}

/// Loads and validates the embed XML template from the `EMBED_FILE` environment variable.
///
/// Falls back to the default template when `EMBED_FILE` is not set.
#[allow(clippy::result_large_err)]
fn load_embed_template() -> Result<(EmbedTemplate, Option<PathBuf>), InitError> {
    let started_at = Instant::now();
    trace!("load_embed_template started at {started_at:?}");

    let embed_path = match std::env::var("EMBED_FILE") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            debug!(
                "EMBED_FILE was not provided; using default template (loaded in {:?})",
                started_at.elapsed()
            );
            return Ok((EmbedTemplate::default_template(), None));
        }
    };

    let xml = std::fs::read_to_string(&embed_path).map_err(|source| InitError::ReadEmbedFile {
        path: embed_path.clone(),
        source,
    })?;

    let template = EmbedTemplate::from_xml(&xml).map_err(|source| InitError::InvalidEmbedXml {
        path: embed_path.clone(),
        source,
    })?;
    debug!(
        "loaded and validated EMBED_FILE in {:?}",
        started_at.elapsed()
    );
    Ok((template, Some(embed_path)))
}

#[allow(clippy::result_large_err)]
fn start_embed_template_hot_reload(
    embed_path: PathBuf,
    embed_template: Arc<RwLock<EmbedTemplate>>,
) -> Result<RecommendedWatcher, InitError> {
    let watch_root = embed_path
        .parent()
        .map_or_else(|| embed_path.clone(), PathBuf::from);
    let watched_file_name = embed_path.file_name().map(|name| name.to_owned());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        if tx.send(result).is_err() {
            debug!("embed reload event receiver dropped");
        }
    })
    .map_err(|source| InitError::WatchEmbedFile {
        path: embed_path.clone(),
        source,
    })?;

    watcher
        .watch(&watch_root, RecursiveMode::NonRecursive)
        .map_err(|source| InitError::WatchEmbedFile {
            path: embed_path.clone(),
            source,
        })?;
    info!(
        "enabled EMBED_FILE hot reload for {:?} (watching {:?})",
        embed_path, watch_root
    );

    tokio::spawn(async move {
        while let Some(result) = rx.recv().await {
            match result {
                Ok(event)
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    ) && event.paths.iter().any(|path| {
                        path == &embed_path
                            || (path.file_name().is_some()
                                && watched_file_name
                                    .as_ref()
                                    .is_some_and(|name| path.file_name() == Some(name.as_ref())))
                    }) =>
                {
                    match std::fs::read_to_string(&embed_path) {
                        Ok(xml) => match EmbedTemplate::from_xml(&xml) {
                            Ok(next_template) => {
                                *embed_template.write().await = next_template;
                                info!("reloaded EMBED_FILE template from {:?}", embed_path);
                            }
                            Err(source) => {
                                error!(
                                    "failed to parse reloaded EMBED_FILE {:?}: {source}",
                                    embed_path
                                );
                            }
                        },
                        Err(source) => {
                            error!(
                                "failed to read reloaded EMBED_FILE {:?}: {source}",
                                embed_path
                            );
                        }
                    }
                }
                Ok(_) => {}
                Err(source) => error!("embed file watcher error for {:?}: {source}", embed_path),
            }
        }
    });

    Ok(watcher)
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
    let event_names = std::env::var("EVENT_NAMES").map_err(InitError::MissingEventNames)?;
    debug!("EVENT_NAMES found");
    let discord_token = std::env::var("DISCORD_TOKEN").map_err(InitError::MissingDiscordToken)?;
    debug!("DISCORD_TOKEN found");
    let newsletter_url = std::env::var("NEWSLETTER_DATABASE_URL")
        .map_err(InitError::MissingNewsletterDatabaseUrl)?;
    debug!("NEWSLETTER_DATABASE_URL found");
    let ban_source = load_ban_source_from_env()?;
    debug!("ban source table: {}", ban_source.table);

    let (embed_template, embed_path) = load_embed_template()?;
    let embed_template = Arc::new(RwLock::new(embed_template));
    let embed_template_watcher = match embed_path {
        Some(path) => Some(start_embed_template_hot_reload(
            path,
            embed_template.clone(),
        )?),
        None => None,
    };

    let enabled_event_types = BanEventType::parse_enabled(&event_names);
    let listener_channels = BanEventType::listener_channels();
    debug!(
        "parsed {} enabled event names from EVENT_NAMES",
        enabled_event_types.len()
    );

    let db = db_connect(&url).await?;
    ensure_ban_events_schema(&db, &ban_source).await?;
    let newsletter_db = db_connect(&newsletter_url).await?;
    if let Err(source) = ensure_newsletter_schema_with_prompt(&newsletter_db).await {
        error!("failed to prepare newsletter schema: {source}");
        return Err(match source {
            InitError::Db(source) => source.into(),
            other => other,
        });
    }

    let listener = listener_create(&url, listener_channels).await?;
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
        embed_template_watcher,
        ban_source,
        enabled_event_types,
    })
}

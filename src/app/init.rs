use log::{debug, info};
use sea_orm::DbErr;

use crate::app::{
    db::db_connect,
    listener::{ListenerCreateError, listener_create},
    types::Connection,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("DATABASE_URL is missing: {0}")]
    MissingDatabaseUrl(#[source] std::env::VarError),

    #[error("EVENT_NAMES is missing: {0}")]
    MissingEventNames(#[source] std::env::VarError),

    #[error("failed to connect to database")]
    Db(#[from] DbErr),

    #[error("failed to create listener")]
    Listener(#[from] ListenerCreateError),
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
    let events: Vec<String> = events.split_whitespace().map(str::to_owned).collect();
    debug!("parsed {} event names", events.len());

    // TODO: Discord things from env

    let db = db_connect(&url).await?;
    let listener = listener_create(&url, events).await?;
    info!("application environment initialized");
    Ok(Connection { db, listener })
}

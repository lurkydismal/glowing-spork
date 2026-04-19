use glowing_spork::entity;
use log::{debug, error, info, trace, warn};
use sea_orm::{
    ConnAcquireErr, Database, DatabaseConnection, DbErr, EntityTrait as _, SqlxError,
    sqlx::postgres::PgListener,
};

/// Verifies that the database connection is alive and accepting requests.
async fn check(db: &DatabaseConnection) {
    trace!("starting database health check");
    debug!("sending ping to database");
    assert!(db.ping().await.is_ok());
    trace!("database health check succeeded");
}

/// Closes the database connection and verifies that it is no longer usable.
async fn close(db: &DatabaseConnection) {
    debug!("closing database connection");
    let _ = db.clone().close().await;
    trace!("database close request completed");
    assert!(matches!(
        db.ping().await,
        Err(DbErr::ConnectionAcquire(ConnAcquireErr::ConnectionClosed))
    ));
    info!("database connection confirmed closed");
}

/// Connects to the database and performs a basic health check.
async fn db_connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    trace!("initializing test logger");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .init();

    debug!("attempting database connection");
    info!("connecting to database");
    // Use a SQLite in memory database so no setup needed.
    // SeaORM supports MySQL, Postgres, SQL Server as well.
    match Database::connect(url).await {
        Ok(db) => {
            info!("database connection established");
            check(&db).await;
            debug!("database connection verified");
            Ok(db)
        }
        Err(err) => {
            error!("database connection failed: {err}");
            unimplemented!("{err}")
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ListenerCreateError {
    #[error("failed to connect to PostgreSQL listener at {url}")]
    Connect {
        url: String,
        #[source]
        source: SqlxError,
    },

    #[error("failed to subscribe to channel `{channel}`")]
    Listen {
        channel: String,
        #[source]
        source: SqlxError,
    },
}

/// Creates a PostgreSQL listener and subscribes it to the requested channels.
async fn listener_create<I, S>(url: &str, events: I) -> Result<PgListener, ListenerCreateError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    info!("creating PostgreSQL listener");
    trace!("connecting listener to database");
    let mut listener =
        PgListener::connect(url)
            .await
            .map_err(|source| ListenerCreateError::Connect {
                url: url.to_owned(),
                source,
            })?;

    debug!("listener connected");
    for event in events {
        let channel = event.as_ref();
        info!("subscribing to channel `{channel}`");
        listener
            .listen(channel)
            .await
            .map_err(|source| ListenerCreateError::Listen {
                channel: channel.to_owned(),
                source,
            })?;
        trace!("subscribed to channel `{channel}`");
    }

    info!("listener ready");
    Ok(listener)
}

/// Bundles the database connection and PostgreSQL listener used by the app.
struct Connection {
    db: DatabaseConnection,
    listener: PgListener,
    // TODO: Discord
}

/// Sets up the environment and constructs the runtime connection bundle.
async fn init() -> Connection {
    info!("initializing application environment");
    // Read .env
    let _ = dotenvy::dotenv();
    debug!("loaded environment file if present");

    let url = match std::env::var("DATABASE_URL") {
        Ok(url) => {
            debug!("DATABASE_URL found");
            url
        }
        Err(err) => {
            error!("DATABASE_URL is missing: {err}");
            unimplemented!("{err}")
        }
    };

    // TODO: Discord things from env

    let db = match db_connect(&url).await {
        Ok(db) => db,
        Err(err) => {
            error!("database initialization failed: {err}");
            unimplemented!("{err}")
        }
    };

    let listener = match listener_create(&url, ["bans_inserted"]).await {
        Ok(db) => db,
        Err(err) => {
            error!("listener initialization failed: {err}");
            unimplemented!("{err}")
        }
    };

    info!("application environment initialized");
    Connection { db, listener }
}

#[derive(Debug, thiserror::Error)]
enum AppError {
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

#[tokio::main]
async fn main() {
    info!("application starting");
    if let Err(err) = run().await {
        error!("{err}");
        std::process::exit(1);
    }
    info!("application exited cleanly");
}

/// Runs the main event loop that waits for shutdown or new ban notifications.
async fn run() -> Result<(), AppError> {
    info!("starting main event loop");
    let mut connection = init().await;

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

                let ban = entity::bans::Entity::find_by_id(ban_id)
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

                info!("new ban: {:?}", ban);
                trace!("ban {ban_id} handled successfully");
            }
        }
    }

    info!("leaving main event loop");
    close(&connection.db).await;
    Ok(())
}

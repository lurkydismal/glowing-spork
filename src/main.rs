use glowing_spork::entity;
use log::{debug, error, info, trace, warn};
use sea_orm::{
    ConnAcquireErr, Database, DatabaseConnection, DbErr, EntityTrait as _, SqlxError,
    sqlx::postgres::PgListener,
};

async fn check(db: &DatabaseConnection) {
    assert!(db.ping().await.is_ok());
}

async fn close(db: &DatabaseConnection) {
    let _ = db.clone().close().await;
    assert!(matches!(
        db.ping().await,
        Err(DbErr::ConnectionAcquire(ConnAcquireErr::ConnectionClosed))
    ));
}

async fn db_connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .init();

    // Use a SQLite in memory database so no setup needed.
    // SeaORM supports MySQL, Postgres, SQL Server as well.
    match Database::connect(url).await {
        Ok(db) => {
            check(&db).await;
            Ok(db)
        }
        Err(err) => unimplemented!("{err}"),
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

async fn listener_create<I, S>(url: &str, events: I) -> Result<PgListener, ListenerCreateError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut listener =
        PgListener::connect(url)
            .await
            .map_err(|source| ListenerCreateError::Connect {
                url: url.to_owned(),
                source,
            })?;
    for event in events {
        let channel = event.as_ref();
        listener
            .listen(channel)
            .await
            .map_err(|source| ListenerCreateError::Listen {
                channel: channel.to_owned(),
                source,
            })?;
    }
    Ok(listener)
}

struct Connection {
    db: DatabaseConnection,
    listener: PgListener,
    // TODO: Discord
}

/// Setup Environment
async fn init() -> Connection {
    // Read .env
    let _ = dotenvy::dotenv();

    let url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(err) => unimplemented!("{err}"),
    };

    // TODO: Discord things from env

    let db = match db_connect(&url).await {
        Ok(db) => db,
        Err(err) => unimplemented!("{err}"),
    };
    let listener = match listener_create(&url, ["bans_inserted"]).await {
        Ok(db) => db,
        Err(err) => unimplemented!("{err}"),
    };
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
    if let Err(err) = run().await {
        error!("{err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AppError> {
    let mut connection = init().await;
    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result?;
                info!("shutdown signal received");
                break;
            }
            notif = connection.listener.recv() => {
                let notif = notif?;
                trace!("Channel: {}", notif.channel());
                let payload = notif.payload();
                let ban_id: i32 = payload.parse().map_err(|source| AppError::ParseBanId {
                    payload: payload.to_owned(),
                    source,
                })?;
                let ban = entity::bans::Entity::find_by_id(ban_id)
                    .one(&connection.db)
                .await.map_err(|source| AppError::QueryBan { ban_id, source })?
                    .ok_or(AppError::BanNotFound { ban_id })?;
                info!("new ban: {:?}", ban);
            }
        }
    }
    close(&connection.db).await;
    Ok(())
}

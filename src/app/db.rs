use log::{debug, info, trace};
use sea_orm::{ConnAcquireErr, Database, DatabaseConnection, DbErr};

/// Verifies that the database connection is alive and accepting requests.
async fn check(db: &DatabaseConnection) {
    trace!("starting database health check");
    debug!("sending ping to database");
    assert!(db.ping().await.is_ok());
    trace!("database health check succeeded");
}

/// Closes the database connection and verifies that it is no longer usable.
pub(super) async fn close(db: &DatabaseConnection) {
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
pub(super) async fn db_connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    debug!("attempting database connection");
    info!("connecting to database");

    // Use a SQLite in memory database so no setup needed.
    // SeaORM supports MySQL, Postgres, SQL Server as well.
    let db = Database::connect(url).await?;
    info!("database connection established");
    check(&db).await;
    debug!("database connection verified");
    Ok(db)
}

use log::{debug, info, trace};
use sea_orm::{ConnAcquireErr, Database, DatabaseConnection, DbErr};
use std::time::Instant;

/// Verifies that the database connection is alive and accepting requests.
async fn check(db: &DatabaseConnection) {
    let started_at = Instant::now();
    trace!("check started at {started_at:?}");
    debug!("sending ping to database");
    assert!(db.ping().await.is_ok());
    trace!(
        "database health check succeeded in {:?}",
        started_at.elapsed()
    );
}

/// Closes the database connection and verifies that it is no longer usable.
pub(super) async fn close(db: &DatabaseConnection) {
    let started_at = Instant::now();
    debug!("close started at {started_at:?}");
    let _ = db.clone().close().await;
    trace!("database close request completed");
    assert!(matches!(
        db.ping().await,
        Err(DbErr::ConnectionAcquire(ConnAcquireErr::ConnectionClosed))
    ));
    info!(
        "database connection confirmed closed in {:?}",
        started_at.elapsed()
    );
}

/// Connects to the database and performs a basic health check.
pub(super) async fn db_connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    let started_at = Instant::now();
    debug!("db_connect started at {started_at:?}");
    info!("connecting to database");

    // SeaORM supports MySQL, Postgres, SQL Server as well.
    let db = Database::connect(url).await?;
    info!("database connection established");
    check(&db).await;
    debug!("database connection verified in {:?}", started_at.elapsed());
    Ok(db)
}

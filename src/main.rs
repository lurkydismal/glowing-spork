use glowing_spork::entity;
use log::{debug, error, info, trace, warn};
use sea_orm::{
    ConnAcquireErr, Database, DatabaseConnection, DbErr, EntityTrait as _,
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
    // TODO: Decide if is better than tracing
    // This disables sqlx's logging and enables sea-orm's logging with parameter injection,
    // which is easier to debug.
    // let env = env_logger::Env::default().filter_or("RUST_LOG", "info,sea_orm=debug,sqlx=warn");
    // env_logger::Builder::from_env(env).init();

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
        Err(err) => Err(err),
    }
}

async fn listener_create(url: &str) -> Result<PgListener, sea_orm::sqlx::Error> {
    let mut listener = PgListener::connect(url).await?; // SQLx
    listener.listen("bans_inserted").await?;
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

    let listener = match listener_create(&url).await {
        Ok(db) => db,
        Err(err) => unimplemented!("{err}"),
    };

    Connection { db, listener }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = init().await;

    loop {
        let notif = connection.listener.recv().await?;
        let ban_id: i32 = notif.payload().parse()?;

        let ban = entity::bans::Entity::find_by_id(ban_id)
            .one(&connection.db)
            .await?;

        info!("new ban: {:?}", ban);
    }

    // WARN: Loop never exits
    // close(&connection.db);
    //
    // Ok(())
}

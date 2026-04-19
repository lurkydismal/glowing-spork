use sea_orm::{DatabaseConnection, sqlx::postgres::PgListener};

/// Bundles the database connection and PostgreSQL listener used by the app.
pub(super) struct Connection {
    pub(super) db: DatabaseConnection,
    pub(super) listener: PgListener,
    // TODO: Discord
}

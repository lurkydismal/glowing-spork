use std::collections::HashSet;
use std::sync::Arc;

use poise::serenity_prelude as serenity;
use sea_orm::DatabaseConnection;

use crate::app::embed::EmbedTemplate;
use crate::app::runtime::{BanEventType, BanSource};

/// Bundles runtime resources used by the application event loop.
pub(super) struct Connection {
    /// PostgreSQL connection used for bans lookup.
    pub(super) db: DatabaseConnection,
    /// SQLite connection used to store newsletter channel registrations.
    pub(super) newsletter_db: DatabaseConnection,
    /// PostgreSQL LISTEN/NOTIFY listener for ban events.
    pub(super) listener: sea_orm::sqlx::postgres::PgListener,
    /// Discord HTTP client for posting announcements.
    pub(super) discord_http: Arc<serenity::Http>,
    /// Handle used to shutdown Discord shards gracefully.
    pub(super) discord_shard_manager: Arc<serenity::ShardManager>,
    /// Background Discord client task.
    pub(super) discord_task: tokio::task::JoinHandle<()>,
    /// Message template used to format newsletter announcements.
    pub(super) embed_template: EmbedTemplate,
    /// Source table and columns used to load ban rows.
    pub(super) ban_source: BanSource,
    /// Ban events allowed to be processed from notifications.
    pub(super) enabled_event_types: HashSet<BanEventType>,
}

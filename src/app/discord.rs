use std::sync::Arc;

use log::{debug, error, info, trace, warn};
use poise::serenity_prelude as serenity;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, Value};
use std::time::Instant;
use tokio::sync::oneshot;

/// Error type used by Discord command handlers.
type DiscordCommandError = Box<dyn std::error::Error + Send + Sync>;

/// Shared data available to all Discord slash commands.
#[derive(Clone)]
pub(super) struct DiscordData {
    /// SQLite connection for newsletter registration records.
    pub(super) newsletter_db: DatabaseConnection,
}

/// Runtime handles exported by the Discord bot bootstrap code.
pub(super) struct DiscordRuntime {
    /// HTTP client used to send messages to channels.
    pub(super) http: Arc<serenity::Http>,
    /// Shard manager used for graceful shutdown.
    pub(super) shard_manager: Arc<serenity::ShardManager>,
    /// Background task running the Serenity client.
    pub(super) task: tokio::task::JoinHandle<()>,
}

/// Poise command context type alias.
type DiscordContext<'a> = poise::Context<'a, DiscordData, DiscordCommandError>;

/// Newsletter channel registration with locale preferences collected from Discord interactions.
#[derive(Clone, Debug)]
pub(super) struct NewsletterChannel {
    /// Discord channel ID that receives ban newsletter messages.
    pub(super) channel_id: u64,
    /// User locale from the interaction that registered the channel.
    pub(super) user_locale: Option<String>,
    /// Guild locale from the interaction that registered the channel.
    pub(super) guild_locale: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum RegistrationAction {
    Register,
    Unregister,
}

impl RegistrationAction {
    fn log_verb(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Unregister => "unregister",
        }
    }
}

/// Registers the current channel in the SQLite newsletter table.
#[poise::command(slash_command)]
pub(super) async fn register(ctx: DiscordContext<'_>) -> Result<(), DiscordCommandError> {
    execute_registration_action(ctx, RegistrationAction::Register).await
}

/// Removes the current channel from the SQLite newsletter table.
#[poise::command(slash_command)]
pub(super) async fn unregister(ctx: DiscordContext<'_>) -> Result<(), DiscordCommandError> {
    execute_registration_action(ctx, RegistrationAction::Unregister).await
}

/// Executes a channel registration command while applying locale-aware confirmation text.
async fn execute_registration_action(
    ctx: DiscordContext<'_>,
    action: RegistrationAction,
) -> Result<(), DiscordCommandError> {
    let started_at = Instant::now();
    trace!("{} command invoked at {started_at:?}", action.log_verb());
    let channel_id = ctx.channel_id().get();
    debug!(
        "attempting to {} channel id {channel_id}",
        action.log_verb()
    );
    let (user_locale, guild_locale) = interaction_locales(&ctx).await;
    let translations =
        crate::app::i18n::resolve_translations(user_locale.as_deref(), guild_locale.as_deref());
    match action {
        RegistrationAction::Register => {
            register_channel(
                &ctx.data().newsletter_db,
                channel_id,
                &user_locale,
                &guild_locale,
            )
            .await?;
            info!("registered channel {channel_id} for newsletters");
            ctx.say(translations.register_success).await?;
        }
        RegistrationAction::Unregister => {
            unregister_channel(&ctx.data().newsletter_db, channel_id).await?;
            info!("unregistered channel {channel_id} from newsletters");
            ctx.say(translations.unregister_success).await?;
        }
    }
    debug!(
        "{} command finished in {:?}",
        action.log_verb(),
        started_at.elapsed()
    );
    Ok(())
}

/// Extracts normalized user and guild locales from an interaction context.
async fn interaction_locales(ctx: &DiscordContext<'_>) -> (Option<String>, Option<String>) {
    let user_locale = crate::app::i18n::normalize_locale(ctx.locale());
    let guild_locale = match ctx.guild_id() {
        Some(guild_id) => match guild_id.to_partial_guild(ctx.serenity_context()).await {
            Ok(guild) => crate::app::i18n::normalize_locale(Some(&guild.preferred_locale)),
            Err(source) => {
                warn!(
                    "failed to fetch preferred locale for guild {}: {source}",
                    guild_id
                );
                None
            }
        },
        None => None,
    };
    (user_locale, guild_locale)
}

/// Starts the Discord bot, registers slash commands, and returns runtime handles.
pub(super) async fn start_discord_bot(
    token: &str,
    newsletter_db: DatabaseConnection,
) -> Result<DiscordRuntime, serenity::Error> {
    let started_at = Instant::now();
    info!("start_discord_bot started at {started_at:?}");
    let (http_tx, http_rx) = oneshot::channel();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![register(), unregister()],
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let db_for_setup = newsletter_db.clone();
            let http_sender = std::sync::Mutex::new(Some(http_tx));
            Box::pin(async move {
                info!("Discord bot logged in as {}", ready.user.name);
                trace!("registering slash commands globally");
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                debug!("slash commands registered globally");
                if let Some(sender) = http_sender.lock().expect("poisoned mutex").take()
                    && sender.send(ctx.http.clone()).is_err()
                {
                    warn!("failed to send Discord HTTP handle to runtime receiver");
                }
                Ok(DiscordData {
                    newsletter_db: db_for_setup,
                })
            })
        })
        .build();

    debug!("building serenity client");
    let intents = serenity::GatewayIntents::non_privileged();
    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await?;

    let shard_manager = client.shard_manager.clone();
    let task = tokio::spawn(async move {
        info!("starting Discord client task");
        if let Err(source) = client.start().await {
            error!("Discord client stopped with error: {source}");
        }
        info!("Discord client task terminated");
    });

    let http = http_rx.await.map_err(|source| {
        error!("failed to receive Discord HTTP handle from setup: {source}");
        serenity::Error::Other("discord setup channel unexpectedly closed")
    })?;

    info!(
        "Discord runtime handles ready in {:?}",
        started_at.elapsed()
    );
    Ok(DiscordRuntime {
        http,
        shard_manager,
        task,
    })
}

/// Inserts a channel into the newsletter registration table.
pub(super) async fn register_channel(
    db: &DatabaseConnection,
    channel_id: u64,
    user_locale: &Option<String>,
    guild_locale: &Option<String>,
) -> Result<(), sea_orm::DbErr> {
    let started_at = Instant::now();
    trace!("register_channel started at {started_at:?} for {channel_id}");
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO newsletter_channels (channel_id, user_locale, guild_locale)
         VALUES (?, ?, ?)
         ON CONFLICT (channel_id) DO UPDATE SET
            user_locale = excluded.user_locale,
            guild_locale = excluded.guild_locale",
        vec![
            Value::BigUnsigned(Some(channel_id)),
            Value::String(user_locale.clone().map(Box::new)),
            Value::String(guild_locale.clone().map(Box::new)),
        ],
    ))
    .await?;
    debug!(
        "channel {channel_id} insert completed in {:?}",
        started_at.elapsed()
    );
    Ok(())
}

/// Deletes a channel from the newsletter registration table.
pub(super) async fn unregister_channel(
    db: &DatabaseConnection,
    channel_id: u64,
) -> Result<(), sea_orm::DbErr> {
    let started_at = Instant::now();
    trace!("unregister_channel started at {started_at:?} for {channel_id}");
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "DELETE FROM newsletter_channels WHERE channel_id = ?",
        vec![Value::BigUnsigned(Some(channel_id))],
    ))
    .await?;
    debug!(
        "channel {channel_id} delete completed in {:?}",
        started_at.elapsed()
    );
    Ok(())
}

/// Reads all registered channel IDs from SQLite.
pub(super) async fn list_registered_channels(
    db: &DatabaseConnection,
) -> Result<Vec<NewsletterChannel>, sea_orm::DbErr> {
    let started_at = Instant::now();
    trace!("list_registered_channels started at {started_at:?}");
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT channel_id, user_locale, guild_locale FROM newsletter_channels".to_owned(),
        ))
        .await?;

    let mut channels = Vec::with_capacity(rows.len());
    for row in rows {
        let value: i64 = row.try_get_by_index(0)?;
        let user_locale: Option<String> = row.try_get_by_index(1)?;
        let guild_locale: Option<String> = row.try_get_by_index(2)?;
        match u64::try_from(value) {
            Ok(channel) => channels.push(NewsletterChannel {
                channel_id: channel,
                user_locale: crate::app::i18n::normalize_locale(user_locale.as_deref()),
                guild_locale: crate::app::i18n::normalize_locale(guild_locale.as_deref()),
            }),
            Err(source) => warn!("skipping invalid channel id {value}: {source}"),
        }
    }

    debug!(
        "loaded {} registered newsletter channels in {:?}",
        channels.len(),
        started_at.elapsed()
    );
    Ok(channels)
}

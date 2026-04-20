use log::{debug, info, trace};
use sea_orm::{SqlxError, sqlx::postgres::PgListener};
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ListenerCreateError {
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
pub(super) async fn listener_create<I, S>(
    url: &str,
    events: I,
) -> Result<PgListener, ListenerCreateError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let started_at = Instant::now();
    info!("listener_create started at {started_at:?}");
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
    info!("listener ready in {:?}", started_at.elapsed());
    Ok(listener)
}

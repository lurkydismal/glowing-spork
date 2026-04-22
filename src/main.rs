//! Binary entrypoint for the Discord ban newsletter service.
//!
//! # Flow
//! 1. Initialize logging and runtime.
//! 2. Call [`app::run`] to start listeners and Discord bot.
//! 3. Exit with non-zero status when startup/runtime fails.

mod app;

use log::{debug, error, info};

#[tokio::main]
async fn main() {
    let started_at = std::time::Instant::now();
    info!("application starting at {started_at:?}");
    if let Err(err) = app::run().await {
        error!("{err}");
        std::process::exit(1);
    }
    debug!("application runtime {:?}", started_at.elapsed());
    info!("application exited cleanly");
}

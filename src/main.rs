mod app;
mod entity;

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

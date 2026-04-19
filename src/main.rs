mod app;

use log::{error, info};

#[tokio::main]
async fn main() {
    info!("application starting");
    if let Err(err) = app::run().await {
        error!("{err}");
        std::process::exit(1);
    }
    info!("application exited cleanly");
}

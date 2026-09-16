#![forbid(unsafe_code)]

use news::{config::Config, error::Result, media, runner, state::AppState, web};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    if std::env::args().nth(1).as_deref() == Some("configcheck") {
        Config::from_env()?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("archivecheck") {
        let count = news::travel::verify_identity_archives(&Config::from_env()?).await?;
        println!("Verified {count} permanent Along identity archives");
        return Ok(());
    }
    fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("news=info".parse().expect("valid log directive"))
                .add_directive(
                    "sqlx::postgres::notice=warn"
                        .parse()
                        .expect("valid log directive"),
                ),
        )
        .init();

    let state = AppState::new(Config::from_env()?).await?;
    // Refreshing expired images can involve several upstream requests. Start it in the
    // background so a slow tmpfile or news source never delays the HTTP listener.
    let startup_sync = tokio::spawn({
        let state = state.clone();
        async move {
            if let Err(error) = media::startup_sync(&state).await {
                tracing::warn!(%error, "image startup sync failed; continuing with available assets");
            }
        }
    });
    let mut background = runner::spawn(state.clone());
    let result = web::serve(state).await;
    startup_sync.abort();
    let _ = startup_sync.await;
    background.shutdown().await;
    result
}

#![forbid(unsafe_code)]

use news::{config::Config, error::Result, runner, state::AppState, web};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    if std::env::args().nth(1).as_deref() == Some("configcheck") {
        Config::from_env()?;
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
    let mut background = runner::spawn(state.clone());
    let result = web::serve(state).await;
    background.shutdown().await;
    result
}

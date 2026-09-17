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

    if std::env::args().nth(1).as_deref() == Some("archivereconcile") {
        let count = news::travel_image::reconcile_identity_archives(&Config::from_env()?).await?;
        println!("Verified and reconciled {count} suspended Along identity archives");
        return Ok(());
    }

    let state = AppState::new(Config::from_env()?).await?;
    let mut background = runner::spawn(state.clone());
    let result = web::serve(state).await;
    background.shutdown().await;
    result
}

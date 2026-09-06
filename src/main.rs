//! The program around the library: read the configuration, open the database,
//! find the identity provider, listen.
//!
//! Every step here fails closed. A forum that starts without knowing who
//! anyone is would be worse than one that does not start.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version") {
        println!("treff {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("treff: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("treff: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn env_path(name: &str, fallback: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| fallback.to_string()))
}

async fn serve() -> anyhow::Result<()> {
    let config_path = std::env::var("TREFF_CONFIG").map_err(|_| {
        anyhow::anyhow!("TREFF_CONFIG is not set; treff does not start without a configuration")
    })?;
    let config = treff::config::Config::parse(&std::fs::read_to_string(&config_path)?)?;

    let data_dir = env_path("TREFF_DATA_DIR", "/var/lib/treff");
    std::fs::create_dir_all(&data_dir)?;
    let db = treff::db::Db::open(&data_dir.join("treff.db")).await?;

    let oidc = treff::auth::OidcSettings::from_env()?;
    let redirect_uri = std::env::var("TREFF_OIDC_REDIRECT_URI").map_err(|_| {
        anyhow::anyhow!("TREFF_OIDC_REDIRECT_URI is not set; the provider needs a way back")
    })?;

    // Discovery happens once, here: a provider that cannot be reached at
    // startup is a reason not to serve, not something to discover per request
    // and fail at halfway through a sign-in.
    let provider = treff::auth::oidc::Provider::discover(&oidc, &redirect_uri).await?;

    let state = treff::web::AppState::new(config, db, oidc, Some(Arc::new(provider)), &data_dir)?;
    let app = treff::web::router(state);

    let listen = std::env::var("TREFF_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("treff: listening on {listen}");
    axum::serve(listener, app).await?;
    Ok(())
}

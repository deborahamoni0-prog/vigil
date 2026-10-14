//! Structured logging with trace ids, so one liquidation can be followed across services (6.5).

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Installs a JSON subscriber. `RUST_LOG` controls the level and defaults to `info`.
pub fn init(service: &'static str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json().with_current_span(false))
        .init();
    tracing::info!(service, "telemetry initialised");
}

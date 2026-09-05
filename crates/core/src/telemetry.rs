//! Tracing setup. One trace id follows a request into its task and on into any model call,
//! and is returned to the client in error payloads and the `X-Trace-Id` header (NFR-O-03).

use crate::config::LogConfig;

/// Install the global subscriber. Called once, from a binary.
pub fn init(cfg: &LogConfig) -> crate::Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_env("JOBSEEKER_LOG")
        .or_else(|_| EnvFilter::try_new(&cfg.level))
        .map_err(|e| crate::Error::Config(format!("invalid log.level: {e}")))?;

    let registry = tracing_subscriber::registry().with(filter);
    match cfg.format.as_str() {
        "json" => registry
            .with(tracing_subscriber::fmt::layer().json().with_current_span(true))
            .try_init()
            .map_err(|e| crate::Error::Internal(e.to_string()))?,
        "pretty" | "text" => registry
            .with(tracing_subscriber::fmt::layer().with_target(false))
            .try_init()
            .map_err(|e| crate::Error::Internal(e.to_string()))?,
        other => {
            return Err(crate::Error::Config(format!(
                "log.format must be \"pretty\" or \"json\", got {other:?}"
            )))
        }
    }
    Ok(())
}

/// A short correlation id. Not a UUID because it is meant to be read aloud and grepped.
pub fn new_trace_id() -> String {
    let hex = blake3::hash(uuid::Uuid::now_v7().as_bytes()).to_hex();
    hex[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ids_are_short_and_unique() {
        let a = new_trace_id();
        let b = new_trace_id();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
    }
}

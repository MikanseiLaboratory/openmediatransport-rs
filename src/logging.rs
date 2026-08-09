//! Logging helpers via `tracing`.

/// Initialize a basic tracing subscriber if none is set (best-effort stub).
pub fn init_logging() {
    // Applications should configure tracing; this is a no-op placeholder.
    tracing::trace!("openmediatransport logging ready");
}

/// Log a debug message.
pub fn debug(msg: &str) {
    tracing::debug!("{msg}");
}

/// Log an info message.
pub fn info(msg: &str) {
    tracing::info!("{msg}");
}

/// Log a warning.
pub fn warn(msg: &str) {
    tracing::warn!("{msg}");
}

/// Log an error.
pub fn error(msg: &str) {
    tracing::error!("{msg}");
}

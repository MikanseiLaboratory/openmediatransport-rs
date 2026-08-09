//! Redirect / proxy helper stub.
#![allow(dead_code)]

use crate::error::OmtError;

/// Redirects an OMT stream from one address to another.
#[derive(Debug)]
pub struct Redirect {
    source: String,
    destination: String,
}

impl Redirect {
    /// Create a redirect from `source` to `destination`.
    pub fn new(source: impl Into<String>, destination: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            destination: destination.into(),
        }
    }

    /// Source address.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Destination address.
    pub fn destination(&self) -> &str {
        &self.destination
    }

    /// Run the redirect loop (stub).
    pub fn run(&mut self) -> Result<(), OmtError> {
        Err(OmtError::NotImplemented("Redirect::run"))
    }
}

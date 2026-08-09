//! Error types for Open Media Transport.

use thiserror::Error;

/// Errors produced by the OMT stack.
#[derive(Debug, Error)]
pub enum OmtError {
    /// Invalid argument or configuration.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Network / socket failure.
    #[error("network error: {0}")]
    Network(String),

    /// Protocol framing or parse error.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Discovery failure.
    #[error("discovery error: {0}")]
    Discovery(String),

    /// Codec failure.
    #[error("codec error: {0}")]
    Codec(String),

    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// VMX codec error.
    #[error("VMX error: {0}")]
    Vmx(#[from] vmx::VmxError),

    /// Feature not yet implemented (scaffold stub).
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
}

use thiserror::Error;
use tonic::Status;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("RPC error: {0}")]
    RpcError(#[from] Status),

    #[error("Rate limited - request denied")]
    RateLimited,

    #[error("Failed to reset limit")]
    ResetFailed,

    #[error("Invalid configuration: {0}")]
    ConfigError(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

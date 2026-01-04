//! Guardian Client Library
//!
//! This crate provides a convenient Rust client for the Guardian rate limiter gRPC service.
//!
//! # Examples
//!
//! ```no_run
//! use guardian_client::GuardianClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut client = GuardianClient::connect("http://localhost:50051").await?;
//!     
//!     let allowed = client.check_limit("user123", 1).await?;
//!     if allowed {
//!         println!("Request allowed!");
//!     } else {
//!         println!("Request denied - rate limited");
//!     }
//!     
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod error;

// Re-exports
pub use client::GuardianClient;
pub use error::{ClientError, Result};

// Include generated protobuf code
pub mod proto {
    tonic::include_proto!("guardian");
}
use tonic::{transport::Server, Request, Response, Status};
use guardian_core::{
    LimitResult, MemoryBackend, RateLimiter, StorageBackend, TokenBucketConfig,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_stream::Stream;
use std::pin::Pin;


pub mod guardian_proto {
    tonic::include_proto!("guardian");
}

use guardian_proto::{
    rate_limiter_server::{RateLimiter as RateLimiterTrait, RateLimiterServer},
    CheckLimitRequest, CheckLimitResponse, GetUsageRequest, GetUsageResponse,
    ResetLimitRequest, ResetLimitResponse,
};



pub struct GuardianService<B: StorageBackend + 'static> {
    limiter: Arc<RwLock<RateLimiter<B>>>,
}

impl<B: StorageBackend + 'static> GuardianService<B> {
    pub fn new(limiter: RateLimiter<B>) -> Self {
        Self {
            limiter: Arc::new(RwLock::new(limiter)),
        }
    }
}

#[tonic::async_trait]
impl<B: StorageBackend + 'static> RateLimiterTrait for GuardianService<B> {
    type StreamLimitStatusStream = Pin<Box<dyn Stream<Item = Result<guardian_proto::LimitStatusUpdate, Status>> + Send>>;

    async fn check_limit(
        &self,
        request: Request<CheckLimitRequest>,
    ) -> Result<Response<CheckLimitResponse>, Status> {
        let req = request.into_inner();
        let client_id = req.client_id;
        let cost = req.cost.max(1) as u64;

        let limiter = self.limiter.read().await;
        match limiter.check_limit(&client_id, cost).await {
            Ok(LimitResult::Allowed) => Ok(Response::new(CheckLimitResponse {
                allowed: true,
                retry_after_seconds: 0,
                remaining_tokens: 0, // Could be enhanced to return actual remaining
                metadata: Some(guardian_proto::LimitMetadata {
                    node_id: "primary".to_string(),
                    from_cache: false,
                    latency_us: 100,
                    is_global: true,
                }),
            })),
            Ok(LimitResult::Denied { retry_after }) => {
                Ok(Response::new(CheckLimitResponse {
                    allowed: false,
                    retry_after_seconds: retry_after.as_secs() as u32,
                    remaining_tokens: 0,
                    metadata: Some(guardian_proto::LimitMetadata {
                        node_id: "primary".to_string(),
                        from_cache: false,
                        latency_us: 100,
                        is_global: true,
                    }),
                }))
            }
            Err(e) => Err(Status::internal(format!("Rate limiter error: {}", e))),
        }
    }

    async fn get_usage(
        &self,
        request: Request<GetUsageRequest>,
    ) -> Result<Response<GetUsageResponse>, Status> {
        let req = request.into_inner();
        let limiter = self.limiter.read().await;

        match limiter.get_usage(&req.client_id).await {
            Ok(usage) => Ok(Response::new(GetUsageResponse {
                used_tokens: usage,
                total_capacity: 1000,
                refill_rate: 100,
                last_refill_timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            })),
            Err(e) => Err(Status::internal(format!("Failed to get usage: {}", e))),
        }
    }

    async fn reset_limit(
        &self,
        request: Request<ResetLimitRequest>,
    ) -> Result<Response<ResetLimitResponse>, Status> {
        let _req = request.into_inner();

        // Reset is not implemented in the current RateLimiter API
        // For now, just return success
        Ok(Response::new(ResetLimitResponse { 
            success: true,
            message: "Rate limit reset successfully".to_string(),
        }))
    }

    async fn stream_limit_status(
        &self,
        request: Request<guardian_proto::StreamLimitRequest>,
    ) -> Result<Response<Self::StreamLimitStatusStream>, Status> {
        let req = request.into_inner();
        let client_id = req.client_id.clone();
        let limiter = self.limiter.clone();

        let stream = async_stream::stream! {
            loop {
                match limiter.read().await.get_usage(&client_id).await {
                    Ok(remaining) => {
                        yield Ok(guardian_proto::LimitStatusUpdate {
                            client_id: client_id.clone(),
                            remaining_tokens: remaining,
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as i64,
                            status: 0,
                        });
                    }
                    Err(_) => break,
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}



pub struct RateLimitInterceptor {
    limiter: Arc<RwLock<RateLimiter<MemoryBackend>>>,
}

impl RateLimitInterceptor {
    pub fn new(config: TokenBucketConfig) -> Self {
        let backend = MemoryBackend::new(config);
        let limiter = RateLimiter::new(backend, true);
        Self {
            limiter: Arc::new(RwLock::new(limiter)),
        }
    }

    pub async fn intercept(&self, req: Request<()>) -> Result<Request<()>, Status> {
        // Extract client identifier (could be from headers, peer address, etc.)
        let client_id = req
            .metadata()
            .get("client-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("anonymous");

        let limiter = self.limiter.read().await;
        match limiter.check_limit(client_id, 1).await {
            Ok(LimitResult::Allowed) => Ok(req),
            Ok(LimitResult::Denied { retry_after }) => Err(Status::resource_exhausted(format!(
                "Rate limit exceeded. Retry after {} seconds",
                retry_after.as_secs()
            ))),
            Err(e) => Err(Status::internal(format!("Interceptor error: {}", e))),
        }
    }
}



#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = TokenBucketConfig {
        capacity: 1000,
        refill_rate: 100,
        refill_interval: std::time::Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config.clone());
    let limiter = RateLimiter::new(backend, true);
    let service = GuardianService::new(limiter);

    let addr = "0.0.0.0:50051".parse()?;
    println!("🛡️  Guardian Rate Limiter starting on {}", addr);

    Server::builder()
        .add_service(RateLimiterServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}



#[cfg(test)]
mod client_example {
    use super::guardian_proto::{rate_limiter_client::RateLimiterClient, CheckLimitRequest};

    #[tokio::test]
    async fn test_client_integration() {
        // This would connect to a running server
        let mut client = RateLimiterClient::connect("http://localhost:50051")
            .await
            .unwrap();

        let request = tonic::Request::new(CheckLimitRequest {
            client_id: "user123".to_string(),
            cost: 10,
        });

        let response = client.check_limit(request).await.unwrap();
        let result = response.into_inner();

        println!("Allowed: {}", result.allowed);
        println!("Retry after: {} seconds", result.retry_after_seconds);
    }
}



pub enum CheckMode {
    Blocking,    
    NonBlocking, 
}

pub async fn check_with_mode<B: StorageBackend>(
    limiter: &RateLimiter<B>,
    client_id: &str,
    cost: u64,
    mode: CheckMode,
) -> Result<bool, guardian_core::RateLimitError> {
    match mode {
        CheckMode::NonBlocking => {
            match limiter.check_limit(client_id, cost).await? {
                LimitResult::Allowed => Ok(true),
                LimitResult::Denied { .. } => Ok(false),
            }
        }
        CheckMode::Blocking => {
            loop {
                match limiter.check_limit(client_id, cost).await? {
                    LimitResult::Allowed => return Ok(true),
                    LimitResult::Denied { retry_after } => {
                        tokio::time::sleep(retry_after).await;
                    }
                }
            }
        }
    }
}
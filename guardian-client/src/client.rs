use tonic::transport::Channel;
use tonic::Request;

use crate::error::{ClientError, Result};
use crate::proto::{
    rate_limiter_client::RateLimiterClient,
    CheckLimitRequest, GetUsageRequest, ResetLimitRequest,
};

/// Guardian rate limiter client
pub struct GuardianClient {
    inner: RateLimiterClient<Channel>,
}

impl GuardianClient {
    /// Connect to a Guardian service at the given endpoint
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use guardian_client::GuardianClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = GuardianClient::connect("http://localhost:50051").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect<D>(dst: D) -> Result<Self>
    where
        D: TryInto<tonic::transport::Endpoint>,
        D::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let endpoint = dst.try_into().map_err(|e| {
            ClientError::ConnectionError(format!("Invalid endpoint: {:?}", e.into()))
        })?;

        let channel = endpoint
            .connect()
            .await
            .map_err(|e| ClientError::ConnectionError(e.to_string()))?;

        Ok(Self {
            inner: RateLimiterClient::new(channel),
        })
    }

    /// Check if a request should be allowed for the given client
    ///
    /// # Arguments
    ///
    /// * `client_id` - Unique identifier for the client (user_id, API key, IP, etc.)
    /// * `cost` - Cost of this request in tokens (default: 1)
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Request is allowed
    /// * `Ok(false)` - Request is denied (rate limited)
    /// * `Err(...)` - Communication or server error
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use guardian_client::GuardianClient;
    /// # async fn example(mut client: GuardianClient) -> Result<(), Box<dyn std::error::Error>> {
    /// if client.check_limit("user123", 1).await? {
    ///     // Process request
    ///     println!("Request allowed");
    /// } else {
    ///     // Reject request
    ///     println!("Rate limited");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn check_limit(&mut self, client_id: &str, cost: u32) -> Result<bool> {
        let request = Request::new(CheckLimitRequest {
            client_id: client_id.to_string(),
            cost,
            override_config: None,
        });

        let response = self
            .inner
            .check_limit(request)
            .await
            .map_err(|e| ClientError::RpcError(e))?;

        Ok(response.into_inner().allowed)
    }
    pub async fn check_limit_detailed(
        &mut self,
        client_id: &str,
        cost: u32,
    ) -> Result<LimitCheckResult> {
        let request = Request::new(CheckLimitRequest {
            client_id: client_id.to_string(),
            cost,
            override_config: None,
        });

        let response = self
            .inner
            .check_limit(request)
            .await
            .map_err(|e| ClientError::RpcError(e))?;

        let resp = response.into_inner();
        Ok(LimitCheckResult {
            allowed: resp.allowed,
            retry_after_seconds: resp.retry_after_seconds,
            remaining_tokens: resp.remaining_tokens,
        })
    }

    /// Get current usage statistics for a client
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use guardian_client::GuardianClient;
    /// # async fn example(mut client: GuardianClient) -> Result<(), Box<dyn std::error::Error>> {
    /// let usage = client.get_usage("user123").await?;
    /// println!("Used tokens: {}", usage);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_usage(&mut self, client_id: &str) -> Result<u64> {
        let request = Request::new(GetUsageRequest {
            client_id: client_id.to_string(),
        });

        let response = self
            .inner
            .get_usage(request)
            .await
            .map_err(|e| ClientError::RpcError(e))?;

        Ok(response.into_inner().used_tokens)
    }

    /// Reset the rate limit for a specific client (admin operation)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use guardian_client::GuardianClient;
    /// # async fn example(mut client: GuardianClient) -> Result<(), Box<dyn std::error::Error>> {
    /// client.reset_limit("user123").await?;
    /// println!("Limit reset successfully");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reset_limit(&mut self, client_id: &str) -> Result<()> {
        let request = Request::new(ResetLimitRequest {
            client_id: client_id.to_string(),
            admin_token: String::new(), // Add token if needed
        });

        let response = self
            .inner
            .reset_limit(request)
            .await
            .map_err(|e| ClientError::RpcError(e))?;

        if response.into_inner().success {
            Ok(())
        } else {
            Err(ClientError::ResetFailed)
        }
    }

    /// Execute a function only if rate limit allows
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use guardian_client::GuardianClient;
    /// # async fn example(mut client: GuardianClient) -> Result<(), Box<dyn std::error::Error>> {
    /// let result = client.with_rate_limit("user123", 1, async {
    ///     // Your protected operation here
    ///     println!("Executing protected operation");
    ///     Ok::<_, guardian_client::ClientError>("Success")
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn with_rate_limit<F, T>(
        &mut self,
        client_id: &str,
        cost: u32,
        f: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        if self.check_limit(client_id, cost).await? {
            f.await
        } else {
            Err(ClientError::RateLimited)
        }
    }
}

#[derive(Debug, Clone)]
pub struct LimitCheckResult {
    pub allowed: bool,
    pub retry_after_seconds: u32,
    pub remaining_tokens: u64,
}

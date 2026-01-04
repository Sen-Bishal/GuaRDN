// Guardian - High-Performance Distributed Rate Limiter
// File: guardian-core/src/lib.rs

use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use thiserror::Error;

// ============================================================================
// ERROR TYPES
// ============================================================================

#[derive(Error, Debug)]
pub enum RateLimitError {
    #[error("Rate limit exceeded for key: {0}")]
    LimitExceeded(String),
    #[error("Storage backend error: {0}")]
    StorageError(String),
    #[error("Invalid configuration: {0}")]
    ConfigError(String),
}

// ============================================================================
// CORE ALGORITHM: TOKEN BUCKET
// ============================================================================

#[derive(Debug, Clone)]
pub struct TokenBucketConfig {
    pub capacity: u64,
    pub refill_rate: u64,  // tokens per second
    pub refill_interval: Duration,
}

impl Default for TokenBucketConfig {
    fn default() -> Self {
        Self {
            capacity: 100,
            refill_rate: 10,
            refill_interval: Duration::from_secs(1),
        }
    }
}

pub struct TokenBucket {
    tokens: AtomicU64,
    capacity: u64,
    refill_rate: u64,
    last_refill: RwLock<SystemTime>,
}

impl TokenBucket {
    pub fn new(config: TokenBucketConfig) -> Self {
        Self {
            tokens: AtomicU64::new(config.capacity),
            capacity: config.capacity,
            refill_rate: config.refill_rate,
            last_refill: RwLock::new(SystemTime::now()),
        }
    }

    pub fn try_consume(&self, cost: u64) -> Result<(), RateLimitError> {
        self.refill();

        let mut current = self.tokens.load(Ordering::Acquire);
        loop {
            if current < cost {
                return Err(RateLimitError::LimitExceeded(
                    "Insufficient tokens".to_string(),
                ));
            }

            match self.tokens.compare_exchange_weak(
                current,
                current - cost,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    fn refill(&self) {
        let now = SystemTime::now();
        let mut last = self.last_refill.write();

        if let Ok(elapsed) = now.duration_since(*last) {
            let tokens_to_add = (elapsed.as_secs() * self.refill_rate)
                + (elapsed.subsec_millis() as u64 * self.refill_rate / 1000);

            if tokens_to_add > 0 {
                let current = self.tokens.load(Ordering::Acquire);
                let new_tokens = (current + tokens_to_add).min(self.capacity);
                self.tokens.store(new_tokens, Ordering::Release);
                *last = now;
            }
        }
    }

    pub fn available_tokens(&self) -> u64 {
        self.refill();
        self.tokens.load(Ordering::Acquire)
    }
}

// ============================================================================
// STORAGE BACKEND ABSTRACTION
// ============================================================================

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError>;
    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError>;
    async fn reset(&self, key: &str) -> Result<(), RateLimitError>;
}

// ============================================================================
// IN-MEMORY BACKEND (High Performance)
// ============================================================================

pub struct MemoryBackend {
    buckets: Arc<RwLock<HashMap<String, Arc<TokenBucket>>>>,
    config: TokenBucketConfig,
}

impl MemoryBackend {
    pub fn new(config: TokenBucketConfig) -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    fn get_or_create_bucket(&self, key: &str) -> Arc<TokenBucket> {
        let buckets = self.buckets.read();
        if let Some(bucket) = buckets.get(key) {
            return Arc::clone(bucket);
        }
        drop(buckets);

        let mut buckets = self.buckets.write();
        buckets
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(TokenBucket::new(self.config.clone())))
            .clone()
    }
}

#[async_trait]
impl StorageBackend for MemoryBackend {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError> {
        let bucket = self.get_or_create_bucket(key);
        match bucket.try_consume(cost) {
            Ok(_) => Ok(true),
            Err(RateLimitError::LimitExceeded(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError> {
        let bucket = self.get_or_create_bucket(key);
        Ok(self.config.capacity - bucket.available_tokens())
    }

    async fn reset(&self, key: &str) -> Result<(), RateLimitError> {
        let mut buckets = self.buckets.write();
        buckets.remove(key);
        Ok(())
    }
}

// ============================================================================
// BATCHING LAYER (Reduces distributed backend calls)
// ============================================================================

pub struct BatchingBackend<B: StorageBackend> {
    backend: Arc<B>,
    local_cache: Arc<RwLock<HashMap<String, LocalBatch>>>,
    batch_size: u64,
}

struct LocalBatch {
    available: AtomicU64,
    reserved_until: RwLock<SystemTime>,
}

impl<B: StorageBackend> BatchingBackend<B> {
    pub fn new(backend: B, batch_size: u64) -> Self {
        Self {
            backend: Arc::new(backend),
            local_cache: Arc::new(RwLock::new(HashMap::new())),
            batch_size,
        }
    }

    async fn reserve_batch(&self, key: &str) -> Result<(), RateLimitError> {
        // Try to reserve batch_size tokens from backend
        for _ in 0..self.batch_size {
            if !self.backend.take_token(key, 1).await? {
                return Err(RateLimitError::LimitExceeded(key.to_string()));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<B: StorageBackend> StorageBackend for BatchingBackend<B> {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError> {
        // Try local cache first (drop lock before await)
        {
            let cache = self.local_cache.read();
            if let Some(batch) = cache.get(key) {
                let current = batch.available.load(Ordering::Acquire);
                if current >= cost {
                    match batch.available.compare_exchange(
                        current,
                        current - cost,
                        Ordering::Release,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => return Ok(true),
                        Err(_) => {} // Retry with backend
                    }
                }
            }
        } // Lock dropped here

        // Need to reserve a new batch
        self.reserve_batch(key).await?;

        // Reacquire lock after await
        let mut cache = self.local_cache.write();
        let batch = cache.entry(key.to_string()).or_insert_with(|| LocalBatch {
            available: AtomicU64::new(self.batch_size),
            reserved_until: RwLock::new(SystemTime::now() + Duration::from_secs(60)),
        });

        let current = batch.available.load(Ordering::Acquire);
        if current >= cost {
            batch.available.store(current - cost, Ordering::Release);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError> {
        self.backend.get_usage(key).await
    }

    async fn reset(&self, key: &str) -> Result<(), RateLimitError> {
        {
            let mut cache = self.local_cache.write();
            cache.remove(key);
        } // Lock dropped here before await
        self.backend.reset(key).await
    }
}

// ============================================================================
// RATE LIMITER FACADE
// ============================================================================

pub struct RateLimiter<B: StorageBackend> {
    backend: Arc<B>,
    fail_open: bool,
}

impl<B: StorageBackend> RateLimiter<B> {
    pub fn new(backend: B, fail_open: bool) -> Self {
        Self {
            backend: Arc::new(backend),
            fail_open,
        }
    }

    pub async fn check_limit(
        &self,
        client_id: &str,
        cost: u64,
    ) -> Result<LimitResult, RateLimitError> {
        match self.backend.take_token(client_id, cost).await {
            Ok(true) => Ok(LimitResult::Allowed),
            Ok(false) => Ok(LimitResult::Denied {
                retry_after: Duration::from_secs(1),
            }),
            Err(e) => {
                if self.fail_open {
                    eprintln!("Rate limiter error (failing open): {}", e);
                    Ok(LimitResult::Allowed)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub async fn get_usage(&self, client_id: &str) -> Result<u64, RateLimitError> {
        self.backend.get_usage(client_id).await
    }
}

#[derive(Debug, PartialEq)]
pub enum LimitResult {
    Allowed,
    Denied { retry_after: Duration },
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[test]
    fn test_token_bucket_basic() {
        let config = TokenBucketConfig {
            capacity: 10,
            refill_rate: 5,
            refill_interval: Duration::from_secs(1),
        };
        let bucket = TokenBucket::new(config);

        assert!(bucket.try_consume(5).is_ok());
        assert!(bucket.try_consume(5).is_ok());
        assert!(bucket.try_consume(1).is_err());
    }

    #[tokio::test]
    async fn test_memory_backend() {
        let config = TokenBucketConfig::default();
        let backend = MemoryBackend::new(config);

        assert!(backend.take_token("user1", 10).await.unwrap());
        assert!(backend.take_token("user1", 50).await.unwrap());
        assert!(!backend.take_token("user1", 50).await.unwrap());
    }

    #[tokio::test]
    async fn test_rate_limiter_fail_open() {
        let config = TokenBucketConfig {
            capacity: 5,
            refill_rate: 1,
            refill_interval: Duration::from_secs(1),
        };
        let backend = MemoryBackend::new(config);
        let limiter = RateLimiter::new(backend, true);

        for _ in 0..5 {
            assert_eq!(
                limiter.check_limit("user1", 1).await.unwrap(),
                LimitResult::Allowed
            );
        }

        let result = limiter.check_limit("user1", 1).await.unwrap();
        assert!(matches!(result, LimitResult::Denied { .. }));
    }

    #[tokio::test]
    async fn test_token_refill() {
        let config = TokenBucketConfig {
            capacity: 10,
            refill_rate: 10,
            refill_interval: Duration::from_secs(1),
        };
        let bucket = TokenBucket::new(config);

        bucket.try_consume(10).unwrap();
        assert!(bucket.try_consume(1).is_err());

        sleep(Duration::from_millis(1100)).await;
        assert!(bucket.try_consume(10).is_ok());
    }
}
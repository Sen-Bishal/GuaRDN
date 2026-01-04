
use async_trait::async_trait;
use guardian_core::{RateLimitError, StorageBackend, TokenBucketConfig};
use redis::{aio::ConnectionManager, AsyncCommands, Client, Script};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};


pub struct RedisBackend {
    connection: Arc<ConnectionManager>,
    config: TokenBucketConfig,
    take_token_script: Script,
    get_usage_script: Script,
}

impl RedisBackend {
    pub async fn new(redis_url: &str, config: TokenBucketConfig) -> Result<Self, RateLimitError> {
        let client = Client::open(redis_url)
            .map_err(|e| RateLimitError::StorageError(format!("Redis client error: {}", e)))?;

        let connection = client
            .get_connection_manager()
            .await
            .map_err(|e| RateLimitError::StorageError(format!("Redis connection error: {}", e)))?;

        Ok(Self {
            connection: Arc::new(connection),
            config,
            take_token_script: Self::create_take_token_script(),
            get_usage_script: Self::create_get_usage_script(),
        })
    }


    fn create_take_token_script() -> Script {
        Script::new(
            r#"
            local key = KEYS[1]
            local capacity = tonumber(ARGV[1])
            local refill_rate = tonumber(ARGV[2])
            local cost = tonumber(ARGV[3])
            local now = tonumber(ARGV[4])
            
            -- Get current state
            local bucket = redis.call('HMGET', key, 'tokens', 'last_refill')
            local tokens = tonumber(bucket[1])
            local last_refill = tonumber(bucket[2])
            
            -- Initialize if doesn't exist
            if not tokens then
                tokens = capacity
                last_refill = now
            end
            
            -- Calculate refill
            if last_refill then
                local elapsed = now - last_refill
                local tokens_to_add = math.floor(elapsed * refill_rate)
                tokens = math.min(capacity, tokens + tokens_to_add)
                last_refill = now
            end
            
            -- Check if we can consume
            if tokens >= cost then
                tokens = tokens - cost
                redis.call('HMSET', key, 'tokens', tokens, 'last_refill', last_refill)
                redis.call('EXPIRE', key, 3600)  -- TTL: 1 hour
                return 1  -- Success
            else
                redis.call('HMSET', key, 'tokens', tokens, 'last_refill', last_refill)
                redis.call('EXPIRE', key, 3600)
                return 0  -- Denied
            end
            "#,
        )
    }

    fn create_get_usage_script() -> Script {
        Script::new(
            r#"
            local key = KEYS[1]
            local capacity = tonumber(ARGV[1])
            local refill_rate = tonumber(ARGV[2])
            local now = tonumber(ARGV[3])
            
            local bucket = redis.call('HMGET', key, 'tokens', 'last_refill')
            local tokens = tonumber(bucket[1]) or capacity
            local last_refill = tonumber(bucket[2]) or now
            
            -- Calculate current tokens with refill
            local elapsed = now - last_refill
            local tokens_to_add = math.floor(elapsed * refill_rate)
            tokens = math.min(capacity, tokens + tokens_to_add)
            
            return capacity - tokens
            "#,
        )
    }

    fn get_current_time() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }
}

#[async_trait]
impl StorageBackend for RedisBackend {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError> {
        let mut conn = self.connection.as_ref().clone();
        let now = Self::get_current_time();

        let result: i32 = self
            .take_token_script
            .key(key)
            .arg(self.config.capacity)
            .arg(self.config.refill_rate)
            .arg(cost)
            .arg(now)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| {
                RateLimitError::StorageError(format!("Redis script execution error: {}", e))
            })?;

        Ok(result == 1)
    }

    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError> {
        let mut conn = self.connection.as_ref().clone();
        let now = Self::get_current_time();

        let usage: u64 = self
            .get_usage_script
            .key(key)
            .arg(self.config.capacity)
            .arg(self.config.refill_rate)
            .arg(now)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| {
                RateLimitError::StorageError(format!("Redis script execution error: {}", e))
            })?;

        Ok(usage)
    }

    async fn reset(&self, key: &str) -> Result<(), RateLimitError> {
        let mut conn = self.connection.as_ref().clone();
        conn.del::<_, ()>(key)
            .await
            .map_err(|e| RateLimitError::StorageError(format!("Redis delete error: {}", e)))?;
        Ok(())
    }
}


pub struct RedisClusterBackend {
    connection: Arc<redis::cluster_async::ClusterConnection>,
    config: TokenBucketConfig,
    take_token_script: Script,
}

impl RedisClusterBackend {
    pub async fn new(
        nodes: Vec<String>,
        config: TokenBucketConfig,
    ) -> Result<Self, RateLimitError> {
        let client = redis::cluster::ClusterClient::new(nodes)
            .map_err(|e| RateLimitError::StorageError(format!("Cluster client error: {}", e)))?;

        let connection = client
            .get_async_connection()
            .await
            .map_err(|e| RateLimitError::StorageError(format!("Cluster connection error: {}", e)))?;

        Ok(Self {
            connection: Arc::new(connection),
            config,
            take_token_script: RedisBackend::create_take_token_script(),
        })
    }

    fn hash_key(&self, key: &str) -> String {
        // Use consistent hashing for cluster sharding
        format!("{{{}}}:ratelimit", key)
    }
}

#[async_trait]
impl StorageBackend for RedisClusterBackend {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError> {
        let hashed_key = self.hash_key(key);
        let mut conn = self.connection.as_ref().clone();
        let now = RedisBackend::get_current_time();

        let result: i32 = self
            .take_token_script
            .key(hashed_key)
            .arg(self.config.capacity)
            .arg(self.config.refill_rate)
            .arg(cost)
            .arg(now)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| {
                RateLimitError::StorageError(format!("Cluster script execution error: {}", e))
            })?;

        Ok(result == 1)
    }

    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError> {
        let hashed_key = self.hash_key(key);
        let mut conn = self.connection.as_ref().clone();

        let bucket: Option<(u64, f64)> = conn
            .hget(&hashed_key, &["tokens", "last_refill"])
            .await
            .map_err(|e| RateLimitError::StorageError(format!("Redis get error: {}", e)))?;

        match bucket {
            Some((tokens, _)) => Ok(self.config.capacity.saturating_sub(tokens)),
            None => Ok(0),
        }
    }

    async fn reset(&self, key: &str) -> Result<(), RateLimitError> {
        let hashed_key = self.hash_key(key);
        let mut conn = self.connection.as_ref().clone();

        conn.del::<_, ()>(hashed_key)
            .await
            .map_err(|e| RateLimitError::StorageError(format!("Redis delete error: {}", e)))?;
        Ok(())
    }
}


use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Instant;

pub struct CachedRedisBackend {
    redis: Arc<RedisBackend>,
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    cache_ttl: std::time::Duration,
}

struct CacheEntry {
    tokens: u64,
    expires_at: Instant,
}

impl CachedRedisBackend {
    pub fn new(redis: RedisBackend, cache_ttl: std::time::Duration) -> Self {
        Self {
            redis: Arc::new(redis),
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl,
        }
    }

    fn get_cached(&self, key: &str) -> Option<u64> {
        let cache = self.cache.read();
        cache.get(key).and_then(|entry| {
            if entry.expires_at > Instant::now() {
                Some(entry.tokens)
            } else {
                None
            }
        })
    }

    fn set_cache(&self, key: &str, tokens: u64) {
        let mut cache = self.cache.write();
        cache.insert(
            key.to_string(),
            CacheEntry {
                tokens,
                expires_at: Instant::now() + self.cache_ttl,
            },
        );
    }
}

#[async_trait]
impl StorageBackend for CachedRedisBackend {
    async fn take_token(&self, key: &str, cost: u64) -> Result<bool, RateLimitError> {
        // Try cache first
        if let Some(cached_tokens) = self.get_cached(key) {
            if cached_tokens >= cost {
                self.set_cache(key, cached_tokens - cost);
                return Ok(true);
            }
        }

        // Fallback to Redis
        let result = self.redis.take_token(key, cost).await?;
        if result {
            // Update cache with estimated remaining tokens
            self.set_cache(key, self.redis.config.capacity - cost);
        }
        Ok(result)
    }

    async fn get_usage(&self, key: &str) -> Result<u64, RateLimitError> {
        self.redis.get_usage(key).await
    }

    async fn reset(&self, key: &str) -> Result<(), RateLimitError> {
        {
            let mut cache = self.cache.write();
            cache.remove(key);
        } // Drop lock before await
        self.redis.reset(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires Redis instance
    async fn test_redis_backend() {
        let config = TokenBucketConfig {
            capacity: 100,
            refill_rate: 10,
            refill_interval: std::time::Duration::from_secs(1),
        };

        let backend = RedisBackend::new("redis://127.0.0.1", config)
            .await
            .unwrap();

        assert!(backend.take_token("test_user", 10).await.unwrap());
        let usage = backend.get_usage("test_user").await.unwrap();
        assert!(usage > 0);

        backend.reset("test_user").await.unwrap();
    }
}
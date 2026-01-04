use guardian_core::{
    BatchingBackend, LimitResult, MemoryBackend, RateLimiter, TokenBucketConfig,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

async fn example_simple_usage() {
    println!("=== Example 1: Simple Usage ===\n");

    let config = TokenBucketConfig {
        capacity: 10,
        refill_rate: 2,
        refill_interval: Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config);
    let limiter = RateLimiter::new(backend, false);

    // Make some requests
    for i in 1..=12 {
        match limiter.check_limit("user123", 1).await {
            Ok(LimitResult::Allowed) => {
                println!("✅ Request {} allowed", i);
            }
            Ok(LimitResult::Denied { retry_after }) => {
                println!(
                    "❌ Request {} denied - retry after {:?}",
                    i, retry_after
                );
            }
            Err(e) => println!("⚠️  Error: {}", e),
        }
    }

    println!("\n📊 Current usage: {}\n", limiter.get_usage("user123").await.unwrap());
}

async fn example_concurrent_users() {
    println!("=== Example 2: Concurrent Users ===\n");

    let config = TokenBucketConfig {
        capacity: 100,
        refill_rate: 10,
        refill_interval: Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config);
    let limiter = Arc::new(RateLimiter::new(backend, false));

    let users = vec!["alice", "bob", "charlie"];
    let mut handles = vec![];

    for user in users {
        let limiter = Arc::clone(&limiter);
        let handle = tokio::spawn(async move {
            let mut allowed = 0;
            let mut denied = 0;

            for _ in 0..50 {
                match limiter.check_limit(user, 1).await {
                    Ok(LimitResult::Allowed) => allowed += 1,
                    Ok(LimitResult::Denied { .. }) => denied += 1,
                    Err(e) => eprintln!("Error for {}: {}", user, e),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            (user, allowed, denied)
        });
        handles.push(handle);
    }

    for handle in handles {
        let (user, allowed, denied) = handle.await.unwrap();
        println!("👤 {}: {} allowed, {} denied", user, allowed, denied);
    }
    println!();
}

async fn example_batching() {
    println!("=== Example 3: Batching for Performance ===\n");

    let config = TokenBucketConfig {
        capacity: 1000,
        refill_rate: 100,
        refill_interval: Duration::from_secs(1),
    };

    let memory_backend = MemoryBackend::new(config.clone());
    let batching_backend = BatchingBackend::new(memory_backend, 100);
    let limiter = Arc::new(RateLimiter::new(batching_backend, false));

    println!("Making 500 requests with batching...");
    let start = Instant::now();

    for i in 0..500 {
        limiter.check_limit("batch_user", 1).await.unwrap();
        if i % 100 == 0 {
            println!("  Completed {} requests", i);
        }
    }

    let elapsed = start.elapsed();
    println!("⏱️  Time: {:?}", elapsed);
    println!("📈 Throughput: {:.0} req/sec\n", 500.0 / elapsed.as_secs_f64());
}


async fn example_fail_open() {
    println!("=== Example 4: Fail-Open Strategy ===\n");

    // Simulate a failing backend
    struct FailingBackend;

    #[async_trait::async_trait]
    impl guardian_core::StorageBackend for FailingBackend {
        async fn take_token(&self, _key: &str, _cost: u64) -> Result<bool, guardian_core::RateLimitError> {
            Err(guardian_core::RateLimitError::StorageError("Simulated failure".to_string()))
        }

        async fn get_usage(&self, _key: &str) -> Result<u64, guardian_core::RateLimitError> {
            Err(guardian_core::RateLimitError::StorageError("Simulated failure".to_string()))
        }

        async fn reset(&self, _key: &str) -> Result<(), guardian_core::RateLimitError> {
            Err(guardian_core::RateLimitError::StorageError("Simulated failure".to_string()))
        }
    }

    
    let limiter_open = RateLimiter::new(FailingBackend, true);
    match limiter_open.check_limit("user", 1).await {
        Ok(LimitResult::Allowed) => println!("✅ Fail-open mode: Request allowed despite backend error"),
        _ => println!("❌ Unexpected result"),
    }

    
    let limiter_closed = RateLimiter::new(FailingBackend, false);
    match limiter_closed.check_limit("user", 1).await {
        Err(e) => println!("❌ Fail-closed mode: Request denied - {}", e),
        _ => println!("✅ Unexpected result"),
    }
    println!();
}

async fn example_cost_based() {
    println!("=== Example 5: Cost-Based Limiting ===\n");

    let config = TokenBucketConfig {
        capacity: 100,
        refill_rate: 10,
        refill_interval: Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config);
    let limiter = RateLimiter::new(backend, false);

    // Different operations have different costs
    struct Operation {
        name: &'static str,
        cost: u64,
    }

    let operations = vec![
        Operation { name: "read", cost: 1 },
        Operation { name: "write", cost: 5 },
        Operation { name: "search", cost: 10 },
        Operation { name: "bulk_export", cost: 50 },
    ];

    for op in operations {
        match limiter.check_limit("api_client", op.cost).await {
            Ok(LimitResult::Allowed) => {
                println!("✅ {} (cost: {}) - allowed", op.name, op.cost);
            }
            Ok(LimitResult::Denied { retry_after }) => {
                println!(
                    "❌ {} (cost: {}) - denied, retry after {:?}",
                    op.name, op.cost, retry_after
                );
            }
            Err(e) => println!("⚠️  Error: {}", e),
        }
    }
    println!();
}

async fn benchmark_throughput() {
    println!("=== Benchmark: Maximum Throughput ===\n");

    let config = TokenBucketConfig {
        capacity: 1_000_000,
        refill_rate: 100_000,
        refill_interval: Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config);
    let limiter = Arc::new(RateLimiter::new(backend, false));

    let num_requests = 100_000;
    let concurrency = 100;
    let semaphore = Arc::new(Semaphore::new(concurrency));

    println!("Running {} requests with {} concurrent workers...", num_requests, concurrency);
    let start = Instant::now();

    let mut handles = vec![];
    for i in 0..num_requests {
        let limiter = Arc::clone(&limiter);
        let semaphore = Arc::clone(&semaphore);

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            limiter.check_limit(&format!("user_{}", i % 1000), 1).await
        });
        handles.push(handle);
    }

    let mut allowed = 0;
    let mut denied = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(LimitResult::Allowed) => allowed += 1,
            Ok(LimitResult::Denied { .. }) => denied += 1,
            Err(_) => {}
        }
    }

    let elapsed = start.elapsed();
    let throughput = num_requests as f64 / elapsed.as_secs_f64();

    println!("\n📊 Results:");
    println!("  Total requests: {}", num_requests);
    println!("  Allowed: {}", allowed);
    println!("  Denied: {}", denied);
    println!("  Time: {:?}", elapsed);
    println!("  Throughput: {:.0} req/sec", throughput);
    println!("  Latency p50: ~{:.2}µs", elapsed.as_micros() as f64 / num_requests as f64);
    println!();
}


async fn benchmark_latency() {
    println!("=== Benchmark: Latency Distribution ===\n");

    let config = TokenBucketConfig {
        capacity: 10_000,
        refill_rate: 1_000,
        refill_interval: Duration::from_secs(1),
    };

    let backend = MemoryBackend::new(config);
    let limiter = RateLimiter::new(backend, false);

    let num_samples = 10_000;
    let mut latencies = Vec::with_capacity(num_samples);

    println!("Measuring {} samples...", num_samples);

    for _ in 0..num_samples {
        let start = Instant::now();
        limiter.check_limit("latency_test", 1).await.unwrap();
        let elapsed = start.elapsed();
        latencies.push(elapsed.as_nanos() as u64);
    }

    latencies.sort_unstable();

    let percentile = |p: f64| -> u64 {
        let idx = ((num_samples as f64 * p) as usize).min(num_samples - 1);
        latencies[idx]
    };

    println!("\n📊 Latency Distribution:");
    println!("  p50:  {} ns", percentile(0.50));
    println!("  p90:  {} ns", percentile(0.90));
    println!("  p95:  {} ns", percentile(0.95));
    println!("  p99:  {} ns", percentile(0.99));
    println!("  p999: {} ns", percentile(0.999));
    println!("  min:  {} ns", latencies[0]);
    println!("  max:  {} ns", latencies[num_samples - 1]);
    println!();
}


#[tokio::main]
async fn main() {
    println!("\n🛡️  Guardian Rate Limiter - Complete Demo\n");
    println!("═══════════════════════════════════════════════════════════\n");

    example_simple_usage().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    example_concurrent_users().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    example_batching().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    example_fail_open().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    example_cost_based().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    benchmark_throughput().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    benchmark_latency().await;

    println!("═══════════════════════════════════════════════════════════");
    println!("\n✅ All examples and benchmarks completed!\n");
}


#[allow(dead_code)]
mod config_example {
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct GuardianConfig {
        pub global: GlobalConfig,
        pub backends: BackendsConfig,
        pub limits: HashMap<String, LimitConfig>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct GlobalConfig {
        pub fail_open: bool,
        pub log_level: String,
        pub metrics_port: u16,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct BackendsConfig {
        pub primary: BackendType,
        pub fallback: Option<BackendType>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    #[serde(tag = "type")]
    pub enum BackendType {
        Memory { cache_size: usize },
        Redis { url: String, pool_size: usize },
        RedisCluster { nodes: Vec<String> },
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct LimitConfig {
        pub capacity: u64,
        pub refill_rate: u64,
        pub refill_interval_secs: u64,
        pub algorithm: Algorithm,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    #[serde(rename_all = "snake_case")]
    pub enum Algorithm {
        TokenBucket,
        SlidingWindow,
        FixedWindow,
    }

    pub fn example_yaml() -> &'static str {
        r#"
global:
  fail_open: true
  log_level: "info"
  metrics_port: 9090

backends:
  primary:
    type: "Redis"
    url: "redis://localhost:6379"
    pool_size: 10
  fallback:
    type: "Memory"
    cache_size: 10000

limits:
  default:
    capacity: 100
    refill_rate: 10
    refill_interval_secs: 1
    algorithm: "token_bucket"
  
  premium_user:
    capacity: 1000
    refill_rate: 100
    refill_interval_secs: 1
    algorithm: "token_bucket"
  
  api_endpoint:
    capacity: 10000
    refill_rate: 1000
    refill_interval_secs: 1
    algorithm: "sliding_window"
"#
    }
}
use guardian_client::GuardianClient;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🛡️  Guardian Retry Pattern Example\n");

    let mut client = GuardianClient::connect("http://localhost:50051").await?;
    let client_id = "retry_user";

    let result = make_request_with_retry(&mut client, client_id, 1, 3).await;

    match result {
        Ok(_) => println!("✅ Request succeeded"),
        Err(e) => println!("❌ Request failed after retries: {}", e),
    }

    Ok(())
}

async fn make_request_with_retry(
    client: &mut GuardianClient,
    client_id: &str,
    cost: u32,
    max_retries: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    for attempt in 0..=max_retries {
        println!("🔄 Attempt {}/{}", attempt + 1, max_retries + 1);
        
        let result = client.check_limit_detailed(client_id, cost).await?;
        
        if result.allowed {
            println!("✅ Request allowed on attempt {}", attempt + 1);
            return Ok(());
        } else {
            if attempt < max_retries {
                let wait_time = result.retry_after_seconds.max(1);
                println!(
                    "⏳ Rate limited. Waiting {} seconds before retry...",
                    wait_time
                );
                sleep(Duration::from_secs(wait_time as u64)).await;
            } else {
                println!("❌ Rate limited. Max retries exceeded.");
                return Err("Rate limited after max retries".into());
            }
        }
    }
    
    Err("Failed after retries".into())
}
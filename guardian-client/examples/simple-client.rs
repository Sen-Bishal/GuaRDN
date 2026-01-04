use guardian_client::GuardianClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🛡️  Guardian Client Example\n");

    println!("Connecting to Guardian service at localhost:50051...");
    let mut client = GuardianClient::connect("http://localhost:50051").await?;
    println!("✅ Connected!\n");


    println!("=== Example 1: Simple Check ===");
    let client_id = "user123";
    
    for i in 1..=5 {
        match client.check_limit(client_id, 1).await {
            Ok(true) => println!("✅ Request {} allowed", i),
            Ok(false) => println!("❌ Request {} denied - rate limited", i),
            Err(e) => println!("⚠️  Error: {}", e),
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    println!();


    println!("=== Example 2: Get Usage ===");
    match client.get_usage(client_id).await {
        Ok(usage) => println!("📊 Current usage for {}: {} tokens", client_id, usage),
        Err(e) => println!("⚠️  Error: {}", e),
    }

    println!();


    println!("=== Example 3: Detailed Check ===");
    match client.check_limit_detailed(client_id, 10).await {
        Ok(result) => {
            if result.allowed {
                println!("✅ Request allowed");
                println!("   Remaining tokens: {}", result.remaining_tokens);
            } else {
                println!("❌ Request denied");
                println!("   Retry after: {} seconds", result.retry_after_seconds);
            }
        }
        Err(e) => println!("⚠️  Error: {}", e),
    }

    println!();

    println!("=== Example 4: With Rate Limit ===");
    let result = client
        .with_rate_limit("user456", 1, async {
            println!("✅ Executing protected operation...");
            Ok::<_, guardian_client::ClientError>("Operation successful!")
        })
        .await;

    match result {
        Ok(msg) => println!("{}", msg),
        Err(e) => println!("⚠️  {}", e),
    }

    println!();

    println!("=== Example 5: Reset Limit ===");
    match client.reset_limit(client_id).await {
        Ok(_) => println!("✅ Limit reset successfully for {}", client_id),
        Err(e) => println!("⚠️  Error: {}", e),
    }

    println!("\n🎉 Examples complete!");
    Ok(())
}

use guardian_client::GuardianClient;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🛡️  Guardian Concurrent Clients Example\n");

    
    let client = GuardianClient::connect("http://localhost:50051").await?;
    let client = Arc::new(Mutex::new(client));

    let mut handles = vec![];

    
    for user_id in 0..10 {
        let client = Arc::clone(&client);
        
        let handle = tokio::spawn(async move {
            let client_id = format!("user_{}", user_id);
            let mut allowed = 0;
            let mut denied = 0;

            
            for _ in 0..20 {
                let mut client = client.lock().await;
                match client.check_limit(&client_id, 1).await {
                    Ok(true) => allowed += 1,
                    Ok(false) => denied += 1,
                    Err(e) => eprintln!("Error for {}: {}", client_id, e),
                }
                drop(client); // Release lock
                
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }

            (client_id, allowed, denied)
        });
        
        handles.push(handle);
    }

    
    println!("Running concurrent requests...\n");
    for handle in handles {
        let (client_id, allowed, denied) = handle.await?;
        println!(
            "👤 {}: {} allowed, {} denied",
            client_id, allowed, denied
        );
    }

    println!("\n✅ All concurrent requests completed!");
    Ok(())
}

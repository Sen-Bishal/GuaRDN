#[cfg(test)]
mod tests {
    use guardian_client::GuardianClient;

    #[tokio::test]
    #[ignore] 
    async fn test_basic_connection() {
        let client = GuardianClient::connect("http://localhost:50051").await;
        assert!(client.is_ok());
    }

    #[tokio::test]
    #[ignore] 
    async fn test_check_limit() {
        let mut client = GuardianClient::connect("http://localhost:50051")
            .await
            .unwrap();
        
        let result = client.check_limit("test_user", 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore] 
    async fn test_get_usage() {
        let mut client = GuardianClient::connect("http://localhost:50051")
            .await
            .unwrap();
        
        client.check_limit("usage_test", 10).await.unwrap();
        let usage = client.get_usage("usage_test").await.unwrap();
        assert!(usage > 0);
    }
}
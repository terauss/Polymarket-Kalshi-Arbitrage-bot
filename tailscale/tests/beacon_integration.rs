//! Integration test for beacon sender and listener.

use std::net::Ipv4Addr;
use std::time::Duration;
use tailscale::beacon::{BeaconListener, BeaconSender};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn test_beacon_discovery_flow() {
    // Simulate controller and trader on localhost

    // 1. Create listener (trader side) on ephemeral port
    let listener = BeaconListener::new(0).await.expect("Failed to create listener");
    let beacon_port = listener.local_addr().unwrap().port();

    // 2. Create sender (controller side) targeting localhost
    let sender = BeaconSender::new(vec![Ipv4Addr::LOCALHOST], beacon_port, 9001)
        .await
        .expect("Failed to create sender");

    // 3. Run sender in background
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let sender_task = tokio::spawn(async move {
        sender.run(cancel_clone).await;
    });

    // 4. Trader discovers controller
    let info = tokio::time::timeout(Duration::from_secs(10), listener.wait_for_controller())
        .await
        .expect("Timeout waiting for controller")
        .expect("Failed to receive beacon");

    // 5. Verify discovered info
    assert_eq!(info.ip, Ipv4Addr::LOCALHOST);
    assert_eq!(info.ws_port, 9001);
    assert!(!info.version.is_empty());

    // 6. Cleanup
    cancel.cancel();
    let _ = sender_task.await;
}

#[tokio::test]
async fn test_beacon_listener_timeout() {
    // Test that listener properly times out when no beacon is sent
    let listener = BeaconListener::new(0).await.expect("Failed to create listener");

    let result = listener
        .wait_for_controller_timeout(Duration::from_millis(100))
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Timeout"));
}

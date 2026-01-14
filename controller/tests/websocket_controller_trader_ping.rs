use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use remote_trader::heartbeat::HeartbeatManager;
use remote_trader::protocol::OutgoingMessage;
use remote_trader::websocket::WebSocketClient;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[tokio::test]
async fn controller_accepts_trader_ws_and_receives_ping() -> Result<()> {
    // Bind an ephemeral port for the controller-hosted WS server.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind ws listener")?;
    let addr = listener.local_addr().context("get listener addr")?;

    // Server: accept one WS connection and keep reading until we receive a ping message.
    let server_task: tokio::task::JoinHandle<Result<()>> = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.context("accept tcp")?;
        let mut ws = accept_async(stream).await.context("accept websocket")?;

        while let Some(msg) = ws.next().await {
            match msg.context("read ws frame")? {
                Message::Text(text) => {
                    if let Ok(OutgoingMessage::Ping { .. }) =
                        serde_json::from_str::<OutgoingMessage>(&text)
                    {
                        return Ok(());
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        bail!("websocket closed before receiving ping");
    });

    // Trader: connect to the controller WS server and start its heartbeat.
    let url = format!("ws://{}", addr);
    let mut ws_client = WebSocketClient::new(url);
    let (outgoing_tx, _incoming_rx) = ws_client.connect().await.context("trader connect")?;

    // Use a short interval so the test completes quickly.
    let heartbeat = HeartbeatManager::new(outgoing_tx, 1, 5);
    let heartbeat_handle = heartbeat.start();

    // Ensure we don't hang forever if something is broken.
    let ping_timeout = Duration::from_secs(10);
    let server_result = tokio::time::timeout(ping_timeout, server_task)
        .await
        .context("timed out waiting for ping")?;

    // Clean up background task(s) regardless of outcome.
    heartbeat_handle.abort();

    // Unwrap join + inner Result
    server_result.context("server task join")??;
    Ok(())
}


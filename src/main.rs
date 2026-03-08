use std::{sync::Arc, time::Duration};
use dotenvy::dotenv;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use tokio::{sync::{Semaphore, mpsc}, task::JoinSet, time::{Instant, sleep}};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct SubscriptionMessage {
    id: Option<u64>,
    result: Option<String>,
    method: Option<String>,
    params: Option<Params>
}

#[derive(Deserialize, Debug)]
struct Params {
    result: String
}

#[derive(Deserialize, Debug)]
struct TransactionResponse {
    result: Option<Transaction>
}

#[derive(Deserialize, Debug)]
struct Transaction {
    hash: String,
    to: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let wss_url = std::env::var("WSS_RPC_URL")
        .expect("WSS_RPC_URL secret does not exist");
    let http_url = Arc::new(std::env::var("HTTP_RPC_URL").expect("HTTP_RPC_URL secret does not exist"));
    let target_contract = Arc::new(
        std::env::var("TARGET_CONTRACT")
        .expect("TARGET_CONTRACT secret does not exist")
        .to_lowercase()
    );

    let (tx, mut rx) = mpsc::channel::<String>(1000);

    let http_client = Client::builder()
        .timeout(Duration::from_secs(5))
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;

    let semaphore = Arc::new(Semaphore::new(20));

    let consumer_handle = tokio::spawn(async move {
        let mut worker_pool = JoinSet::new();

        while let Some(hash) = rx.recv().await {
           while let Some(_) = worker_pool.try_join_next() {}

           let permit = semaphore.clone().acquire_owned().await.expect("semaphore closed");
           
           let client = http_client.clone();
           let url = http_url.clone();
           let target = target_contract.clone();

           worker_pool.spawn(async move {
                let _permit = permit;

                let request_body = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "eth_getTransactionByHash",
                    "params": [hash]
                });

                let response = match client.post(url.as_ref()).json(&request_body).send().await {
                    Ok(res) => res,
                    Err(e) => {
                        eprintln!("HTTP request failed for {}: {}", hash, e);
                        return;
                    }
                };

                let rpc_data = match response.json::<TransactionResponse>().await {
                    Ok(data) => data,
                    Err(e) => {
                        eprintln!("Failed to parse JSON for {}: {}", hash, e);
                        return;
                    }
                };

                let Some(transaction) = rpc_data.result else {
                    return;
                };

                if let Some(to_address) = transaction.to {
                    if to_address.to_lowercase() == *target {
                        println!("tx hash: {} | to: {}", transaction.hash, to_address);
                    }
                }
           });
        }
        println!("producer dropped, processing remaining queue");

        while let Some(res) = worker_pool.join_next().await {
            if let Err(e) = res {
                eprintln!("worker panicked during shutdown: {}", e);
            }
        }

        println!("all workers finished shutting down gracefully");
    });

    let supervisor = async move {
        let mut backoff_secs = 1;

        loop {
            let ws_stream = match connect_async(&wss_url).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    eprintln!("WebSocket connection failed: {} retrying in {}s", e, backoff_secs);
                    sleep(Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            println!("WebSocket connected successfully.");

            let connection_start = Instant::now();

            let (mut write, mut read) = ws_stream.split();

            let subscribe_request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_subscribe",
                "params": ["newPendingTransactions"]
            });

            if let Err(e) = write.send(Message::Text(subscribe_request.to_string().into())).await {
                eprintln!("Failed to send subscription request: {}", e);
                continue;
            }

            while let Some(message) = read.next().await {
                match message {
                    Ok(msg) if msg.is_text() => {
                        let text = match msg.to_text() {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("Failed to parse message text: {}", e);
                                continue;
                            }
                        };

                        match serde_json::from_str::<SubscriptionMessage>(text) {
                            Ok(SubscriptionMessage {method: Some(m), params: Some(p), ..}) if m == "eth_subscription" => {
                                if tx.send(p.result).await.is_err() {
                                    eprintln!("consumer dropped, exiting program");
                                    return;
                                }
                            }

                            Ok(SubscriptionMessage { id: Some(id), result: Some(res), .. }) => {
                                println!("Subscription confirmed. Request ID: {}, Sub ID: {}", id, res);
                            }

                            Err(e) => {
                                eprintln!("Failed to deserialize JSON: {} | Payload: {}", e, text);
                            }

                            _ => {}
                        }
                    }

                    Ok(_) => {}

                    Err(e) => {
                        eprintln!("WebSocket error: {}", e);
                        break;
                    }
                }
            }

            eprintln!("websocket stream ended unexpectedly");
            if connection_start.elapsed().as_secs() > 10 {
                println!("connection was stable resetting backoff");
                backoff_secs = 1;
            } else {
                println!("connection dropped instantly, applying backoff of {}s", backoff_secs);
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        }
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nCTRL C recieved initiating graceful shutdown");
        }
        _ = supervisor => {
            println!("\nsupervisor exited unexpectedly");
        }
    }

    let _ = consumer_handle.await;
    println!("shutdown complete");

    Ok(())
}

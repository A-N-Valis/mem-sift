# mem-sift

`mem-sift` is a lightweight Rust tool for monitoring the Ethereum mempool in real time. It subscribes to pending transactions over WebSocket, fetches transaction data through an HTTP RPC endpoint, and filters transactions targeting a specific smart contract.

## Features

* Real-time Ethereum mempool monitoring
* WebSocket subscription to `newPendingTransactions`
* Concurrent transaction lookups
* Worker pool with semaphore-based concurrency control
* Automatic WebSocket reconnection with exponential backoff
* Graceful shutdown via `CTRL+C`
* Environment-based configuration with `.env`

## Setup

Create a `.env` file in the project root:

```
WSS_RPC_URL=your_websocket_rpc_url
HTTP_RPC_URL=your_http_rpc_url
TARGET_CONTRACT=0xYourContractAddress
```

## Run

```
cargo run --release
```
#![allow(
    clippy::new_without_default,
    clippy::needless_lifetimes,
    clippy::vec_init_then_push
)]

use alloy::rpc::types::Transaction as AlloyTransaction;
use pin_project::pin_project;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};
use tonic::{transport::Channel, Request, Streaming};

pub mod api;
pub mod eth;
pub mod filter;
pub mod types;

use api::{
    api_client::ApiClient, RawTxMsg, RawTxSequenceMsg, TransactionResponse, TxFilter,
    TxSequenceMsg, TxSequenceResponse,
};
use eth::{CompactBeaconBlock, ExecutionPayload, ExecutionPayloadHeader, Transaction};

#[pin_project]
pub struct TxStream {
    #[pin]
    stream: Streaming<Transaction>,
}

impl TxStream {
    pub async fn next(&mut self) -> Option<AlloyTransaction> {
        let proto = self.stream.message().await.unwrap_or(None)?;
        Some(proto_to_tx(proto))
    }
}

#[allow(clippy::large_enum_variant)]
pub enum SendType {
    Transaction {
        tx: Transaction,
        response: oneshot::Sender<TransactionResponse>,
    },
    RawTransaction {
        msg: RawTxMsg,
        response: oneshot::Sender<TransactionResponse>,
    },
    TransactionSequence {
        msg: TxSequenceMsg,
        response: oneshot::Sender<TxSequenceResponse>,
    },
    RawTransactionSequence {
        msg: RawTxSequenceMsg,
        response: oneshot::Sender<TxSequenceResponse>,
    },
}

struct ClientInner {
    cmd_rx: mpsc::UnboundedReceiver<SendType>,
    new_tx_sender: mpsc::UnboundedSender<Transaction>,
    new_raw_tx_sender: mpsc::UnboundedSender<RawTxMsg>,
    new_tx_seq_sender: mpsc::UnboundedSender<TxSequenceMsg>,
    new_raw_tx_seq_sender: mpsc::UnboundedSender<RawTxSequenceMsg>,
    new_tx_responses: Streaming<TransactionResponse>,
    new_raw_tx_responses: Streaming<TransactionResponse>,
    new_tx_seq_responses: Streaming<TxSequenceResponse>,
    new_raw_tx_seq_responses: Streaming<TxSequenceResponse>,
}

impl ClientInner {
    async fn run_loop(mut self) {
        while let Some(cmd) = self.cmd_rx.recv().await {
            match cmd {
                SendType::Transaction { tx, response } => {
                    self.new_tx_sender.send(tx).unwrap();

                    if let Some(res) = self.new_tx_responses.next().await {
                        let _ = response.send(res.unwrap());
                    }
                }
                SendType::RawTransaction { msg, response } => {
                    self.new_raw_tx_sender.send(msg).unwrap();

                    if let Some(res) = self.new_raw_tx_responses.next().await {
                        let _ = response.send(res.unwrap());
                    }
                }
                SendType::TransactionSequence { msg, response } => {
                    self.new_tx_seq_sender.send(msg).unwrap();

                    if let Some(res) = self.new_tx_seq_responses.next().await {
                        let _ = response.send(res.unwrap());
                    }
                }
                SendType::RawTransactionSequence { msg, response } => {
                    self.new_raw_tx_seq_sender.send(msg).unwrap();

                    if let Some(res) = self.new_raw_tx_seq_responses.next().await {
                        let _ = response.send(res.unwrap());
                    }
                }
            }
        }
    }
}

pub struct Client {
    key: String,
    client: ApiClient<Channel>,
    cmd_tx: mpsc::UnboundedSender<SendType>,
}

impl Client {
    pub async fn connect(
        target: String,
        api_key: String,
    ) -> Result<Client, Box<dyn std::error::Error>> {
        let targetstr = if !target.starts_with("http://") {
            "http://".to_owned() + &target
        } else {
            target
        };

        let mut client = ApiClient::connect(targetstr.to_owned()).await?;

        // Set up the different streams
        let (new_tx_sender, rx) = mpsc::unbounded_channel();
        let rx_stream = UnboundedReceiverStream::new(rx);

        let mut req = Request::new(rx_stream);
        // Append the api key metadata
        req.metadata_mut()
            .append("x-api-key", api_key.parse().unwrap());

        let new_tx_responses = client.send_transaction(req).await?.into_inner();

        let (new_raw_tx_sender, rx) = mpsc::unbounded_channel();
        let rx_stream = UnboundedReceiverStream::new(rx);

        let mut req = Request::new(rx_stream);
        // Append the api key metadata
        req.metadata_mut()
            .append("x-api-key", api_key.parse().unwrap());

        let new_raw_tx_responses = client.send_raw_transaction(req).await?.into_inner();

        let (new_tx_seq_sender, rx) = mpsc::unbounded_channel();
        let rx_stream = UnboundedReceiverStream::new(rx);

        let mut req = Request::new(rx_stream);
        // Append the api key metadata
        req.metadata_mut()
            .append("x-api-key", api_key.parse().unwrap());

        let new_tx_seq_responses = client.send_transaction_sequence(req).await?.into_inner();

        let (new_raw_tx_seq_sender, rx) = mpsc::unbounded_channel();
        let rx_stream = UnboundedReceiverStream::new(rx);

        let mut req = Request::new(rx_stream);
        // Append the api key metadata
        req.metadata_mut()
            .append("x-api-key", api_key.parse().unwrap());

        let new_raw_tx_seq_responses = client
            .send_raw_transaction_sequence(req)
            .await?
            .into_inner();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let client = Client {
            client,
            key: api_key,
            cmd_tx,
        };

        // Create the inner client which has access to all the gRPC channels
        let inner = ClientInner {
            cmd_rx,
            new_tx_sender,
            new_tx_responses,
            new_raw_tx_sender,
            new_raw_tx_responses,
            new_tx_seq_sender,
            new_tx_seq_responses,
            new_raw_tx_seq_sender,
            new_raw_tx_seq_responses,
        };

        // Spawn the loop
        tokio::task::spawn(inner.run_loop());

        Ok(client)
    }

    /// Broadcasts a signed transaction to the Fiber Network. Returns hash and the timestamp
    /// of when the first node received the transaction.
    pub async fn send_transaction(
        &self,
        tx: Transaction,
    ) -> Result<(String, i64), Box<dyn std::error::Error>> {
        let (res, rx) = oneshot::channel();

        let _ = self
            .cmd_tx
            .send(SendType::Transaction { tx, response: res });

        let res = rx.await?;

        Ok((res.hash.to_owned(), res.timestamp))
    }

    /// Broadcasts a signed, RLP encoded transaction to the Fiber Network. Returns hash and the timestamp
    /// of when the first node received the transaction.
    pub async fn send_raw_transaction(
        &self,
        raw_tx: Vec<u8>,
    ) -> Result<(String, i64), Box<dyn std::error::Error>> {
        let (res, rx) = oneshot::channel();

        let _ = self.cmd_tx.send(SendType::RawTransaction {
            msg: RawTxMsg { raw_tx },
            response: res,
        });

        let res = rx.await?;

        Ok((res.hash.to_owned(), res.timestamp))
    }

    /// Broadcasts a signed transaction sequence to the Fiber Network. Returns the array of hashes and
    /// the timestamp of when the first node received the sequence.
    pub async fn send_transaction_sequence(
        &self,
        tx_sequence: Vec<Transaction>,
    ) -> Result<(Vec<String>, i64), Box<dyn std::error::Error>> {
        let (res, rx) = oneshot::channel();

        let _ = self.cmd_tx.send(SendType::TransactionSequence {
            msg: TxSequenceMsg {
                sequence: tx_sequence,
            },
            response: res,
        });

        let res = rx.await?;

        let timestamp = res.sequence_response[0].timestamp;
        let hashes = res
            .sequence_response
            .into_iter()
            .map(|resp| resp.hash)
            .collect();

        Ok((hashes, timestamp))
    }

    /// Broadcasts a signed, RLP encoded transaction sequence to the Fiber Network. Returns the array of hashes and
    /// the timestamp of when the first node received the sequence.
    pub async fn send_raw_transaction_sequence(
        &self,
        raw_tx_sequence: Vec<Vec<u8>>,
    ) -> Result<(Vec<String>, i64), Box<dyn std::error::Error>> {
        let (res, rx) = oneshot::channel();

        let _ = self.cmd_tx.send(SendType::RawTransactionSequence {
            msg: RawTxSequenceMsg {
                raw_txs: raw_tx_sequence,
            },
            response: res,
        });

        let res = rx.await?;

        let timestamp = res.sequence_response[0].timestamp;
        let hashes = res
            .sequence_response
            .into_iter()
            .map(|resp| resp.hash)
            .collect();

        Ok((hashes, timestamp))
    }

    /// Subscribes to new transactions, returning a stream of Alloy RPC transactions.
    pub async fn subscribe_new_txs(
        &self,
        filter: Option<Vec<u8>>,
    ) -> UnboundedReceiverStream<AlloyTransaction> {
        let f = match filter {
            Some(encoded_filter) => TxFilter {
                encoded: encoded_filter,
            },
            None => TxFilter { encoded: vec![] },
        };

        let mut req = Request::new(f);

        req.metadata_mut()
            .append("x-api-key", self.key.parse().unwrap());

        let mut inner = self
            .client
            .clone()
            .subscribe_new_txs(req)
            .await
            .unwrap()
            .into_inner();

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(Ok(transaction)) = inner.next().await {
                let _ = tx.send(proto_to_tx(transaction));
            }
        });

        UnboundedReceiverStream::new(rx)
    }

    pub async fn subscribe_new_execution_headers(
        &self,
    ) -> UnboundedReceiverStream<ExecutionPayloadHeader> {
        let mut req = Request::new(());

        req.metadata_mut()
            .append("x-api-key", self.key.parse().unwrap());

        let mut inner = self
            .client
            .clone()
            .subscribe_execution_headers(req)
            .await
            .unwrap()
            .into_inner();

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(Ok(header)) = inner.next().await {
                let _ = tx.send(header);
            }
        });

        UnboundedReceiverStream::new(rx)
    }

    /// Subscribes to new execution payloads, returns a stream of [`ExecutionPayload`]s.
    pub async fn subscribe_new_execution_payloads(
        &self,
    ) -> UnboundedReceiverStream<ExecutionPayload> {
        let mut req = Request::new(());

        req.metadata_mut()
            .append("x-api-key", self.key.parse().unwrap());

        let mut inner = self
            .client
            .clone()
            .subscribe_execution_payloads(req)
            .await
            .unwrap()
            .into_inner();

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(Ok(block)) = inner.next().await {
                let _ = tx.send(block);
            }
        });

        UnboundedReceiverStream::new(rx)
    }

    pub async fn subscribe_new_beacon_blocks(&self) -> UnboundedReceiverStream<CompactBeaconBlock> {
        let mut req = Request::new(());

        req.metadata_mut()
            .append("x-api-key", self.key.parse().unwrap());

        let mut inner = self
            .client
            .clone()
            .subscribe_beacon_blocks(req)
            .await
            .unwrap()
            .into_inner();

        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            while let Some(Ok(block)) = inner.next().await {
                let _ = tx.send(block);
            }
        });

        UnboundedReceiverStream::new(rx)
    }
}

fn proto_to_tx(proto: Transaction) -> AlloyTransaction {
    let tx_type = match proto.r#type {
        1 => Some("0x1"),
        2 => Some("0x2"),
        _ => None,
    };

    let v = if tx_type.is_some() {
        if proto.v > 1 {
            proto.v - 37
        } else {
            proto.v
        }
    } else {
        proto.v
    };

    // If transaction is legacy (no type) and its v value is 27 or 28, we set chain ID to None.
    // This signifies a pre EIP-155 transaction.
    let chain_id = if tx_type.is_none() && proto.v < 37 {
        Value::Null
    } else {
        quantity(proto.chain_id)
    };

    let access_list = proto
        .access_list
        .into_iter()
        .map(|item| {
            json!({
                "address": fixed_hex(item.address, 20),
                "storageKeys": item.storage_keys.into_iter().map(|key| fixed_hex(key, 32)).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let mut value = json!({
        "hash": fixed_hex(proto.hash, 32),
        "nonce": quantity(proto.nonce),
        "blockHash": null,
        "blockNumber": null,
        "transactionIndex": null,
        "from": fixed_hex(proto.from.unwrap_or_default(), 20),
        "to": proto.to.map(|to| fixed_hex(to, 20)),
        "value": bytes_quantity(proto.value),
        "gas": quantity(proto.gas),
        "input": hex_data(proto.input),
        "v": quantity(v),
        "r": bytes_quantity(proto.r),
        "s": bytes_quantity(proto.s),
        "chainId": chain_id,
    });

    if let Some(tx_type) = tx_type {
        value["type"] = json!(tx_type);
        value["accessList"] = json!(access_list);
    }
    if proto.gas_price != 0 {
        value["gasPrice"] = quantity(proto.gas_price);
    }
    if proto.max_fee != 0 {
        value["maxFeePerGas"] = quantity(proto.max_fee);
    }
    if proto.priority_fee != 0 {
        value["maxPriorityFeePerGas"] = quantity(proto.priority_fee);
    }

    serde_json::from_value(value)
        .expect("Fiber protobuf transaction should map to Alloy RPC transaction")
}

fn fixed_hex(bytes: Vec<u8>, expected_len: usize) -> String {
    let mut padded = vec![0; expected_len.saturating_sub(bytes.len())];
    padded.extend(bytes);
    format!("0x{}", hex::encode(padded))
}

fn hex_data(bytes: Vec<u8>) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn bytes_quantity(bytes: Vec<u8>) -> Value {
    let first_non_zero = bytes.iter().position(|byte| *byte != 0);
    match first_non_zero {
        Some(index) => json!(format!("0x{}", hex::encode(&bytes[index..]))),
        None => json!("0x0"),
    }
}

fn quantity(value: impl Into<u128>) -> Value {
    json!(format!("0x{:x}", value.into()))
}

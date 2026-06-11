use phyllo::{
    channel::{ChannelBuilder, ChannelHandler},
    error::RegisterChannelError,
    message::Message,
    socket::{SocketBuilder, SocketHandler},
};
use schema::StreamEvent;
use serde::{de::Error, Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use url::Url;

/// Creates a client for the OpenSea Stream API.
pub async fn client(network: Network, token: &str) -> SocketHandler<Collection> {
    let mut network: Url = Url::from(network);
    network.query_pairs_mut().append_pair("token", token);
    SocketBuilder::new(network).build().await
}

/// Subscribes to all events for a collection.
pub async fn subscribe_to(
    socket: &mut SocketHandler<Collection>,
    collection: Collection,
) -> Result<
    (
        ChannelHandler<Collection, Event, Value, StreamEvent>,
        broadcast::Receiver<Message<Collection, Event, Value, StreamEvent>>,
    ),
    RegisterChannelError,
> {
    socket.channel(ChannelBuilder::new(collection)).await
}

/// A collection whose OpenSea events can be subscribed to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Collection {
    /// Collection with slug.
    Collection(String),
    /// All possible collections.
    All,
}

impl std::fmt::Display for Collection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "collection:{}",
            match &self {
                Collection::Collection(c) => c,
                Collection::All => "*",
            }
        )
    }
}

impl Serialize for Collection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Collection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        let s = s
            .strip_prefix("collection:")
            .ok_or_else(|| D::Error::custom("expected collection:name"))?;

        Ok(match s {
            "*" => Collection::All,
            _ => Collection::Collection(s.to_owned()),
        })
    }
}

/// OpenSea Stream API network.
pub enum Network {
    /// Production networks.
    Mainnet,
    /// Test networks.
    Testnet,
}

impl From<Network> for Url {
    fn from(val: Network) -> Self {
        match val {
            Network::Mainnet => {
                Url::parse("wss://stream.openseabeta.com/socket/websocket").unwrap()
            }
            Network::Testnet => {
                Url::parse("wss://testnets-stream.openseabeta.com/socket/websocket").unwrap()
            }
        }
    }
}

/// Phoenix event names delivered by the OpenSea socket.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// An item has been listed.
    ItemListed,
    /// An item has been sold.
    ItemSold,
    /// An item has been transferred.
    ItemTransferred,
    /// An item has had metadata updated.
    ItemMetadataUpdated,
    /// An item listing was cancelled.
    ItemCancelled,
    /// An item received an offer.
    ItemReceivedOffer,
    /// An item received a bid.
    ItemReceivedBid,
}

/// OpenSea stream JSON schema.
pub mod schema {
    use alloy::primitives::{Address, B256, U256};
    use chrono::{DateTime, Utc};
    use serde::{de::Error, Deserialize, Serialize};
    use std::{fmt, str::FromStr};
    use url::Url;

    /// Payload of a message received from the websocket.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct StreamEvent {
        /// Timestamp of when this message was sent to the client.
        pub sent_at: DateTime<Utc>,
        /// Contents of the message.
        #[serde(flatten)]
        pub payload: Payload,
    }

    /// Content of an OpenSea stream event.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    #[serde(tag = "event_type", content = "payload")]
    #[serde(rename_all = "snake_case")]
    pub enum Payload {
        /// An item has been listed for sale.
        ItemListed(ItemListedData),
        /// An item has been sold.
        ItemSold(ItemSoldData),
        /// An item has been transferred.
        ItemTransferred(ItemTransferredData),
        /// An item has had metadata updated.
        ItemMetadataUpdated(ItemMetadataUpdatedData),
        /// An item listing was cancelled.
        ItemCancelled(ItemCancelledData),
        /// An item received an offer.
        ItemReceivedOffer(ItemReceivedOfferData),
        /// An item received a bid.
        ItemReceivedBid(ItemReceivedBidData),
    }

    /// Context for a message.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct Context {
        /// Collection that the token belongs to.
        pub collection: Collection,
        /// Information about the item itself.
        pub item: Item,
    }

    /// An OpenSea collection.
    #[derive(Debug, Clone)]
    pub struct Collection(String);

    impl Serialize for Collection {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            #[derive(Serialize)]
            struct Inner {
                slug: String,
            }

            Inner {
                slug: self.0.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Collection {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            #[derive(Deserialize)]
            struct Inner {
                slug: String,
            }

            Deserialize::deserialize(deserializer).map(|v: Inner| Collection(v.slug))
        }
    }

    /// Context about an item.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct Item {
        /// Identifier.
        pub nft_id: NftId,
        /// Link to OpenSea page.
        pub permalink: Url,
        /// Chain the item is on.
        pub chain: Chain,
        /// Basic metadata.
        pub metadata: Metadata,
    }

    /// Identifier of the NFT.
    #[derive(Debug, Clone)]
    pub struct NftId {
        /// Chain the item is on.
        pub network: Chain,
        /// Contract address.
        pub address: Address,
        /// Token ID.
        pub id: U256,
    }

    impl Serialize for NftId {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            format!("{}/{:?}/{}", self.network, self.address, self.id).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for NftId {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let s: String = Deserialize::deserialize(deserializer)?;
            let mut parts = s.splitn(3, '/').fuse();

            let network = parts
                .next()
                .map(Chain::from_str)
                .ok_or_else(|| D::Error::custom("expected network"))?
                .map_err(|_| D::Error::custom("invalid network"))?;

            let address = parts
                .next()
                .map(Address::from_str)
                .ok_or_else(|| D::Error::custom("expected address"))?
                .map_err(D::Error::custom)?;

            let id = parts
                .next()
                .map(|id| U256::from_str_radix(id, 10))
                .ok_or_else(|| D::Error::custom("expected id"))?
                .map_err(D::Error::custom)?;

            Ok(NftId {
                network,
                address,
                id,
            })
        }
    }

    /// Network an item is on.
    #[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    #[serde(tag = "name", rename_all = "lowercase")]
    #[non_exhaustive]
    pub enum Chain {
        /// Ethereum mainnet.
        Ethereum,
        /// Polygon mainnet.
        #[serde(rename = "matic")]
        Polygon,
        /// Klaytn mainnet.
        Klaytn,
        /// Solana mainnet.
        Solana,
        /// Goerli testnet.
        Goerli,
        /// Rinkeby testnet.
        Rinkeby,
        /// Mumbai testnet.
        Mumbai,
        /// Baobab testnet.
        Baobab,
    }

    impl FromStr for Chain {
        type Err = ();

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "ethereum" => Ok(Chain::Ethereum),
                "matic" => Ok(Chain::Polygon),
                "klaytn" => Ok(Chain::Klaytn),
                "solana" => Ok(Chain::Solana),
                "goerli" => Ok(Chain::Goerli),
                "rinkeby" => Ok(Chain::Rinkeby),
                "mumbai" => Ok(Chain::Mumbai),
                "baobab" => Ok(Chain::Baobab),
                _ => Err(()),
            }
        }
    }

    impl fmt::Display for Chain {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{}",
                match self {
                    Chain::Ethereum => "ethereum",
                    Chain::Polygon => "matic",
                    Chain::Klaytn => "klaytn",
                    Chain::Solana => "solana",
                    Chain::Goerli => "goerli",
                    Chain::Rinkeby => "rinkeby",
                    Chain::Mumbai => "mumbai",
                    Chain::Baobab => "baobab",
                }
            )
        }
    }

    /// Basic metadata of an item.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct Metadata {
        /// Name.
        pub name: Option<String>,
        /// Description.
        pub description: Option<String>,
        /// Image URL.
        pub image_url: Option<Url>,
        /// Animation URL.
        pub animation_url: Option<Url>,
        /// URL to metadata.
        pub metadata_url: Option<Url>,
    }

    /// Payload data for item listing events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemListedData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the listing was created.
        pub event_timestamp: DateTime<Utc>,
        /// Starting price of the listing.
        #[serde(with = "u256_fromstr_radix_10")]
        pub base_price: U256,
        /// Expiration date.
        pub expiration_date: DateTime<Utc>,
        /// Whether the listing is private.
        pub is_private: bool,
        /// Timestamp of when the listing was created.
        pub listing_date: DateTime<Utc>,
        /// Listing type.
        pub listing_type: Option<ListingType>,
        /// Creator of the listing.
        #[serde(with = "address_fromjson")]
        pub maker: Address,
        /// Hash id of the listing.
        pub order_hash: B256,
        /// Token accepted for payment.
        pub payment_token: PaymentToken,
        /// Number of items on sale.
        pub quantity: u64,
        /// Buyer of the listing.
        #[serde(with = "address_fromjson_opt", default)]
        pub taker: Option<Address>,
    }

    /// Payload data for item sold events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemSoldData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the item was sold.
        pub event_timestamp: DateTime<Utc>,
        /// Timestamp of when the listing was closed.
        pub closing_date: DateTime<Utc>,
        /// Whether the listing was private.
        pub is_private: bool,
        /// Listing type.
        pub listing_type: Option<ListingType>,
        /// Creator of the listing.
        #[serde(with = "address_fromjson")]
        pub maker: Address,
        /// Token used for payment.
        pub payment_token: PaymentToken,
        /// Number of items bought.
        pub quantity: u64,
        /// Purchase price.
        #[serde(with = "u256_fromstr_radix_10")]
        pub sale_price: U256,
        /// Buyer/winner of the listing.
        #[serde(with = "address_fromjson")]
        pub taker: Address,
        /// Transaction for the purchase.
        pub transaction: Transaction,
    }

    /// Payload data for item transferred events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemTransferredData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the item was transferred.
        pub event_timestamp: DateTime<Utc>,
        /// Transaction of the transfer.
        pub transaction: Transaction,
        /// Address the item was transferred from.
        #[serde(with = "address_fromjson")]
        pub from_account: Address,
        /// Address the item was transferred to.
        #[serde(with = "address_fromjson")]
        pub to_account: Address,
        /// Number of items transferred.
        pub quantity: u64,
    }

    /// Payload data for item metadata updates.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemMetadataUpdatedData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// New name.
        pub name: Option<String>,
        /// New description.
        pub description: Option<String>,
        /// New cached preview URL.
        pub image_preview_url: Option<Url>,
        /// New animation URL.
        pub animation_url: Option<Url>,
        /// New background color.
        pub background_color: Option<String>,
        /// New URL to metadata.
        pub metadata_url: Option<Url>,
        /// New traits.
        #[serde(default)]
        pub traits: Vec<serde_json::Value>,
    }

    /// Payload data for item cancelled events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemCancelledData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the listing was cancelled.
        pub event_timestamp: DateTime<Utc>,
        /// Listing type.
        pub listing_type: Option<ListingType>,
        /// Creator of the cancellation order.
        #[serde(with = "address_fromjson")]
        pub maker: Address,
        /// Hash id of the listing.
        pub order_hash: B256,
        /// Token accepted for payment.
        pub payment_token: PaymentToken,
        /// Number of items in listing.
        pub quantity: u64,
        /// Transaction for the cancellation.
        pub transaction: Transaction,
    }

    /// Payload data for item offer events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemReceivedOfferData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the offer was received.
        pub event_timestamp: DateTime<Utc>,
        /// Offer price.
        #[serde(with = "u256_fromstr_radix_10")]
        pub base_price: U256,
        /// Timestamp of when the offer was created.
        pub created_date: DateTime<Utc>,
        /// Timestamp of when the offer will expire.
        pub expiration_date: DateTime<Utc>,
        /// Creator of the offer.
        #[serde(with = "address_fromjson")]
        pub maker: Address,
        /// Hash id of the listing.
        pub order_hash: B256,
        /// Token offered for payment.
        pub payment_token: PaymentToken,
        /// Number of items on the offer.
        pub quantity: u64,
        /// Taker of the offer.
        #[serde(with = "address_fromjson_opt", default)]
        pub taker: Option<Address>,
    }

    /// Payload data for item bid events.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct ItemReceivedBidData {
        /// Context.
        #[serde(flatten)]
        pub context: Context,
        /// Timestamp of when the bid was received.
        pub event_timestamp: DateTime<Utc>,
        /// Bid price.
        #[serde(with = "u256_fromstr_radix_10")]
        pub base_price: U256,
        /// Timestamp of when the bid was created.
        pub created_date: DateTime<Utc>,
        /// Timestamp of when the bid will expire.
        pub expiration_date: DateTime<Utc>,
        /// Creator of the bid.
        #[serde(with = "address_fromjson")]
        pub maker: Address,
        /// Hash id of the listing.
        pub order_hash: B256,
        /// Token offered for payment.
        pub payment_token: PaymentToken,
        /// Number of items on the offer.
        pub quantity: u64,
        /// Taker of the bid.
        #[serde(with = "address_fromjson_opt", default)]
        pub taker: Option<Address>,
    }

    /// Auctioning system used by the listing.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    #[serde(rename_all = "lowercase")]
    pub enum ListingType {
        /// English auction.
        English,
        /// Dutch auction.
        Dutch,
    }

    /// Details of a transaction.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct Transaction {
        /// Transaction hash.
        pub hash: B256,
        /// Timestamp of transaction.
        pub timestamp: DateTime<Utc>,
    }

    /// Token used for payment.
    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct PaymentToken {
        /// Contract address.
        pub address: Address,
        /// Granularity of the token.
        pub decimals: u64,
        /// Price of token in ETH.
        #[serde(with = "f64_fromstring")]
        pub eth_price: f64,
        /// Name.
        pub name: String,
        /// Symbol.
        pub symbol: String,
        /// Price of token in USD.
        #[serde(with = "f64_fromstring")]
        pub usd_price: f64,
    }

    mod address_fromjson {
        use alloy::primitives::Address;
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        #[derive(Serialize, Deserialize)]
        struct Inner {
            address: Address,
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Address, D::Error>
        where
            D: Deserializer<'de>,
        {
            Deserialize::deserialize(deserializer).map(|v: Inner| v.address)
        }

        pub fn serialize<S>(value: &Address, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Inner { address: *value }.serialize(serializer)
        }
    }

    mod address_fromjson_opt {
        use alloy::primitives::Address;
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        #[derive(Serialize, Deserialize)]
        struct Inner {
            address: Address,
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Address>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let inner: Option<Inner> = Deserialize::deserialize(deserializer)?;
            Ok(inner.map(|i| i.address))
        }

        pub fn serialize<S>(value: &Option<Address>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            value.map(|v| Inner { address: v }).serialize(serializer)
        }
    }

    mod u256_fromstr_radix_10 {
        use alloy::primitives::U256;
        use serde::{de::Visitor, Deserializer, Serializer};
        use std::fmt;

        pub fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct Helper;

            impl<'de> Visitor<'de> for Helper {
                type Value = U256;

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a string")
                }

                fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    U256::from_str_radix(value, 10).map_err(serde::de::Error::custom)
                }
            }

            deserializer.deserialize_str(Helper)
        }

        pub fn serialize<S>(value: &U256, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.collect_str(&value)
        }
    }

    mod f64_fromstring {
        use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

        pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum StringFloat {
                Str(String),
                F64(f64),
            }

            match StringFloat::deserialize(deserializer)? {
                StringFloat::Str(s) => s.parse().map_err(D::Error::custom),
                StringFloat::F64(f) => Ok(f),
            }
        }

        pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            value.to_string().serialize(serializer)
        }
    }
}

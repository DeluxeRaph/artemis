//! Local MEV-Share JSON-RPC and SSE wire types.

pub mod rpc {
    use alloy::primitives::{Address, Bytes, B256};
    use serde::{
        de::{self, Deserializer},
        ser::{SerializeSeq, Serializer},
        Deserialize, Serialize,
    };

    /// A bundle of transactions to send to the matchmaker.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct SendBundleRequest {
        /// The version of the MEV-Share API to use.
        #[serde(rename = "version")]
        pub protocol_version: ProtocolVersion,
        /// Data used by block builders to check if the bundle should be considered for inclusion.
        #[serde(rename = "inclusion")]
        pub inclusion: Inclusion,
        /// The transactions to include in the bundle.
        #[serde(rename = "body")]
        pub bundle_body: Vec<BundleItem>,
        /// Requirements for the bundle to be included in the block.
        #[serde(rename = "validity", skip_serializing_if = "Option::is_none")]
        pub validity: Option<Validity>,
        /// Preferences on what data should be shared about the bundle and its transactions.
        #[serde(rename = "privacy", skip_serializing_if = "Option::is_none")]
        pub privacy: Option<Privacy>,
    }

    /// Data used by block builders to check if the bundle should be considered for inclusion.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Inclusion {
        /// The first block the bundle is valid for.
        #[serde(with = "crate::mev_share::serde_quantity")]
        pub block: u64,
        /// The last block the bundle is valid for.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::mev_share::serde_quantity::opt"
        )]
        pub max_block: Option<u64>,
    }

    /// A bundle body item.
    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    #[serde(untagged)]
    #[serde(rename_all = "camelCase")]
    pub enum BundleItem {
        /// The hash of either a transaction or bundle we are trying to backrun.
        Hash {
            /// Transaction hash.
            hash: B256,
        },
        /// A new signed transaction.
        #[serde(rename_all = "camelCase")]
        Tx {
            /// Bytes of the signed transaction.
            tx: Bytes,
            /// If true, the transaction can revert without the bundle being considered invalid.
            can_revert: bool,
        },
    }

    /// Requirements for the bundle to be included in the block.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Validity {
        /// Minimum refund requirements for body items.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub refund: Option<Vec<Refund>>,
        /// Overall refund recipients.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub refund_config: Option<Vec<RefundConfig>>,
    }

    /// Minimum refund requirement.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Refund {
        /// Bundle body index.
        pub body_idx: u64,
        /// Refund percent.
        pub percent: u64,
    }

    /// Refund recipient configuration.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct RefundConfig {
        /// Address to refund.
        pub address: Address,
        /// Refund percent.
        pub percent: u64,
    }

    /// Bundle privacy preferences.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct Privacy {
        /// Hints to share.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub hints: Option<PrivacyHint>,
        /// Builders that should be allowed to see the bundle.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub builders: Option<Vec<Address>>,
    }

    /// Hints on what data should be shared about the bundle and its transactions.
    #[derive(Clone, Debug, PartialEq, Default)]
    pub struct PrivacyHint {
        pub calldata: bool,
        pub contract_address: bool,
        pub logs: bool,
        pub function_selector: bool,
        pub hash: bool,
        pub tx_hash: bool,
    }

    impl PrivacyHint {
        pub fn with_calldata(mut self) -> Self {
            self.calldata = true;
            self
        }

        pub fn with_contract_address(mut self) -> Self {
            self.contract_address = true;
            self
        }

        pub fn with_logs(mut self) -> Self {
            self.logs = true;
            self
        }

        pub fn with_function_selector(mut self) -> Self {
            self.function_selector = true;
            self
        }

        pub fn with_hash(mut self) -> Self {
            self.hash = true;
            self
        }

        pub fn with_tx_hash(mut self) -> Self {
            self.tx_hash = true;
            self
        }

        fn num_hints(&self) -> usize {
            self.calldata as usize
                + self.contract_address as usize
                + self.logs as usize
                + self.function_selector as usize
                + self.hash as usize
                + self.tx_hash as usize
        }
    }

    impl Serialize for PrivacyHint {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut seq = serializer.serialize_seq(Some(self.num_hints()))?;
            if self.calldata {
                seq.serialize_element("calldata")?;
            }
            if self.contract_address {
                seq.serialize_element("contract_address")?;
            }
            if self.logs {
                seq.serialize_element("logs")?;
            }
            if self.function_selector {
                seq.serialize_element("function_selector")?;
            }
            if self.hash {
                seq.serialize_element("hash")?;
            }
            if self.tx_hash {
                seq.serialize_element("tx_hash")?;
            }
            seq.end()
        }
    }

    impl<'de> Deserialize<'de> for PrivacyHint {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let hints = Vec::<String>::deserialize(deserializer)?;
            let mut privacy_hint = PrivacyHint::default();
            for hint in hints {
                match hint.as_str() {
                    "calldata" => privacy_hint.calldata = true,
                    "contract_address" => privacy_hint.contract_address = true,
                    "logs" => privacy_hint.logs = true,
                    "function_selector" => privacy_hint.function_selector = true,
                    "hash" => privacy_hint.hash = true,
                    "tx_hash" => privacy_hint.tx_hash = true,
                    _ => return Err(de::Error::custom("invalid privacy hint")),
                }
            }
            Ok(privacy_hint)
        }
    }

    /// Response from the matchmaker after sending a bundle.
    #[derive(Deserialize, Debug, Serialize, Clone, PartialEq)]
    #[serde(rename_all = "camelCase")]
    pub struct SendBundleResponse {
        /// Hash of the bundle bodies.
        pub bundle_hash: B256,
    }

    /// The MEV-Share API version.
    #[derive(Deserialize, Debug, Serialize, Clone, Default, PartialEq)]
    pub enum ProtocolVersion {
        #[default]
        #[serde(rename = "beta-1")]
        Beta1,
        #[serde(rename = "v0.1")]
        V0_1,
    }

    impl SendBundleRequest {
        /// Create a new bundle request.
        pub fn new(
            block_num: u64,
            max_block: Option<u64>,
            protocol_version: ProtocolVersion,
            bundle_body: Vec<BundleItem>,
        ) -> Self {
            Self {
                protocol_version,
                inclusion: Inclusion {
                    block: block_num,
                    max_block,
                },
                bundle_body,
                validity: None,
                privacy: None,
            }
        }
    }
}

pub mod sse {
    use std::{array::TryFromSliceError, fmt::LowerHex, ops::Deref};

    use alloy::primitives::{Address, Bytes, B256, U256};
    use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};

    /// SSE event from the MEV-Share endpoint.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Event {
        /// Transaction or bundle hash.
        pub hash: B256,
        /// Transactions from the event.
        #[serde(rename = "txs", with = "null_sequence")]
        pub transactions: Vec<EventTransaction>,
        /// Event logs emitted by executing the transaction.
        #[serde(with = "null_sequence")]
        pub logs: Vec<EventTransactionLog>,
    }

    /// Transaction from the event.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct EventTransaction {
        /// Transaction recipient address.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub to: Option<Address>,
        /// 4-byte function selector.
        #[serde(rename = "functionSelector", skip_serializing_if = "Option::is_none")]
        pub function_selector: Option<FunctionSelector>,
        /// Calldata of the transaction.
        #[serde(rename = "callData", skip_serializing_if = "Option::is_none")]
        pub calldata: Option<Bytes>,
    }

    /// A log produced by a transaction.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct EventTransactionLog {
        /// The address of the contract that emitted the log.
        pub address: Address,
        /// Log topics.
        pub topics: Vec<B256>,
    }

    /// Historic SSE event hint.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Hint {
        #[serde(with = "null_sequence")]
        pub txs: Vec<EventTransaction>,
        pub hash: B256,
        #[serde(with = "null_sequence")]
        pub logs: Vec<EventTransactionLog>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gas_used: Option<U256>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mev_gas_price: Option<U256>,
    }

    /// 4-byte function selector.
    #[derive(Clone, PartialEq, Eq, Hash)]
    pub struct FunctionSelector(pub [u8; 4]);

    impl FunctionSelector {
        fn hex_encode(&self) -> String {
            alloy::hex::encode(self.0.as_ref())
        }
    }

    impl Serialize for FunctionSelector {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&self.to_string())
        }
    }

    impl<'de> Deserialize<'de> for FunctionSelector {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let hex_str = String::deserialize(deserializer)?;
            let s = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
            if s.len() != 8 {
                return Err(serde::de::Error::custom(format!(
                    "Expected 4 byte function selector: {hex_str}"
                )));
            }

            let bytes = alloy::hex::decode(s).map_err(serde::de::Error::custom)?;
            FunctionSelector::try_from(bytes.as_slice()).map_err(serde::de::Error::custom)
        }
    }

    impl AsRef<[u8]> for FunctionSelector {
        fn as_ref(&self) -> &[u8] {
            self.0.as_ref()
        }
    }

    impl std::fmt::Debug for FunctionSelector {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("FunctionSelector")
                .field(&self.hex_encode())
                .finish()
        }
    }

    impl std::fmt::Display for FunctionSelector {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "0x{}", self.hex_encode())
        }
    }

    impl LowerHex for FunctionSelector {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "0x{}", self.hex_encode())
        }
    }

    impl Deref for FunctionSelector {
        type Target = [u8];

        fn deref(&self) -> &[u8] {
            self.as_ref()
        }
    }

    impl From<[u8; 4]> for FunctionSelector {
        fn from(src: [u8; 4]) -> Self {
            Self(src)
        }
    }

    impl<'a> TryFrom<&'a [u8]> for FunctionSelector {
        type Error = TryFromSliceError;

        fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
            let sel: [u8; 4] = value.try_into()?;
            Ok(Self(sel))
        }
    }

    mod null_sequence {
        use super::*;

        pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
        where
            D: Deserializer<'de>,
            T: DeserializeOwned,
        {
            Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
        }

        pub(crate) fn serialize<T, S>(val: &Vec<T>, serializer: S) -> Result<S::Ok, S::Error>
        where
            T: Serialize,
            S: Serializer,
        {
            if val.is_empty() {
                serializer.serialize_none()
            } else {
                val.serialize(serializer)
            }
        }
    }
}

mod serde_quantity {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{value:x}"))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let value = value.strip_prefix("0x").unwrap_or(&value);
        u64::from_str_radix(value, 16).map_err(serde::de::Error::custom)
    }

    pub mod opt {
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(value) => serializer.serialize_some(&format!("0x{value:x}")),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let Some(value) = Option::<String>::deserialize(deserializer)? else {
                return Ok(None);
            };
            let value = value.strip_prefix("0x").unwrap_or(&value);
            u64::from_str_radix(value, 16)
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
    }
}

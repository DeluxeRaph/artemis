use alloy::sol;

sol! {
    #[derive(Debug, Default, PartialEq, Eq, Hash)]
    struct AdditionalRecipient {
        uint256 amount;
        address recipient;
    }

    #[derive(Debug, Default, PartialEq, Eq, Hash)]
    struct BasicOrderParameters {
        address consideration_token;
        uint256 consideration_identifier;
        uint256 consideration_amount;
        address offerer;
        address zone;
        address offer_token;
        uint256 offer_identifier;
        uint256 offer_amount;
        uint8 basic_order_type;
        uint256 start_time;
        uint256 end_time;
        bytes32 zone_hash;
        uint256 salt;
        bytes32 offerer_conduit_key;
        bytes32 fulfiller_conduit_key;
        uint256 total_original_additional_recipients;
        AdditionalRecipient[] additional_recipients;
        bytes signature;
    }

    #[derive(Debug, Default, PartialEq, Eq, Hash)]
    struct SellQuote {
        bool quote_available;
        address nft_address;
        uint256 price;
    }

    interface SudoOpenseaArb {
        function executeArb(
            BasicOrderParameters basic_order,
            uint256 payment_value,
            address sudo_pool
        );
    }

    interface SudoPairQuoter {
        function getMultipleSellQuotes(address[] pool_addresses)
            view
            returns (SellQuote[] sell_quotes);
    }

    interface LSSVMPairFactory {
        event NewPair(address pool_address);
    }

    interface LSSVMPair {
        event SwapNFTInPair();
        event SwapNFTOutPair();
        event SpotPriceUpdate(uint128 new_spot_price);
        event TokenWithdrawal(uint256 amount);
    }
}

pub mod zone_interface {
    pub use super::{AdditionalRecipient, BasicOrderParameters};
}

pub mod sudo_pair_quoter {
    pub use super::{SellQuote, SudoPairQuoter};
    pub use crate::sudo_pair_quoter_bytecode::SUDOPAIRQUOTER_DEPLOYED_BYTECODE;
}

pub mod sudo_opensea_arb {
    pub use super::SudoOpenseaArb;
}

pub mod lssvm_pair_factory {
    pub use super::LSSVMPairFactory;
}

pub mod lssvm_pair {
    pub use super::LSSVMPair;
}

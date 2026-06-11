use alloy::sol_types::SolEvent;
use ethers::{
    prelude::Lazy,
    types::{Address, H256 as TxHash},
};

/// Block number at which the sudo factory was deployed.
pub const FACTORY_DEPLOYMENT_BLOCK: u64 = 14650730;

/// Address of the sudo pair factory.
pub static LSSVM_PAIR_FACTORY_ADDRESS: Lazy<Address> = Lazy::new(|| {
    "0xb16c1342e617a5b6e4b631eb114483fdb289c0a4"
        .parse()
        .unwrap()
});

/// Group of event signatures which are emitted when a pool is touched.
pub static POOL_EVENT_SIGNATURES: Lazy<Vec<TxHash>> = Lazy::new(|| {
    vec![
        alloy_b256_to_ethers(bindings::lssvm_pair::LSSVMPair::SwapNFTInPair::SIGNATURE_HASH),
        alloy_b256_to_ethers(bindings::lssvm_pair::LSSVMPair::SwapNFTOutPair::SIGNATURE_HASH),
        alloy_b256_to_ethers(bindings::lssvm_pair::LSSVMPair::SpotPriceUpdate::SIGNATURE_HASH),
        alloy_b256_to_ethers(bindings::lssvm_pair::LSSVMPair::TokenWithdrawal::SIGNATURE_HASH),
    ]
});

fn alloy_b256_to_ethers(value: alloy::primitives::B256) -> TxHash {
    TxHash::from_slice(value.as_slice())
}

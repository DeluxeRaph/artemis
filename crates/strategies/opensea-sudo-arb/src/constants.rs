use alloy::primitives::{address, Address, B256};
use alloy::sol_types::SolEvent;
use std::sync::LazyLock;

/// Block number at which the sudo factory was deployed.
pub const FACTORY_DEPLOYMENT_BLOCK: u64 = 14650730;

/// Address of the sudo pair factory.
pub const LSSVM_PAIR_FACTORY_ADDRESS: Address =
    address!("b16c1342e617a5b6e4b631eb114483fdb289c0a4");

/// Group of event signatures which are emitted when a pool is touched.
pub static POOL_EVENT_SIGNATURES: LazyLock<Vec<B256>> = LazyLock::new(|| {
    vec![
        bindings::lssvm_pair::LSSVMPair::SwapNFTInPair::SIGNATURE_HASH,
        bindings::lssvm_pair::LSSVMPair::SwapNFTOutPair::SIGNATURE_HASH,
        bindings::lssvm_pair::LSSVMPair::SpotPriceUpdate::SIGNATURE_HASH,
        bindings::lssvm_pair::LSSVMPair::TokenWithdrawal::SIGNATURE_HASH,
    ]
});

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Result;
use artemis_core::{
    collectors::mevshare_collector::MevShareCollector,
    engine::Engine,
    executors::mev_share_executor::MevshareExecutor,
    types::{CollectorMap, ExecutorMap},
};
use clap::Parser;
use mev_share_uni_arb::{
    strategy::MevShareUniArb,
    types::{Action, Event},
};
use tracing::{info, Level};
use tracing_subscriber::{filter, prelude::*};

/// CLI Options.
#[derive(Parser, Debug)]
pub struct Args {
    /// Ethereum node WS endpoint.
    #[arg(long)]
    pub wss: String,
    /// Private key for sending txs.
    #[arg(long)]
    pub private_key: String,
    /// MEV share signer
    #[arg(long)]
    pub flashbots_signer: String,
    /// Address of the arb contract.
    #[arg(long)]
    pub arb_contract_address: Address,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up tracing and parse args.
    let filter = filter::Targets::new()
        .with_target("mev_share_uni_arb", Level::INFO)
        .with_target("artemis_core", Level::INFO);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    let args = Args::parse();

    // Set up the Alloy signer used by the migrated MEV-share executor.
    let fb_signer: PrivateKeySigner = args.flashbots_signer.parse()?;
    let (strategy_client, strategy_wallet) =
        strategy_ethers_bridge::build_strategy_client(&args.wss, &args.private_key).await?;

    // Set up engine.
    let mut engine: Engine<Event, Action> = Engine::default();

    // Set up collector.
    let mevshare_collector = Box::new(MevShareCollector::new(String::from(
        "https://mev-share.flashbots.net",
    )));
    let mevshare_collector = CollectorMap::new(mevshare_collector, Event::MEVShareEvent);
    engine.add_collector(Box::new(mevshare_collector));

    // Set up strategy.
    let strategy = MevShareUniArb::new(strategy_client, strategy_wallet, args.arb_contract_address);
    engine.add_strategy(Box::new(strategy));

    // Set up executor.
    let mev_share_executor = Box::new(MevshareExecutor::new(fb_signer));
    let mev_share_executor = ExecutorMap::new(mev_share_executor, |action| match action {
        Action::SubmitBundle(bundle) => Some(bundle),
    });
    engine.add_executor(Box::new(mev_share_executor));

    // Start engine.
    if let Ok(mut set) = engine.run().await {
        while let Some(res) = set.join_next().await {
            info!("res: {:?}", res);
        }
    }

    Ok(())
}

mod strategy_ethers_bridge {
    use std::sync::Arc;

    use anyhow::Result;
    use ethers::{
        middleware::{NonceManagerMiddleware, SignerMiddleware},
        prelude::MiddlewareBuilder,
        providers::{Provider as EthersProvider, Ws},
        signers::{LocalWallet, Signer},
    };

    pub type StrategyClient =
        SignerMiddleware<NonceManagerMiddleware<EthersProvider<Ws>>, LocalWallet>;

    /// Compatibility client for the MEV-share strategy's ethers-shaped upstream APIs.
    ///
    /// Keep ethers construction here until the strategy and MEV-share request types
    /// can move fully to Alloy.
    pub async fn build_strategy_client(
        wss: &str,
        private_key: &str,
    ) -> Result<(Arc<StrategyClient>, LocalWallet)> {
        let ws = Ws::connect(wss).await?;
        let provider = EthersProvider::new(ws);
        let wallet: LocalWallet = private_key.parse()?;
        let address = wallet.address();
        let client = provider.nonce_manager(address).with_signer(wallet.clone());

        Ok((Arc::new(client), wallet))
    }
}

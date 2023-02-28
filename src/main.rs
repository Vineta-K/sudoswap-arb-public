pub mod abigen;
pub mod bot;
// pub mod database;
pub mod events;
pub mod execution;
pub mod looks;
pub mod market_searcher;
pub mod multi;
pub mod nftx;
pub mod sudo;
pub mod tests;
pub mod txpool;
pub mod utils;

use ethers::prelude::*;
use ethers_flashbots::*;

use eyre::eyre;
use gumdrop::Options;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc, collections::HashSet};
use tracing_subscriber::{filter::EnvFilter, fmt::Subscriber};
use url::Url;

use crate::bot::Bot;

pub const ONE: u128 = 1_000_000_000_000_000_000;

// CLI Options
#[derive(Debug, Options, Clone)]
pub struct Opts {
    help: bool,

    #[options(
        help = "path to json file with the contract addresses",
        default = "config.json"
    )]
    config: PathBuf,

    #[options(help = "path to json file with the api keys", default = "apikeys.json")]
    api_keys: PathBuf,

    #[options(help = "node_uri")]
    node_uri: String,

    #[options(help = "polling interval (ms)", default = "50")]
    interval: u64,

    #[options(help = "the db to be used for state", default = "state.sqlite")]
    database: PathBuf,

    #[options(help = "number of calls to concurrently send", default = "1024")]
    n_buffered_calls: usize,

    #[options(help = "compute_limit", default = "0")]
    compute_limit: u64,

    #[options(help = "max events to dl at once", default = "0")]
    n_events: u64,

    #[options(
        help = "Deployed block - can override for testing",
        default = "14033905"
    )]
    deployed_block: u64,

    #[options(help = "Local simulate", default = "false")]
    local_simulate: bool,
}

#[derive(Debug)]
pub struct FeePercent {
    contested: U256,
    uncontested: U256,
}

#[derive(Deserialize)]
pub struct Config {
    #[serde(rename = "PairFactory")]
    sudo_factory_address: Address,
    #[serde(rename = "SudoRouter")]
    sudo_router_address: Address,
    #[serde(rename = "Aggregators")]
    aggregator_addresses: HashSet<Address>,
    #[serde(rename = "SushiRouter")]
    sushi_router_address: Address,
    #[serde(rename = "NFTXZap")]
    nftx_zap_address: Address,
    #[serde(rename = "WETH")]
    weth_address: Address,
    // #[serde(rename = "ETH")]
    // eth_address: Address,
    #[serde(rename = "NFTxFactory")]
    nftx_factory_address: Address,
    #[serde(rename = "GasHeuristic")]
    gas_heuristic: u128,
    #[serde(rename = "MinimumProfit")]
    minimum_profit: u128,
    #[serde(rename = "PercentToMiner")]
    percent_to_miner: Vec<u128>,
    #[serde(rename = "BannedAddresses")]
    banned_nfts: Vec<Address>,
    #[serde(rename = "LooksRareExchange")]
    looks_exchange_address: Address,
}

#[derive(Deserialize)]
pub struct ApiKeys {
    #[serde(rename = "LooksRareEndpoint")]
    looks_rare_endpoint: String,
    #[serde(rename = "BotAddress")]
    bot_address: Address,
    #[serde(rename = "BotKey")]
    bot_key: String,
    #[serde(rename = "FlashbotKey")]
    fb_key: String,
    #[serde(rename = "MultiAddress")]
    multi_address: Address,
    #[serde(rename = "RelayEndpoints")]
    relay_uris: Vec<String>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    //Set up tracing
    Subscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let opts = Opts::parse_args_default_or_exit(); //Parse command line options

    if opts.node_uri.starts_with("ws") {
        let ws = Ws::connect(opts.node_uri.clone()).await?;
        let provider = Provider::new(ws);
        let http_uri = opts.node_uri.replace("ws", "http");
        let http_provider = Provider::<Http>::try_from(http_uri)?;
        run(opts, provider.clone(), provider, http_provider).await?;
    }
    // else {
    //     let provider = Provider::<Ipc>::connect_ipc(opts.node_uri.clone());
    // }
    Err(eyre!(
        "Failed to create provider, check --node-uri is set and correct -websockets only till I fix some generics!"
    ))
}

async fn run<P, Q, S>(
    opts: Opts,
    provider: Provider<P>,
    fb_provider: Provider<Q>,
    http_provider: Provider<S>,
) -> eyre::Result<()>
where
    P: PubsubClient + 'static,
    Q: JsonRpcClient + 'static,
    S: JsonRpcClient + 'static,
{
    let cfg: Config = serde_json::from_reader(std::fs::File::open(&opts.config)?)?;
    let api_keys: ApiKeys = serde_json::from_reader(std::fs::File::open(&opts.api_keys)?)?;

    //Flashbots client setup (for simulating/sending bundles)
    let fb_signer = api_keys.fb_key.parse::<LocalWallet>()?;
    let wallet = api_keys.bot_key.parse::<LocalWallet>()?;
    let fb_provider = Arc::new(fb_provider);
    let mut fb_clients = Vec::new();
    for uri in &api_keys.relay_uris {
        fb_clients.push(SignerMiddleware::new(
            FlashbotsMiddleware::new(
                fb_provider.clone(),
                Url::parse(uri.as_str())?,
                fb_signer.clone(),
            ),
            wallet.clone(),
        ))
    } 

    //General client setup (for calls)
    //let provider = provider.interval(Duration::from_millis(opts.interval));
    let client = Arc::new(provider);
    let http_client = Arc::new(http_provider);

    //Run bot
    let mut bot: Bot<_, _, _, _> =
        Bot::new(client, http_client, fb_clients, opts, cfg, api_keys).await?;
    bot.run().await?;

    Ok(())
}

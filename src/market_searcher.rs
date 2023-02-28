use std::collections::HashSet;
use std::fmt::Display;
use std::sync::Arc;

use ethers::prelude::*;
use ethers::providers::call_raw::spoof::State;
use tracing::info;

use crate::abigen::{LooksRareExchange, NFTxFactory, SudoPairFactory, UniRouterV2, ERC721};
use crate::events::{get_events_as_iter, Event, NFTxNewVaultEvent, SudoNewPairEvent};
use crate::looks::{LooksOrderbook, LooksQuote};
use crate::multi::Call;
use crate::nftx::NFTxVault;
use crate::sudo::SudoPool;
// use crate::utils::send_tg;

#[derive(Debug, Clone)]
pub enum MarketType<M: Middleware> {
    Sudo(SudoPool<M>),
    Looks(Box<LooksQuote>),
    NFTx(NFTxVault<M>),
}
impl<M> Display for MarketType<M>
where
    M: Middleware,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarketType::Sudo(_) => write!(f, "Sudo"),
            MarketType::Looks(_) => write!(f, "Looks"),
            MarketType::NFTx(_) => write!(f, "NFTx"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Marketplace<M: Middleware> {
    pub market_type: MarketType<M>,
    pub nft_contract: ERC721<M>,
    pub nft_address: Address,
}

impl<M> Marketplace<M>
where
    M: Middleware + 'static,
{
    pub async fn create_buy_txs(
        &self,
        address: Address,
        input: U256,
        num_nft: usize,
        router: &UniRouterV2<M>,
        exchange: &LooksRareExchange<M>,
        weth_address: Address,
    ) -> eyre::Result<(Option<Vec<Call>>, Option<Vec<U256>>)> {
        match &self.market_type {
            MarketType::Sudo(pool) => match &pool.contract {
                crate::sudo::SudoContract::ETH(_) => {
                    let (calls, ids) = pool.create_buy_txs(input, address, num_nft).await?;
                    Ok((Some(calls), ids))
                }
                crate::sudo::SudoContract::ERC20(_) => todo!(),
            },
            MarketType::NFTx(vault) => {
                let (calls, ids) = vault
                    .create_buy_txs(input, num_nft,address, router, weth_address)
                    .await?;
                Ok((Some(calls), ids))
            }
            MarketType::Looks(quote) => {
                let (calls, ids) = quote.create_buy_txs(address, exchange).await?;
                Ok((Some(calls), ids))
            }
        }
    }
    pub async fn create_sell_txs(
        &self,
        address: Address,
        output: U256,
        ids: Vec<U256>,
        router: &UniRouterV2<M>,
        exchange: &LooksRareExchange<M>,
        weth_address: Address,
    ) -> eyre::Result<Option<Vec<Call>>> {
        match &self.market_type {
            MarketType::Sudo(pool) => match &pool.contract {
                crate::sudo::SudoContract::ETH(_) => {
                    let calls = pool
                        .create_sell_txs(output, address, ids, &self.nft_contract)
                        .await?;
                    Ok(Some(calls))
                }
                crate::sudo::SudoContract::ERC20(_) => todo!(),
            },
            MarketType::NFTx(vault) => {
                let calls = vault
                    .create_sell_txs(
                        output,
                        address,
                        ids,
                        router,
                        &self.nft_contract,
                        weth_address,
                    )
                    .await?;
                Ok(Some(calls))
            }
            MarketType::Looks(quote) => {
                let calls = quote
                    .create_sell_txs(address, exchange, &self.nft_contract, weth_address)
                    .await?;
                Ok(Some(calls))
            }
        }
    }

    pub fn contract_address(&self) -> Address {
        match &self.market_type {
            MarketType::Sudo(pool) => pool.pool_address,
            MarketType::Looks(quote) => quote.collection_address,
            MarketType::NFTx(vault) => vault.vault_address,
        }
    }
}

// #[derive(Clone)]
pub struct MarketSearcher<M: Middleware, O: Middleware> {
    pub client: Arc<M>,
    pub sudo_factory: SudoPairFactory<O>,
    pub nftx_factory: NFTxFactory<O>,
    pub looks_exchange: LooksRareExchange<M>,
    pub looksrare_endpoint: String,
    pub marketplaces: Vec<Marketplace<M>>,
    pub marketplace_addresses: HashSet<Address>,
    pub http_client: Arc<reqwest::Client>,
    pub nftx_router: UniRouterV2<M>,
    pub weth_address: Address,
    pub looks_orderbook: LooksOrderbook,
}

impl<M, O> MarketSearcher<M, O>
where
    M: Middleware + 'static,
    O: Middleware + 'static,
{
    pub async fn new(
        client: Arc<M>,
        sudo_factory: SudoPairFactory<O>,
        nftx_factory: NFTxFactory<O>,
        looks_exchange_address: Address,
        looksrare_endpoint: String,
        nftx_router_address: Address,
        weth_address: Address,
        looks_orderbook: LooksOrderbook,
    ) -> eyre::Result<MarketSearcher<M, O>> {
        let marketplaces = Vec::new();
        let http_client = Arc::new(reqwest::Client::new());
        let nftx_router = UniRouterV2::new(nftx_router_address, client.clone());
        let looks_exchange = LooksRareExchange::new(looks_exchange_address, client.clone());
        let marketplace_addresses = HashSet::new();
        Ok(Self {
            client,
            sudo_factory,
            looksrare_endpoint,
            nftx_factory,
            looks_exchange,
            marketplaces,
            http_client,
            nftx_router,
            weth_address,
            marketplace_addresses,
            looks_orderbook,
        })
    }

    pub async fn get_new_market_events(
        &self,
        start_block: U64,
        end_block: U64,
    ) -> eyre::Result<Vec<Event>> {
        info!(
            "Getting events from blocks {} to {}",
            start_block, end_block
        );
        let mut events = Vec::new();
        events.extend(
            get_events_as_iter(
                self.sudo_factory.new_pair_filter(),
                SudoNewPairEvent::from_new_pair_filter(),
                end_block,
                start_block,
            )
            .await?,
        );
        events.extend(
            get_events_as_iter(
                self.nftx_factory.new_vault_filter(),
                NFTxNewVaultEvent::from_new_vault_filter(),
                end_block,
                start_block,
            )
            .await?,
        ); //TODO -> fix nftx pricing call reverts

        info!("Collected {} events", events.len());
        Ok(events)
    }

    pub async fn get_new_market_events_limited(
        &self,
        start_block: U64,
        end_block: U64,
        limit: u64,
    ) -> eyre::Result<Vec<Event>> {
        let big_n = ((end_block - start_block) / limit).as_u64(); //fuck you clippy i wanted a capital N
        info!("Need {} calls", big_n + 1);

        let mut events = Vec::new();
        for n in (0..=big_n).map(|n| end_block - n * limit).rev() {
            //Ensure we don't overshoot start block which will lead to double counting events (will it?)
            let n_minus = if n - limit < start_block {
                start_block
            } else {
                n - limit
            };
            info!(
                "Getting events from blocks {} - {} ({} Blocks)",
                n_minus,
                n,
                n - n_minus
            );
            events.extend(
                get_events_as_iter(
                    self.sudo_factory.new_pair_filter(),
                    SudoNewPairEvent::from_new_pair_filter(),
                    n,
                    n_minus,
                )
                .await?,
            );
            events.extend(
                get_events_as_iter(
                    self.nftx_factory.new_vault_filter(),
                    NFTxNewVaultEvent::from_new_vault_filter(),
                    n,
                    n_minus,
                )
                .await?,
            ); //TODO -> fix nftx pricing call reverts
        }
        info!("Collected {} events", events.len());
        Ok(events)
    }

    pub async fn get_new_logs(&self, start_block: U64, end_block: U64) -> eyre::Result<Vec<Log>> {
        let filter = Filter::new().from_block(start_block).to_block(end_block);
        let logs = self.client.get_logs(&filter).await?;
        info!("Collected {} logs", logs.len());
        Ok(logs)
    }

    pub async fn get_new_logs_limited(
        &self,
        start_block: U64,
        end_block: U64,
    ) -> eyre::Result<Vec<Log>> {
        let big_n = (end_block - start_block).as_u64(); //fuck you clippy i wanted a capital N
        info!("Need {} calls", big_n);
        let mut logs = Vec::new();
        for n in (0..big_n).map(|n| end_block - n).rev() {
            //Ensure we don't overshoot start block which will lead to double counting events
            let n_minus = n - 1;
            info!(
                "Processing logs from block {} - {} ({} Blocks)",
                n_minus,
                n,
                n - n_minus
            );
            let filter = Filter::new().from_block(n_minus).to_block(n);
            let mut new = self.client.get_logs(&filter).await?;
            logs.append(&mut new);
        }
        Ok(logs)
    }

    pub async fn get_prices(
        &self,
        marketplace: &Marketplace<M>,
        num_nft: usize,
        diff: Option<&State>,
    ) -> eyre::Result<(Option<U256>, Option<U256>)> {
        match &marketplace.market_type {
            MarketType::Sudo(pool) => {
                let buy_price = pool.get_buy_price(num_nft, diff).await?;
                let sell_price = pool
                    .get_sell_price(self.client.clone(), num_nft, diff)
                    .await?;
                Ok((buy_price, sell_price))
            }
            MarketType::NFTx(vault) => {
                let buy_price = vault
                    .get_buy_price(&self.nftx_router, num_nft, self.weth_address)
                    .await?;
                let sell_price = vault
                    .get_sell_price(&self.nftx_router, num_nft, self.weth_address)
                    .await?;
                Ok((buy_price, sell_price))
            }
            MarketType::Looks(quote) => match quote.is_order_ask {
                true => Ok((quote.get_price(), None)),
                false => Ok((None, None)), //maybe put this logic in the quote struct impl
            },
        }
    }
}

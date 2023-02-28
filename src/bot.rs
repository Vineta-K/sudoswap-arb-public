use ethers::prelude::*;
use ethers::providers::call_raw::spoof::{self, State};
use ethers_flashbots::*;
use eyre::eyre;
use parking_lot::Mutex;
use tokio::select;
use tokio::time::{self, Instant};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, info, warn};

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use crate::abigen::{Multi, NFTxFactory, NFTxVaultContract, SudoPairETH, SudoPairFactory, ERC721};
use crate::events::Event;
use crate::execution::Executor;
use crate::looks::LooksOrderbook;
use crate::market_searcher::{MarketSearcher, MarketType, Marketplace};
use crate::multi::Call;
use crate::nftx::NFTxVault;
use crate::sudo::{PoolType, SudoContract, SudoPool};
use crate::txpool::TxpoolTx;
use crate::utils::{apply_state_diff, complete_buffered, send_tg, to_eth};
use crate::{ApiKeys, Config, Opts};
use crate::{FeePercent, ONE};

pub struct RouterAddresses {
    pub sudo_router_address: Address,
    pub aggregator_addresses: HashSet<Address>,
    pub sushi_router_address: Address,
    pub nftx_zap_address: Address,
}

#[derive(Debug, Eq, Clone, Copy)]
pub struct ArbInfo {
    block_number: U64,
    inds: (usize, usize),
    quotes: (U256, U256),
    delta: U256,
    nft_address: Address,
    _breakeven_gas_price: Option<U256>,
}

impl Ord for ArbInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.delta.cmp(&other.delta)
    }
}
impl PartialOrd for ArbInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for ArbInfo {
    fn eq(&self, other: &Self) -> bool {
        self.delta == other.delta
    }
}
impl Display for ArbInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NFT address: {:?}, buy ({}): {}, sell ({}): {}, delta: {}",
            self.nft_address,
            self.inds.0,
            to_eth(self.quotes.0),
            self.inds.1,
            to_eth(self.quotes.1),
            to_eth(self.delta),
        )
    }
}

impl ArbInfo {
    pub async fn update(&mut self, _new: ArbInfo) {}
} // TODO

#[derive(Debug, Default, Clone)]
pub struct NFTMarket {
    best_buy_quote: Option<U256>,
    best_sell_quote: Option<U256>,
    buy_marketplace_ind: Option<usize>,
    sell_marketplace_ind: Option<usize>,
    all_marketplace_inds: HashSet<usize>,
} //Todo -> implement update function on this struct rather than seperate

#[derive(Debug)]
struct BotOpts {
    gas_heuristic: U256,
    percent_to_miner: FeePercent,
    minimum_profit: U256,
    n_buffered_calls: usize,
    n_events: u64,
}

pub struct Bot<M: Middleware, N: Middleware, O: Middleware, S: Signer> {
    address: Address,
    client: Arc<M>,
    _http_client: Arc<O>,
    router_addresses: RouterAddresses,
    banned_nfts: HashSet<Address>,
    executor: Executor<M, N, S>,
    full_update: bool,
    last_updated_block: U64,
    market_searcher: MarketSearcher<M, O>,
    nft_markets: HashMap<Address, NFTMarket>,
    opts: BotOpts,
}

impl<M, N, O, S> Bot<M, N, O, S>
where
    M: Middleware + 'static,
    <M as ethers::providers::Middleware>::Provider: PubsubClient,
    N: Middleware + 'static,
    O: Middleware + 'static,
    S: Signer + 'static,
{
    pub async fn new(
        client: Arc<M>,
        http_client: Arc<O>,
        fb_clients: Vec<SignerMiddleware<FlashbotsMiddleware<N, S>, S>>, //I gave up on the onion
        opts: Opts,
        config: Config,
        api_keys: ApiKeys,
    ) -> eyre::Result<Bot<M, N, O, S>> {
        info!("Initialising bot");
        let last_updated_block = opts.deployed_block;
        let last_updated_block: U64 = last_updated_block.into();

        let looksrare_endpoint = api_keys.looks_rare_endpoint;
        let weth_address = config.weth_address;
        let router_addresses = RouterAddresses {
            sudo_router_address: config.sudo_router_address,
            aggregator_addresses: config.aggregator_addresses,
            sushi_router_address: config.sushi_router_address,
            nftx_zap_address: config.nftx_zap_address,
        };

        let executor_contract = Multi::new(api_keys.multi_address, client.clone());
        let executor = Executor::new(fb_clients, executor_contract, opts.local_simulate).await?;
        let address = api_keys.bot_address;
        let r_client = Arc::new(reqwest::Client::new());
        let looks_orderbook = LooksOrderbook::new(
            r_client,
            String::from("https://api.looksrare.org/"),
            Vec::new(),
        );
        let opts = BotOpts {
            gas_heuristic: config.gas_heuristic.into(),
            percent_to_miner: FeePercent {
                contested: config.percent_to_miner[0].into(),
                uncontested: config.percent_to_miner[1].into(),
            },
            n_buffered_calls: opts.n_buffered_calls,
            n_events: opts.n_events,
            minimum_profit: config.minimum_profit.into(),
        };
        let sudo_factory = SudoPairFactory::new(config.sudo_factory_address, http_client.clone()); //http so we can get big events
        let nftx_factory = NFTxFactory::new(config.nftx_factory_address, http_client.clone()); //http so we can get big events
        let market_searcher = MarketSearcher::new(
            client.clone(),
            sudo_factory,
            nftx_factory,
            config.looks_exchange_address,
            looksrare_endpoint,
            router_addresses.sushi_router_address,
            weth_address,
            looks_orderbook,
        )
        .await?;
        let nft_markets = HashMap::new();
        let banned_nfts: HashSet<Address> = config.banned_nfts.into_iter().collect();

        Ok(Self {
            client,
            _http_client: http_client,
            executor,
            router_addresses,
            banned_nfts,
            market_searcher,
            nft_markets,
            last_updated_block,
            address,
            opts,
            full_update: true,
        })
    }

    pub async fn run(&mut self) -> eyre::Result<()> {
        let block_watcher = self.client.clone();
        let pending_tx_watcher = self.client.clone();
        let mut block_stream = block_watcher.subscribe_blocks().await?;
        let mut tx_stream = pending_tx_watcher.subscribe_pending_txs().await?; //vec for testing compatibility -> fix later
        let interval = time::interval(Duration::from_millis(1000));
        let mut looks_update_stream = IntervalStream::new(interval);
        loop {
            select! {
                Some(_) = block_stream.next() => {
                    self.on_block().await?
                }
                Some(t) = tx_stream.next() => {
                    self.on_txpool(t).await?
                }
                Some(_) = looks_update_stream.next() => {
                    self.on_looks_orderbook_update().await?
                }
            }
        }
    }

    pub async fn on_txpool(&self, mem_tx: TxpoolTransaction) -> eyre::Result<()> {
        debug!("--Recieved mempool tx--");
        let nonce = self
            .client
            .get_transaction_count(self.address, None)
            .await?;

        if mem_tx.to.unwrap_or_default() == self.router_addresses.sudo_router_address
            || mem_tx.to.unwrap_or_default() == self.router_addresses.nftx_zap_address
            || self
                .router_addresses
                .aggregator_addresses
                .contains(&mem_tx.to.unwrap_or_default())
            || self
                .market_searcher
                .marketplace_addresses
                .contains(&mem_tx.to.unwrap_or_default())
        {
            let tx = TxpoolTx(mem_tx).to_transaction();
            info!("-----Tracing Mempool Tx-----");
            let trace_type = TraceType::StateDiff;
            let trace = self.client.trace_call(&tx, vec![trace_type], None).await;
            if let Err(e) = trace {
                warn!("Trace failed: {:?}, {:?}", e, tx);
                return Ok(());
            }
            let diff = trace.unwrap().state_diff.unwrap();
            let touched_addresses: HashSet<Address> = diff.0.iter().map(|(k, _)| *k).collect();
            let mut state = spoof::state();
            apply_state_diff(&mut state, diff);
            let nft_markets_new = Arc::new(Mutex::new(self.nft_markets.clone()));
            let n = self
                .update_pricing(touched_addresses, &nft_markets_new, false, Some(&state))
                .await?;
            if n > 0 {
                //Only try and take if mkts were updated (don't backrun stale arbs)
                let nft_markets_post_diff = Arc::try_unwrap(nft_markets_new).unwrap().into_inner();
                let mut arbs = self
                    .find_arbs(&nft_markets_post_diff, self.last_updated_block)
                    .await?;
                arbs.sort_unstable();
                for arb in arbs.into_iter().rev().take(5) {
                    self.take_arb(arb, nonce, Some(tx.clone()), Some(&state))
                        .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn on_looks_orderbook_update(&mut self) -> eyre::Result<()> {
        // This is shit and never worked
        debug!("-----Updating Looks Quotes-----");
        let quote = self
            .market_searcher
            .looks_orderbook
            .get_new_orders()
            .await?;
        if let Some(q) = quote {
            if q.len() == 0 {
                return Ok(());
            }
            let q = &q[0];
            let marketplace = Marketplace {
                market_type: MarketType::Looks(Box::new(q.clone())),
                nft_address: q.collection_address,
                nft_contract: ERC721::new(q.collection_address, self.client.clone()),
            };
            if !self
                .market_searcher
                .marketplace_addresses
                .contains(&marketplace.contract_address())
                && !self.banned_nfts.contains(&marketplace.nft_address)
            //ENS currently
            {
                let contract_address = marketplace.contract_address();
                self.market_searcher.marketplaces.push(marketplace);
                self.market_searcher
                    .marketplace_addresses
                    .insert(contract_address);
            }
            let nft_markets_new = Arc::new(Mutex::new(self.nft_markets.clone()));
            self.update_pricing(
                HashSet::from([q.collection_address]),
                &nft_markets_new,
                false,
                None,
            )
            .await?;
            self.nft_markets = Arc::try_unwrap(nft_markets_new).unwrap().into_inner();
            //?;
        }
        Ok(())
    }

    pub async fn on_block(&mut self) -> eyre::Result<()> {
        info!("-----Recieved Block-----");
        let now = Instant::now();
        let current_block = self.client.get_block_number().await?;
        info!(
            "Last updated block {}, Current block {}",
            self.last_updated_block, current_block
        );

        //Collect general data to update state with
        info!("Getting new events");
        let new_events = self
            .get_new_events(self.last_updated_block, current_block)
            .await?;

        info!("Getting new logs");
        let logs = match self.full_update {
            true => Vec::new(),
            false => {
                self.get_new_logs(self.last_updated_block, current_block)
                    .await?
            }
        };
        info!("Got new events and logs in {:?}", now.elapsed());

        //Do the update
        self.update_state(new_events, logs).await?;

        //Find & take arbs
        info!("Finding arb opportunities");
        let now = Instant::now();
        let mut arbs = self.find_arbs(&self.nft_markets, current_block).await?;
        arbs.sort_unstable();
        let nonce = self
            .client
            .get_transaction_count(self.address, None)
            .await?;
        info!("Found arbs in in {:?}", now.elapsed());
        for arb in arbs.into_iter().rev() {
            self.take_arb(arb, nonce, None, None).await?;
        }

        //We are now up-to-date
        self.last_updated_block = current_block;
        self.full_update = false;
        Ok(())
    }

    pub async fn update_state(&mut self, events: Vec<Event>, logs: Vec<Log>) -> eyre::Result<()> {
        info!("Processing events");
        let now = Instant::now();
        self.handle_events(events).await?;
        info!("Processed events in {:?}", now.elapsed());
        info!(
            "Number of marketplaces {} (check: {}), Number of NFTs {}, (check {})",
            self.market_searcher.marketplaces.len(),
            self.market_searcher.marketplace_addresses.len(),
            self.nft_markets.len(),
            self.market_searcher.looks_orderbook.nft_vec.len(),
        );

        info!("Updating market pricing");
        let now = Instant::now();

        let nft_markets_new = Arc::new(Mutex::new(self.nft_markets.clone())); //hmm
        let delta_addresses = logs
            .iter()
            .map(|log| log.address)
            .collect::<HashSet<Address>>();
        self.update_pricing(delta_addresses, &nft_markets_new, self.full_update, None)
            .await?; //not obvious but this mutates the mutex!!!!
        self.nft_markets = Arc::try_unwrap(nft_markets_new).unwrap().into_inner(); //?;
        info!("Updated pricing in {:?}", now.elapsed());
        Ok(())
    }

    pub async fn get_new_events(
        &self,
        start_block: U64,
        end_block: U64,
    ) -> eyre::Result<Vec<Event>> {
        match self.opts.n_events {
            0 => {
                self.market_searcher
                    .get_new_market_events(start_block, end_block)
                    .await
            }
            _ => {
                self.market_searcher
                    .get_new_market_events_limited(start_block, end_block, self.opts.n_events)
                    .await
            }
        }
    }
    pub async fn get_new_logs(&self, start_block: U64, end_block: U64) -> eyre::Result<Vec<Log>> {
        match self.opts.n_events {
            0 => {
                self.market_searcher
                    .get_new_logs(start_block, end_block)
                    .await
            }
            _ => {
                self.market_searcher
                    .get_new_logs_limited(start_block, end_block)
                    .await
            }
        }
    }

    pub async fn handle_events(&mut self, events: Vec<Event>) -> eyre::Result<()> {
        //Create and perform calls, mutating marketplaces_new
        let mut calls = Vec::new();
        let marketplaces_new = Arc::new(Mutex::new(Vec::new()));
        for event in events {
            let marketplaces_new = marketplaces_new.clone();
            calls.push(self.handle_event(event, marketplaces_new))
        }
        complete_buffered(calls, self.opts.n_buffered_calls).await?;

        //Process, only add new marketplace if new (Todo -> is this check even required??)
        let marketplaces_new = Arc::try_unwrap(marketplaces_new).unwrap().into_inner(); //?;
        for marketplace in marketplaces_new {
            if !self
                .market_searcher
                .marketplace_addresses
                .contains(&marketplace.contract_address())
                && !self.banned_nfts.contains(&marketplace.nft_address)
            {
                let contract_address = marketplace.contract_address();
                let nft_address = marketplace.nft_address;
                self.market_searcher.marketplaces.push(marketplace);
                self.market_searcher
                    .marketplace_addresses
                    .insert(contract_address);
                if !self
                    .market_searcher
                    .looks_orderbook
                    .nft_vec
                    .contains(&nft_address)
                {
                    self.market_searcher
                        .looks_orderbook
                        .nft_vec
                        .push(nft_address);
                }
            }
        }
        Ok(())
    }

    pub async fn handle_event(
        &self,
        event: Event,
        marketplaces_new: Arc<Mutex<Vec<Marketplace<M>>>>,
    ) -> eyre::Result<()> {
        //Todo -> add as impl fn for each Event struct
        match event {
            Event::SudoNewPair(event) => {
                let pair = SudoPairETH::new(event.pool_address, self.client.clone());
                let pool_address = event.pool_address;
                let pair_variant = pair.pair_variant().call().await?;
                if [0, 1].contains(&pair_variant) {
                    let nft_address = pair.nft().call().await?;
                    let pool_type = pair.pool_type().call().await?;
                    let nft_contract = ERC721::new(nft_address, self.client.clone());
                    let contract = SudoContract::ETH(pair);

                    let pool_type = match pool_type {
                        0 => PoolType::Token,
                        1 => PoolType::NFT,
                        2 => PoolType::Trade,
                        _ => panic!(), //Should never be anything other than 0,1,2 as is ETH enum
                    };
                    // let approved = self.approved_sudo.contains(&(nft_address, pool_address));

                    let pool = SudoPool {
                        pool_address,
                        pool_type,
                        pair_variant,
                        contract, //try and get rid of this grim enum shit at some point
                        nft_contract: nft_contract.clone(), //go and properly lifetime this shit at some point
                    };

                    let mut marketplaces = marketplaces_new.lock();
                    {
                        marketplaces.push(Marketplace {
                            market_type: MarketType::Sudo(pool),
                            nft_contract,
                            nft_address,
                        });
                    }
                }
            }
            Event::NFTxNewVault(event) => {
                let nft_address = event.nft_address;
                let vault_address = event.vault_address;
                let contract = NFTxVaultContract::new(vault_address, self.client.clone());
                let nft_contract = ERC721::new(nft_address, self.client.clone());

                let vault = NFTxVault {
                    vault_address,
                    contract,
                };
                let mut marketplaces = marketplaces_new.lock();
                {
                    marketplaces.push(Marketplace {
                        market_type: MarketType::NFTx(vault),
                        nft_contract,
                        nft_address,
                    });
                }
            }
        }
        Ok(())
    }
    pub async fn update_pricing(
        &self,
        delta_addresses: HashSet<Address>,
        nft_markets_new: &Arc<Mutex<HashMap<Address, NFTMarket>>>,
        full_update: bool,
        diff: Option<&State>,
    ) -> eyre::Result<usize> {
        //find quotes from marketplaces, updating nft quotes if better than previous
        let mut calls = Vec::new();

        for (i, marketplace) in self.market_searcher.marketplaces.iter().enumerate() {
            let address = marketplace.contract_address();
            if delta_addresses.contains(&address) || full_update {
                let nft_markets_new = nft_markets_new.clone();
                let marketplace_inds = &nft_markets_new
                    .lock()
                    .get(&marketplace.nft_address)
                    .cloned()
                    .unwrap_or_default()
                    .all_marketplace_inds;
                {
                    nft_markets_new
                        .lock()
                        .insert(marketplace.nft_address, NFTMarket::default());
                } //test whether we can delete this
                calls.push(self.update_market_price(i, nft_markets_new.clone(), marketplace, diff));
                //update other marketplaces -> should haev just used a graph this is fucking horrible
                for j in marketplace_inds {
                    calls.push(self.update_market_price(
                        *j,
                        nft_markets_new.clone(),
                        &self.market_searcher.marketplaces[*j],
                        diff,
                    ));
                }
            }
        }
        let n_calls = calls.len();
        info!("Updating {} market prices", n_calls);
        complete_buffered(calls, self.opts.n_buffered_calls).await?;

        Ok(n_calls)
    }

    pub async fn update_market_price(
        //TODO make this a impl of NFTmarket (market.update_price(markets_new,i))
        &self,
        marketplace_ind: usize,
        nft_markets_new: Arc<Mutex<HashMap<Address, NFTMarket>>>,
        marketplace: &Marketplace<M>,
        diff: Option<&State>,
    ) -> eyre::Result<()> {
        //Get prices from chain (I cba reimplementing all the Sudo curve logics in my code so here we are)
        let (buy_price, sell_price) = self
            .market_searcher
            .get_prices(marketplace, 1_usize, diff)
            .await?;

        let mut nft_markets = nft_markets_new.lock();
        let nft_market = nft_markets.entry(marketplace.nft_address).or_default();
        nft_market.all_marketplace_inds.insert(marketplace_ind); //HashSet so won't duplicate

        //Update buy quote if better
        if let Some(new_price) = buy_price {
            if let Some(current_price) = nft_market.best_buy_quote {
                if new_price < current_price {
                    nft_market.best_buy_quote = Some(new_price);
                    nft_market.buy_marketplace_ind = Some(marketplace_ind);
                }
            } else {
                nft_market.best_buy_quote = Some(new_price);
                nft_market.buy_marketplace_ind = Some(marketplace_ind);
            }
        }

        //Update sell quote if better
        if let Some(new_price) = sell_price {
            if let Some(current_price) = nft_market.best_sell_quote {
                if new_price > current_price {
                    nft_market.best_sell_quote = Some(new_price);
                    nft_market.sell_marketplace_ind = Some(marketplace_ind);
                }
            } else {
                nft_market.best_sell_quote = Some(new_price);
                nft_market.sell_marketplace_ind = Some(marketplace_ind);
            }
        }
        Ok(())
    } //todo -> do on marketplace

    pub async fn find_arbs(
        &self,
        nft_markets: &HashMap<Address, NFTMarket>,
        block_number: U64,
    ) -> eyre::Result<Vec<ArbInfo>> {
        let mut arbs = Vec::new();
        for (nft_address, market) in nft_markets {
            if let Some(sell_quote) = market.best_sell_quote {
                if let Some(buy_quote) = market.best_buy_quote {
                    if sell_quote > buy_quote {
                        let delta = sell_quote - buy_quote;

                        let buy_market = market.buy_marketplace_ind.unwrap();
                        let sell_market = market.sell_marketplace_ind.unwrap();
                        let arb = ArbInfo {
                            delta,
                            block_number,
                            inds: (buy_market, sell_market),
                            quotes: (buy_quote, sell_quote),
                            nft_address: *nft_address,
                            _breakeven_gas_price: None,
                        };
                        debug!("Found arbitrage -> {}", &arb);
                        arbs.push(arb);
                    }
                }
            }
        }
        Ok(arbs)
    }

    pub async fn find_optimal_input(
        &self,
        buy_marketplace: &Marketplace<M>,
        sell_marketplace: &Marketplace<M>,
        state: Option<&State>,
        basefee_next_max: U256,
    ) -> eyre::Result<(Vec<Call>, U256, U256)> {
        let mut gas = U256::zero();
        let mut profit = U256::zero();
        let mut calls = Vec::new();

        //TODO -> skip i=1 case fetching prices cause we already have info in arb data
        for i in 1..256 {
            if i > 1 {
                send_tg(format!("Calculating profit for {} nfts", i).as_str()).await?;
                info!("Calculating profit for {} nfts", i);
            }
            //Find new prices and gas costs for arbing i NFTs
            let (buy_price, _) = self
                .market_searcher
                .get_prices(buy_marketplace, i, state)
                .await?;
            let (_, sell_price) = self
                .market_searcher
                .get_prices(sell_marketplace, i, state)
                .await?;
            if let Some(buy_price) = buy_price {
                if let Some(sell_price) = sell_price {
                    let r = self
                        .create_tx_and_find_gas(
                            buy_marketplace,
                            sell_marketplace,
                            buy_price,
                            sell_price,
                            state,
                            i,
                        )
                        .await;
                    if let Err(e) = r {
                        send_tg(format!("Gas estimation error: {:?}", e).as_str()).await?;
                        info!("Gas estimation error: {:?}", e);
                        break;
                    }
                    let (new_calls, new_gas) = r.unwrap();
                    //Check if this is more profitable than last iteration, if so write to output variables and continue to next iteration
                    if sell_price > buy_price {
                        let new_delta = sell_price - buy_price;
                        let burnt = new_gas * basefee_next_max;
                        if new_delta > burnt {
                            let new_profit = new_delta - burnt;
                            if new_profit > profit {
                                profit = new_profit;
                                gas = new_gas;
                                calls = new_calls;
                                continue;
                            }
                        }
                    }
                }
            }
            break;
        }
        Ok((calls, profit, gas))
    }

    pub async fn create_tx_and_find_gas(
        &self,
        buy_marketplace: &Marketplace<M>,
        sell_marketplace: &Marketplace<M>,
        input: U256,
        output: U256,
        state: Option<&State>,
        num_nft: usize,
    ) -> eyre::Result<(Vec<Call>, U256)> {
        let (buy_calls, ids) = buy_marketplace
            .create_buy_txs(
                self.executor.address,
                input,
                num_nft,
                &self.market_searcher.nftx_router,
                &self.market_searcher.looks_exchange,
                self.market_searcher.weth_address,
            )
            .await?;
        if ids.is_none() {
            return Err(eyre!("error getting ids"));
        }
        let sell_calls = sell_marketplace
            .create_sell_txs(
                self.executor.address,
                output,
                ids.unwrap(),
                &self.market_searcher.nftx_router,
                &self.market_searcher.looks_exchange,
                self.market_searcher.weth_address,
            )
            .await?;
        if let Some(buy_calls) = buy_calls {
            if let Some(sell_calls) = sell_calls {
                let calls = vec![buy_calls, sell_calls];
                let calls: Vec<Call> = calls.into_iter().flatten().collect();
                let gas = self
                    .executor
                    .estimate_gas(calls.clone(), U256::zero(), self.address, state)
                    .await?;
                return Ok((calls, gas));
            }
        }
        Err(eyre!("Failed to create buy/sell calls"))
    }

    pub async fn take_arb(
        &self,
        arb: ArbInfo,
        nonce: U256,
        prepend_tx: Option<Transaction>,
        state: Option<&State>,
    ) -> eyre::Result<()> {
        let balance = self.client.get_balance(self.executor.address, None).await?;
        let basefee_next_max = self
            .client
            .get_block(arb.block_number)
            .await?
            .unwrap()
            .base_fee_per_gas
            .unwrap()
            * U256::from(9 * ONE)
            / U256::from(8 * ONE);

        if balance > arb.quotes.0 && arb.delta > self.opts.gas_heuristic * basefee_next_max {
            let buy_marketplace = &self.market_searcher.marketplaces[arb.inds.0];
            let sell_marketplace = &self.market_searcher.marketplaces[arb.inds.1];
            let backrun = prepend_tx.is_some();
            if let MarketType::NFTx(_) = buy_marketplace.market_type {
                if let MarketType::NFTx(_) = sell_marketplace.market_type {
                    //we cant do nftx-nftx arbs rn cause of sushi slippage not being priced in (NFT markets & slippage is hard problem and gonna need a lot of thought for V2 graph based version of this)
                    return Ok(());
                }
            }
            //contested vs uncontested fees -> dynamic later??
            let mut fee_percent = self.opts.percent_to_miner.uncontested;
            if let MarketType::Sudo(_) = buy_marketplace.market_type {
                if let MarketType::Sudo(_) = sell_marketplace.market_type {
                    fee_percent = self.opts.percent_to_miner.contested;
                }
            }
            info!(
                "Attempting to take arbitrage {}-{} -> {}",
                buy_marketplace.market_type, sell_marketplace.market_type, arb
            );

            let optimal_input = self
                .find_optimal_input(
                    buy_marketplace,
                    sell_marketplace,
                    state,
                    basefee_next_max,
                )
                .await; //TODO ERROR HANDLE
            if let Err(ref e) = optimal_input {
                warn!("Optimal input calc error: {}", e);
                send_tg(format!("Optimal input calc error: {}", e).as_str()).await?;
                return Ok(());
            }
            let (calls, profit, gas) = optimal_input.unwrap();

            if profit > self.opts.minimum_profit {
                let eth_to_coinbase =
                    (profit * fee_percent * U256::from(ONE)) / U256::from(100 * ONE);
                //To delete when safe
                // let sim_info = self
                //     .executor
                //     .simulate_arb(
                //         calls.clone(),
                //         gas + U256::from(50_000_u64),
                //         arb.block_number,
                //         basefee_next_max,
                //         eth_to_coinbase,
                //         nonce,
                //         prepend_tx.clone(),
                //     )
                //     .await;
                // if let Err(e) = sim_info {
                //     warn!("{:?}", e);
                //     send_tg(format!("Sim error: {:?}", e).as_str()).await?;
                //     return Ok(());
                // }
                let live_bundle = self
                    .executor
                    .create_live_bundle(
                        calls,
                        gas + U256::from(50_000_u64),
                        basefee_next_max,
                        arb.block_number,
                        eth_to_coinbase,
                        nonce,
                        prepend_tx,
                    )
                    .await?;
                let r = self.executor.send_fb_bundle(live_bundle).await;
                if let Err(e) = r {
                    warn!("Bundle error {}", e);
                    send_tg(format!("Bundle error {}", e).as_str()).await?;
                    return Ok(());
                }
                let (bundle_hash, block) = r.unwrap();
                info!(
                    "Sent bundle {:?} block: {} -> {}, backrun: {}, gas price: {}GWei, expected profit: {}, coinbase tip: {}",
                    bundle_hash,
                    block,
                    arb,
                    backrun,
                    to_eth(basefee_next_max) * 1e9,
                    to_eth(profit),//to_eth(profit),
                    to_eth(eth_to_coinbase)//to_eth(coinbase_diff_arb_tx),
                );
                send_tg(format!(
                    "Sent bundle {:?} block: {} {}-{} -> {}, backrun: {}, gas price: {}GWei, expected profit: {}, coinbase tip: {}",
                    bundle_hash,
                    block,
                    buy_marketplace.market_type,
                    sell_marketplace.market_type,
                    arb,
                    backrun,
                    to_eth(basefee_next_max) * 1e9,
                    to_eth(profit),//to_eth(profit),
                    to_eth(eth_to_coinbase)//to_eth(coinbase_diff_arb_tx),
                )
                .as_str()).await?;

                // }
            }
        }
        Ok(())
    }
}

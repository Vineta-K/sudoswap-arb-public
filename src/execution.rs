use ethers::prelude::*;
use ethers::providers::call_raw::spoof::State;
use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers_flashbots::*;
use tracing::warn;

use eyre::eyre;

use crate::abigen::Multi;
use crate::multi::{Call, Multicall, MulticallHeader};
use crate::utils::{no_errors, send_tg};

pub struct Executor<M: Middleware, N: Middleware, S: Signer> {
    fb_clients: Vec<SignerMiddleware<FlashbotsMiddleware<N, S>, S>>,
    executor: Multi<M>,
    local_sim: bool,
    pub pending: Vec<H256>,
    pub address: Address,
}

impl<M, N, S> Executor<M, N, S>
where
    M: Middleware + 'static,
    N: Middleware + 'static,
    S: Signer + 'static,
{
    pub async fn new(
        fb_clients: Vec<SignerMiddleware<FlashbotsMiddleware<N, S>, S>>,
        executor: Multi<M>,
        local_sim: bool,
    ) -> eyre::Result<Executor<M, N, S>> {
        let pending = Vec::new();
        let address = executor.address();
        Ok(Self {
            fb_clients,
            executor,
            pending,
            address,
            local_sim,
        })
    }

    pub fn create_multicall_header(
        &self,
        eth_to_coinbase: U256,
        block_number: U64,
    ) -> MulticallHeader {
        MulticallHeader::new(
            false,
            false,
            eth_to_coinbase.as_u128(),
            block_number.as_u64(),
        )
    }

    pub async fn estimate_gas(
        &self,
        bundle_calls: Vec<Call>,
        eth_to_coinbase: U256,
        address: Address,
        diff: Option<&State>,
    ) -> eyre::Result<U256> {
        let header = self.create_multicall_header(eth_to_coinbase, U64::from(0));
        let multicall = Multicall::new(header, bundle_calls);
        let multi_params = multicall.encode_parameters();

        let gas = match diff {
            Some(s) => {
                let tx = self.executor.ostium(multi_params).from(address).tx;
                self.fb_clients[0]
                    .estimate_gas_override(&tx, None, s)
                    .await?
            }
            None => {
                self.executor
                    .ostium(multi_params)
                    .from(address)
                    .estimate_gas()
                    .await?
            }
        };
        Ok(gas)
    }

    pub async fn simulate_arb(
        &self,
        bundle_calls: Vec<Call>,
        gas: U256,
        block_number: U64,
        basefee_next_max: U256,
        eth_to_coinbase: U256,
        nonce: U256,
        prepend_tx: Option<Transaction>,
    ) -> eyre::Result<SimulatedBundle> {
        let header = self.create_multicall_header(eth_to_coinbase, U64::zero());
        let multicall = Multicall::new(header, bundle_calls);
        let multi_params = multicall.encode_parameters();
        let fb_client = &self.fb_clients[0]; //flashbots relay with sim

        let tx: Eip1559TransactionRequest = self.executor.ostium(multi_params).gas(gas).tx.into();
        let tx = tx
            .nonce(nonce)
            .max_fee_per_gas(basefee_next_max)
            .max_priority_fee_per_gas(U256::zero());
        let mut tx: TypedTransaction = tx.into();

        fb_client.fill_transaction(&mut tx, None).await?;
        let signature = fb_client.signer().sign_transaction(&tx).await?;

        let mut bundle = BundleRequest::new();
        if let Some(prepend_tx) = prepend_tx {
            bundle = bundle.push_transaction(prepend_tx);
        }
        bundle = bundle.push_transaction(tx.rlp_signed(&signature));

        bundle = bundle
            .set_block(block_number + 1)
            .set_simulation_block(block_number)
            .set_simulation_timestamp(0);

        let simulated_bundle = match self.local_sim {
            true => fb_client
                .inner()
                .provider()
                .request("eth_callBundle", [&bundle])
                .await
                .map_err(|e| format!("Local: {:?}", e)),
            false => fb_client
                .inner()
                .simulate_bundle(&bundle)
                .await
                .map_err(|e| format!("Relay: {:?}", e)),
        };
        match simulated_bundle {
            //Check bundle ok
            Ok(bundle) => match no_errors(&bundle) {
                //Check txs ok
                true => Ok(bundle),
                false => Err(eyre!("Errors in : {:?}", bundle)),
            },
            Err(e) => Err(eyre!("Bundle simulation error: {:?}", e)),
        }
    }

    pub async fn create_live_bundle(
        &self,
        bundle_calls: Vec<Call>,
        gas: U256,
        gas_price: U256,
        block_number: U64,
        eth_to_coinbase: U256,
        nonce: U256,
        prepend_tx: Option<Transaction>,
    ) -> eyre::Result<BundleRequest> {
        let header = self.create_multicall_header(eth_to_coinbase, U64::zero());
        let multicall = Multicall::new(header, bundle_calls);
        let multi_params = multicall.encode_parameters();
        let fb_client = &self.fb_clients[0];

        let tx: Eip1559TransactionRequest = self.executor.ostium(multi_params).gas(gas).tx.into();
        let tx = tx
            .nonce(nonce)
            .max_fee_per_gas(gas_price)
            .max_priority_fee_per_gas(U256::zero());
        let mut tx: TypedTransaction = tx.into();

        fb_client.fill_transaction(&mut tx, None).await?;
        let signature = fb_client.signer().sign_transaction(&tx).await?;

        let mut live_bundle = BundleRequest::new();
        if let Some(prepend_tx) = prepend_tx {
            live_bundle = live_bundle.push_transaction(prepend_tx);
        }
        live_bundle = live_bundle.push_transaction(tx.rlp_signed(&signature));

        live_bundle = live_bundle.set_block(block_number + 1);
        Ok(live_bundle)
    }

    pub async fn send_fb_bundle(&self, bundle: BundleRequest) -> eyre::Result<(H256, U64)> {
        let pending_bundle = self.fb_clients[0].inner().send_bundle(&bundle).await?;
        for (i, fb_client) in self.fb_clients[1..].iter().enumerate() {
            let resp = fb_client.inner().send_bundle(&bundle).await;
            if let Err(err) = resp {
                if let FlashbotsMiddlewareError::RelayError(RelayError::ResponseSerdeJson {
                    err: _,
                    text: _,
                }) = err
                {
                    continue;
                }
                warn!("Error in relay {}: {}", i + 1, err);
                send_tg(format!("Error in relay {}: {}", i + 1, err).as_str()).await?;
            }
        }
        Ok((pending_bundle.bundle_hash, pending_bundle.block))
    }

    pub async fn create_backrun_tx(
        &self,
        bundle_calls: Vec<Call>,
        gas: U256,
        nonce: U256,
        prepend_tx: Transaction,
        basefee_next_max: U256,
    ) -> eyre::Result<(Bytes, U256)> {
        let header = self.create_multicall_header(U256::zero(), U64::zero());
        let multicall = Multicall::new(header, bundle_calls);
        let multi_params = multicall.encode_parameters();
        let fb_client = &self.fb_clients[0];

        match prepend_tx.transaction_type {
            Some(i) => match i.as_u64() {
                1 => Err(eyre!("Access list not supported")),
                2 => {
                    //Eip1559
                    if prepend_tx.max_fee_per_gas.is_none()
                        || prepend_tx.max_priority_fee_per_gas.is_none()
                    {
                        return Err(eyre!("Eip1559 missing gas fees"));
                    }
                    let tx: Eip1559TransactionRequest =
                        self.executor.ostium(multi_params).gas(gas).tx.into();
                    let tx = tx
                        .nonce(nonce)
                        .max_fee_per_gas(prepend_tx.max_fee_per_gas.unwrap())
                        .max_priority_fee_per_gas(prepend_tx.max_priority_fee_per_gas.unwrap());
                    let mut tx: TypedTransaction = tx.into();
                    fb_client.fill_transaction(&mut tx, None).await?;
                    let signature = fb_client.signer().sign_transaction(&tx).await?;
                    Ok((
                        tx.rlp_signed(&signature),
                        prepend_tx.max_priority_fee_per_gas.unwrap(),
                    ))
                }
                _ => Err(eyre!("We fucked up if we are here")),
            },
            None => {
                //Legacy
                if prepend_tx.gas_price.is_none() {
                    return Err(eyre!("Legacy missing gas fees"));
                }
                if basefee_next_max > prepend_tx.gas_price.unwrap() {
                    return Err(eyre!("Legacy priced too cheap"));
                }

                let tx: TransactionRequest = self.executor.ostium(multi_params).gas(gas).tx.into();
                let tx = tx
                    .nonce(nonce)
                    .gas_price(prepend_tx.gas_price.unwrap() - U256::from(1));
                let mut tx: TypedTransaction = tx.into();
                fb_client.fill_transaction(&mut tx, None).await?;
                let signature = fb_client.signer().sign_transaction(&tx).await?;
                Ok((
                    tx.rlp_signed(&signature),
                    prepend_tx.gas_price.unwrap() - basefee_next_max,
                ))
            }
        }
    }
}

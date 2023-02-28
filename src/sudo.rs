use std::sync::Arc;

use ethers::prelude::*;
use ethers::providers::call_raw::spoof::State;
use ethers::providers::call_raw::RawCall;

use crate::abigen::{SudoPairERC20, SudoPairETH, ERC20, ERC721};
// use crate::database::add_approved_nft;
use crate::multi::{Call, Type};
use crate::sudo::PoolType::{Token, Trade, NFT};

#[derive(Debug, Clone)]
pub enum SudoContract<M: Middleware> {
    ETH(SudoPairETH<M>),
    ERC20(SudoPairERC20<M>),
}

#[derive(Copy, Clone, Debug)]
pub enum PoolType {
    Token,
    NFT,
    Trade,
}

#[derive(Debug, Clone)]
pub struct SudoPool<M: Middleware> {
    pub pool_address: Address,
    pub pool_type: PoolType,
    pub pair_variant: u8,
    pub contract: SudoContract<M>,
    pub nft_contract: ERC721<M>,
}

impl<M> SudoPool<M>
where
    M: Middleware,
{
    pub async fn get_buy_price(
        &self,
        num_nft: usize,
        diff: Option<&State>,
    ) -> eyre::Result<Option<U256>>
    where
        M: Middleware + 'static,
    {
        match self.pool_type {
            Trade | NFT => match &self.contract {
                SudoContract::ETH(c) => {
                    let (_, _, _, price, _) = match diff {
                        //apply state diff if present
                        Some(d) => {
                            c.get_buy_nft_quote(U256::from(num_nft))
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => c.get_buy_nft_quote(U256::from(num_nft)).call().await?,
                    };
                    let contract_liq = match diff {
                        Some(d) => {
                            self.nft_contract
                                .balance_of(self.pool_address)
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => {
                            self.nft_contract
                                .balance_of(self.pool_address)
                                .call()
                                .await?
                        }
                    };
                    if U256::from(num_nft) > contract_liq {
                        Ok(None)
                    } else {
                        Ok(Some(price))
                    }
                }
                SudoContract::ERC20(c) => {
                    let (_, _, _, price, _) = match diff {
                        Some(d) => {
                            c.get_buy_nft_quote(U256::from(num_nft))
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => c.get_buy_nft_quote(U256::from(num_nft)).call().await?,
                    };
                    let contract_liq = match diff {
                        Some(d) => self
                            .nft_contract
                            .balance_of(self.pool_address)
                            .call_raw()
                            .state(d)
                            .await?
                            .as_usize(),
                        None => self
                            .nft_contract
                            .balance_of(self.pool_address)
                            .call()
                            .await?
                            .as_usize(),
                    };
                    if num_nft > contract_liq {
                        Ok(None)
                    } else {
                        Ok(Some(price))
                    }
                }
            },
            Token => Ok(None),
        }
    }
    pub async fn get_sell_price(
        &self,
        client: Arc<M>,
        num_nft: usize,
        diff: Option<&State>,
    ) -> eyre::Result<Option<U256>>
    where
        M: Middleware + 'static,
    {
        match self.pool_type {
            Trade | Token => match &self.contract {
                SudoContract::ETH(c) => {
                    let (_, _, _, price, _) = match diff {
                        Some(d) => {
                            c.get_sell_nft_quote(U256::from(num_nft))
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => c.get_sell_nft_quote(U256::from(num_nft)).call().await?,
                    };
                    let contract_liq = match diff {
                        Some(d) => {
                            if let Some(bal) = d
                                .hashmap()
                                .get(&self.pool_address)
                                .cloned()
                                .unwrap_or_default()
                                .balance
                            {
                                bal
                            } else {
                                client.get_balance(self.pool_address, None).await?
                            }
                        }
                        None => client.get_balance(self.pool_address, None).await?,
                    };
                    if price > contract_liq {
                        Ok(None)
                    } else {
                        Ok(Some(price))
                    }
                }
                SudoContract::ERC20(c) => {
                    let (_, _, _, price, _) = match diff {
                        Some(d) => {
                            c.get_sell_nft_quote(U256::from(num_nft))
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => c.get_sell_nft_quote(U256::from(num_nft)).call().await?,
                    };
                    let erc20_contract = ERC20::new(c.token().call().await?, client.clone()); //maybe manage this better later in the pool struct
                    let contract_liq = match diff {
                        Some(d) => {
                            erc20_contract
                                .balance_of(self.pool_address)
                                .call_raw()
                                .state(d)
                                .await?
                        }
                        None => erc20_contract.balance_of(self.pool_address).call().await?,
                    };
                    if price > contract_liq {
                        Ok(None)
                    } else {
                        Ok(Some(price))
                    }
                }
            },
            NFT => Ok(None),
        }
    }
    pub async fn create_buy_txs(
        &self,
        input: U256,
        address: Address,
        num_nft: usize,
    ) -> eyre::Result<(Vec<Call>, Option<Vec<U256>>)> {
        let mut calls = Vec::new();
        match &self.contract {
            SudoContract::ETH(contract) => {
                let ids = contract.get_all_held_ids().call().await;
                if ids.is_err() {
                    return Ok((calls, None));
                };
                let mut ids = ids.unwrap();
                if ids.len() < num_nft {
                    return Ok((calls, None));
                }
                ids.truncate(num_nft);
                let payload = contract
                    .swap_token_for_specific_nf_ts(
                        ids.clone(),
                        input,
                        address,
                        false,
                        Address::zero(),
                    )
                    .calldata()
                    .unwrap();
                calls.push(Call::new(
                    contract.address(),
                    payload[0..4].to_vec(),
                    Type::ValueCall,
                    Option::from(input),
                    payload[4..].to_vec(),
                ));
                Ok((calls, Some(ids)))
            }
            SudoContract::ERC20(_) => Ok((calls, None)),
        }
    }
    pub async fn create_sell_txs(
        &self,
        output: U256,
        address: Address,
        ids: Vec<U256>,
        nft_contract: &ERC721<M>,
    ) -> eyre::Result<Vec<Call>> {
        let mut calls = Vec::new();
        match &self.contract {
            SudoContract::ETH(contract) => {
                let approved = nft_contract
                    .is_approved_for_all(address, contract.address())
                    .call()
                    .await
                    .unwrap_or(false);
                if !approved {
                    let payload = nft_contract
                        .set_approval_for_all(contract.address(), true)
                        .calldata()
                        .unwrap();
                    calls.push(Call::new(
                        nft_contract.address(),
                        payload[0..4].to_vec(),
                        Type::Call,
                        None,
                        payload[4..].to_vec(),
                    ));
                }
                let payload = contract
                    .swap_nf_ts_for_token(ids, output, address, false, Address::zero())
                    .calldata()
                    .unwrap();
                calls.push(Call::new(
                    contract.address(),
                    payload[0..4].to_vec(),
                    Type::Call,
                    None,
                    payload[4..].to_vec(),
                ));
                Ok(calls)
            }
            SudoContract::ERC20(_) => Ok(calls),
        }
    }
}

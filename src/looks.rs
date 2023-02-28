use std::str::FromStr;
use std:: sync::Arc;
use tracing::warn;

use ethers::{abi::ethereum_types::Signature, prelude::*};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::abigen::{LooksRareExchange, MakerOrder, TakerOrder, ERC721};
use crate::multi::{Call, Type};

#[derive(Debug, Deserialize, Clone)]
pub struct LooksQuote {
    pub hash: H256,
    #[serde(rename = "collectionAddress")]
    pub collection_address: Address,
    #[serde(rename = "tokenId")]
    pub token_id: Option<String>,
    #[serde(rename = "isOrderAsk")]
    pub is_order_ask: bool,
    pub signer: Address,
    pub strategy: Address,
    #[serde(rename = "currencyAddress")]
    pub currency_address: Address,
    pub amount: u64,
    pub price: String,
    pub nonce: String,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "minPercentageToAsk")]
    pub min_percentage_to_ask: u64,
    pub params: Option<String>,
    pub status: Option<String>,
    pub signature: Signature,
    pub v: u64,
    pub r: H256,
    pub s: H256,
}

impl LooksQuote {
    pub fn get_price(&self) -> Option<U256> {
        Some(U256::from_dec_str(self.price.as_str()).unwrap())
    }

    pub async fn create_buy_txs<M: Middleware>(
        &self,
        address: Address,
        exchange: &LooksRareExchange<M>,
    ) -> eyre::Result<(Vec<Call>, Option<Vec<U256>>)> {
        let mut calls = Vec::new();
        let price = U256::from_dec_str(self.price.as_str()).unwrap();
        let token_id = self
            .token_id
            .as_ref()
            .map(|o| U256::from_dec_str(o.as_str()).unwrap())
            .unwrap_or_default();
        let nonce = U256::from_dec_str(self.nonce.as_str()).unwrap();
        let start_time = U256::from(self.start_time);
        let end_time = U256::from(self.end_time);
        let min_percentage_to_ask = U256::from(self.min_percentage_to_ask);
        let params = Bytes::from_str(self.params.as_deref().unwrap()).unwrap();

        let maker_ask = MakerOrder {
            is_order_ask: self.is_order_ask,
            signer: self.signer,
            collection: self.collection_address,
            price,
            token_id,
            amount: U256::from(self.amount),
            strategy: self.strategy,
            currency: self.currency_address,
            nonce,
            start_time,
            end_time,
            min_percentage_to_ask,
            params: params.clone(),
            v: self.v.try_into().unwrap(),
            r: *self.r.as_fixed_bytes(),
            s: *self.s.as_fixed_bytes(),
        };
        let taker_bid = TakerOrder {
            is_order_ask: !self.is_order_ask,
            taker: address,
            price,
            token_id,
            min_percentage_to_ask,
            params,
        };
        let payload = exchange
            .match_ask_with_taker_bid_using_eth_and_weth(taker_bid, maker_ask)
            .calldata()
            .unwrap();
        calls.push(Call::new(
            exchange.address(),
            payload[0..4].to_vec(),
            Type::ValueCall,
            Option::from(price),
            payload[4..].to_vec(),
        ));

        // let maker_ask = quote;

        Ok((calls, Some(vec![token_id])))
    }
    pub async fn create_sell_txs<M: Middleware + 'static>(
        &self,
        address: Address,
        exchange: &LooksRareExchange<M>,
        nft_contract: &ERC721<M>,
        _weth_address: Address,
    ) -> eyre::Result<Vec<Call>> {
        let mut calls = Vec::new();
        let price = U256::from_dec_str(self.price.as_str()).unwrap();
        let token_id = self
            .token_id
            .as_deref()
            .map(|o| U256::from_dec_str(o).unwrap())
            .unwrap_or_default();
        let nonce = U256::from_dec_str(self.nonce.as_str()).unwrap();
        let start_time = U256::from(self.start_time);
        let end_time = U256::from(self.end_time);
        let min_percentage_to_ask = U256::from(self.min_percentage_to_ask);
        let params = Bytes::from_str(self.params.as_deref().unwrap()).unwrap();
        let maker_bid = MakerOrder {
            is_order_ask: self.is_order_ask,
            signer: self.signer,
            collection: self.collection_address,
            price,
            token_id,
            amount: U256::from(self.amount),
            strategy: self.strategy,
            currency: self.currency_address,
            nonce,
            start_time,
            end_time,
            min_percentage_to_ask,
            params: params.clone(),
            v: self.v.try_into().unwrap(),
            r: *self.r.as_fixed_bytes(),
            s: *self.s.as_fixed_bytes(),
        };
        let taker_ask = TakerOrder {
            is_order_ask: !self.is_order_ask,
            taker: address,
            price,
            token_id,
            min_percentage_to_ask,
            params,
        };
        let approved = nft_contract
            .is_approved_for_all(address, exchange.address())
            .call()
            .await?;
        if !approved {
            let payload = nft_contract
                .set_approval_for_all(exchange.address(), true)
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
        let payload = exchange
            .match_bid_with_taker_ask(taker_ask, maker_bid)
            .calldata()
            .unwrap();
        calls.push(Call::new(
            exchange.address(),
            payload[0..4].to_vec(),
            Type::Call,
            None,
            payload[4..].to_vec(),
        ));
        Ok(calls)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct LooksResp {
    success: bool,
    message: Option<String>,
    pub data: Vec<LooksQuote>,
}

pub struct LooksOrderbook {
    pub nft_vec_ind: usize,
    pub nft_vec: Vec<Address>,
    http_client: Arc<reqwest::Client>,
    endpoint_uri: String,
}

impl LooksOrderbook {
    pub fn new(
        http_client: Arc<reqwest::Client>,
        endpoint_uri: String,
        nft_vec: Vec<Address>,
    ) -> Self {
        Self {
            nft_vec_ind: 0,
            http_client,
            endpoint_uri,
            nft_vec,
        }
    }

    //TODO ABSTRACT OUT INTO LOOKS QUERY WHEN NOT CODING AT MIDNIGHT
    pub async fn get_new_orders(&mut self) -> eyre::Result<Option<Vec<LooksQuote>>> {
        let new_order = self.get_new_looks_quote().await?;
        if self.nft_vec_ind <= self.nft_vec.len()  {
            self.nft_vec_ind += 1;
        } else {
            self.nft_vec_ind = 0
        }
        Ok(new_order)
    }

    pub async fn get_new_looks_quote(&self) -> eyre::Result<Option<Vec<LooksQuote>>> {
        if self.nft_vec.is_empty(){
            return Ok(None)
        }
        let query = format!("?strategy=0x56244Bb70CbD3EA9Dc8007399F61dFC065190031&currency=0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2&collection={:?}&status[]=VALID&pagination[first]=1&sort=PRICE_ASC&isOrderAsk=true",self.nft_vec[self.nft_vec_ind]);
        let uri = self.endpoint_uri.to_owned() + "api/v1/orders" + query.as_str();
        let r = self
            .http_client
            .get(uri.as_str())
            .header("accept", "application/json");
        let response = r.send().await?;
        match response.status() {
            StatusCode::OK => {
                let response: LooksResp = response.json().await?;
                let data = Some(response.data);
                let _s = response.success; //keep clippy happy
                let _m = response.message; //keep clippy happy
                Ok(data)
            }
            s => {
                warn!(
                    "Failed to recieve LooksRare api quote -> Status code {:?}",
                    s
                );
                Ok(None)
            }
        }
    }
}

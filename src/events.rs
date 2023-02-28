use std::iter::Map;

use ethers::prelude::*;

use rusqlite::types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

use crate::abigen::{NewPairFilter,NewVaultFilter};

#[derive(Serialize, Deserialize)]
pub enum Event {
    SudoNewPair(SudoNewPairEvent),
    NFTxNewVault(NFTxNewVaultEvent),
}
impl FromSql for Event {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let event = serde_json::from_str(value.as_str()?).unwrap();
        Ok(event)
    }
}
impl ToSql for Event {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        let to_sql_output = serde_json::to_string(self).unwrap();
        Ok(to_sql_output.into())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NFTxNewVaultEvent {
    pub vault_address: Address,
    pub vault_id: U256,
    pub nft_address: Address
}

impl NFTxNewVaultEvent {
    pub fn from_new_vault_filter() -> impl Fn(NewVaultFilter) -> Event {
        |log| {
            Event::NFTxNewVault(NFTxNewVaultEvent {
                vault_address: log.vault_address,
                vault_id: log.vault_id,
                nft_address: log.asset_address,
            })
        }
    }
}


#[derive(Debug, Serialize, Deserialize)]
pub struct SudoNewPairEvent {
    pub pool_address: Address,
}

impl SudoNewPairEvent {
    pub fn from_new_pair_filter() -> impl Fn(NewPairFilter) -> Event {
        |log| {
            Event::SudoNewPair(SudoNewPairEvent {
                pool_address: log.pool_address,
            })
        }
    }
}

pub async fn get_events_as_iter<M, Filter, F>(
    filter: ethers::contract::builders::Event<'_, M, Filter>,
    closure: F,
    n: U64,
    n_minus: U64,
) -> eyre::Result<Map<std::vec::IntoIter<Filter>, impl FnMut(Filter) -> Event>>
where
    M: Middleware + 'static,
    Filter: EthLogDecode,
    F: Fn(Filter) -> Event,
{
    let map = filter
        .from_block(n_minus)
        .to_block(n)
        .query()
        .await?
        .into_iter()
        .map(closure);
    Ok(map)
}

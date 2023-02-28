use ethers::prelude::*;
use ethers::types::transaction::eip2930::AccessList;

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::FutureExt;
use tokio_stream::Stream;

pub struct TxpoolTx(pub(crate) TxpoolTransaction);

impl TxpoolTx {
    pub fn to_transaction(&self) -> Transaction {
        let TxpoolTx(t) = self;
        Transaction {
            hash: t.hash,
            nonce: t.nonce,
            block_hash: t.block_hash,
            block_number: t.block_number,
            transaction_index: t.transaction_index,
            from: t.from.unwrap_or_default(),
            to: t.to,
            value: t.value,
            gas_price: t.gas_price,
            gas: t.gas.unwrap_or_default(),
            input: t.input.clone(),
            v: t.v,
            r: t.r,
            s: t.s,
            transaction_type: t.transaction_type,
            access_list: Some(AccessList::default()),
            max_priority_fee_per_gas: t.max_priority_fee_per_gas,
            max_fee_per_gas: t.max_fee_per_gas,
            chain_id: t.chain_id,
            other: OtherFields::default(),
        }
    }
}

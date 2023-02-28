use ethers_flashbots::SimulatedBundle;
use tokio_stream;

use ethers::{
    prelude::*,
    providers::call_raw::spoof::State,
};
use futures::{Future, StreamExt};
use reqwest::get;
use tracing::warn;

pub fn to_eth(value: U256) -> f64 {
    let mut b32: [u8; 32] = Default::default();
    value.to_little_endian(&mut b32);

    let b0: [u8; 8] = b32[(0..8)].try_into().expect("Slicing bytes failed");
    let b1: [u8; 8] = b32[(8..16)].try_into().expect("Slicing bytes failed");
    let b2: [u8; 8] = b32[(16..24)].try_into().expect("Slicing bytes failed");
    let b3: [u8; 8] = b32[(24..32)].try_into().expect("Slicing bytes failed");

    let f0 = u64::from_le_bytes(b0) as f64;
    let f1 = u64::from_le_bytes(b1) as f64 * 2.0_f64.powi(64);
    let f2 = u64::from_le_bytes(b2) as f64 * 2.0_f64.powi(128);
    let f3 = u64::from_le_bytes(b3) as f64 * 2.0_f64.powi(192);
    let r = f0 + f1 + f2 + f3;
    r * 10.0_f64.powi(-18)
}

pub async fn send_tg(msg: &str) -> eyre::Result<()> {
    let token = "TOKEN_HERE";
    let chat_id = "CHAT_ID_HERE";
    get(format!(
        "https://api.telegram.org/bot{token}/sendMessage?chat_id={chat_id}&text={msg}"
    ))
    .await?;
    Ok(())
} //add these to secrtets

pub async fn complete_buffered(
    calls: Vec<impl Future<Output = eyre::Result<()>>>,
    n_buf: usize,
) -> eyre::Result<()> {
    let mut stream = tokio_stream::iter(calls).buffer_unordered(n_buf);
    while let Some(result) = stream.next().await {
        match result {
            Ok(_) => (),
            Err(e) => warn!("{:?}", e),
        }
    }
    Ok(())
}

pub fn apply_state_diff(state: &mut State, state_diff: StateDiff) {
    for (account_address, account_diff) in state_diff.0 {
        let bal = get_diff(account_diff.balance);
        let nonce = get_diff(account_diff.nonce);
        let code = get_diff(account_diff.code);
        
        
        state.account(account_address).balance = bal;
    
        state.account(account_address).nonce = nonce.map(|n| n.as_u64().into());

        state.account(account_address).code = code;
 
        
        for (storage_address, storage_diff) in account_diff.storage {
            if let Some(s_diff) = get_diff(storage_diff) {
                state.account(account_address).store(storage_address, s_diff.as_bytes().into());
            }
        }
    }
}

pub fn get_diff<T>(diff: Diff<T>) -> Option<T> {
    match diff {
        Diff::Same => None,
        Diff::Born(t) => Some(t),
        Diff::Died(_) => None,
        Diff::Changed(changed) => Some(changed.to),
    }
}

pub fn no_errors(bundle: &SimulatedBundle) -> bool {
    for tx in &bundle.transactions {
        if tx.error.is_some() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::utils::to_eth;
    use ethers::prelude::U256;
    use rand::thread_rng;
    use rand::Rng;
    #[test]
    fn test_eth_conv() {
        let mut rng = thread_rng();
        let x: u64 = rng.gen();
        println!("x={}", x);
        let wei = U256::from(x);
        let eth = to_eth(wei);
        println!("eth = {}", eth);
        assert!((x as f64 / 1000_000_000_000_000_000.0 - eth).abs() < 0.001);
    }
}

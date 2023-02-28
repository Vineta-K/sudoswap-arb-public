#[cfg(test)]
pub mod tests {
    use crate::abigen::{LooksRareExchange, NFTxVaultContract, SudoPairETH, ERC20, ERC721};
    use crate::looks::{LooksQuote, LooksResp};
    use crate::multi::Call;
    use crate::nftx::NFTxVault;
    use crate::sudo::{PoolType, SudoContract, SudoPool};
    use crate::utils::{apply_state_diff, to_eth};
    use crate::ApiKeys;
    use crate::{
        abigen::{Multi, UniRouterV2, Weth},
        multi::{Multicall, MulticallHeader},
    };
    use ethers::prelude::*;
    use ethers::providers::call_raw::spoof;
    use ethers_flashbots::{BundleRequest, SimulatedBundle};
    use std::collections::HashSet;
    use std::sync::Arc;

    #[tokio::test]
    pub async fn test_nftx_pricing() {
        let vault_address = "0xfc9A95101e7C4034989CDDEb062E906d93ffC7a5"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let sushi_router_address = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let weth_address = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse::<Address>()
            .unwrap();
        let ws = Ws::connect("ws://localhost:8545").await.unwrap();
        let provider = Provider::new(ws);
        let client = Arc::new(provider);

        let contract = NFTxVaultContract::new(vault_address, client.clone());
        let vault = NFTxVault {
            vault_address,
            contract,
        };
        let router = UniRouterV2::new(sushi_router_address, client.clone());

        let buy_price = vault.get_buy_price(&router, 1, weth_address).await.unwrap();
        println!("Buy Price: {}", to_eth(buy_price.unwrap()));
        let sell_price = vault
            .get_sell_price(&router, 1, weth_address)
            .await
            .unwrap();
        println!("Sell Price: {}", to_eth(sell_price.unwrap()));
    }
    // #[tokio::test]
    pub async fn test_call_bundle() {
        let pool_address = "0xa069e4e3407ebae16de63f76ff322cbf7e167fbd"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let nft_address = "0xAB054A3b1CF3862FdaeB251FFb32C49d26755ECf"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let exec_addr = "0xe25244692b11d9B78387e3C69753DD653631267B"
            .parse::<Address>()
            .unwrap();
        let bot_addr = "0xF7BCAFADe91139b428Acd9494767a972EBab2E69"
            .parse::<Address>()
            .unwrap();
        let api_keys: ApiKeys =
            serde_json::from_reader(std::fs::File::open("apikeys.json").unwrap()).unwrap();
        let wallet = api_keys.bot_key.parse::<LocalWallet>().unwrap();

        let ws = Ws::connect("ws://localhost:8546").await.unwrap();
        let provider = Provider::new(ws);
        let client = Arc::new(provider);
        let executor = Multi::new(exec_addr, client.clone());
        let nft = ERC721::new(nft_address, client.clone());
        let pool = SudoPool {
            pool_address,
            pool_type: PoolType::Trade,
            pair_variant: 0,
            contract: SudoContract::ETH(SudoPairETH::new(pool_address, client.clone())),
            nft_contract: nft,
        };

        let mut calls = Vec::new();

        let contract = match &pool.contract {
            SudoContract::ETH(c) => c,
            SudoContract::ERC20(_) => todo!(),
        };

        let (_, _, _, price, fee) = contract
            .get_buy_nft_quote(U256::from(1))
            .call()
            .await
            .unwrap();
        let (call, ids) = pool.create_buy_txs(price, exec_addr, 1).await.unwrap();
        calls.push(call);
        // println!("{:?}",ids);
        // // let (_, _, _, price, fee) = pool.get_buy_nft_quote(U256::from(1)).call().await.unwrap();
        // // let (call,ids) = create_sudo_buy_txs(&pool, price, bot_addr).await.unwrap();
        // // calls.push(call);

        // let call = create_sudo_sell_txs(&pool, price, exec_addr, ids.unwrap(), &nft)
        //     .await
        //     .unwrap();
        // calls.push(call);
        let calls: Vec<Call> = calls.into_iter().flatten().collect();

        let block_number = client.get_block_number().await.unwrap();
        let nonce = client.get_transaction_count(bot_addr, None).await.unwrap();
        let eth_to_coinbase = 69420;
        let header = MulticallHeader::new(false, false, eth_to_coinbase, block_number.as_u64());
        let multicall = Multicall::new(header, calls);
        let multi_params = multicall.encode_parameters();

        let tx = &mut executor.ostium(multi_params).gas(1_000_000).tx;

        tx.set_nonce(nonce);
        tx.set_chain_id(1);
        tx.set_gas_price(100_000_000_000_u128);
        client.fill_transaction(tx, None).await.unwrap();
        let signature = wallet.sign_transaction(&tx).await.unwrap();

        let mut bundle = BundleRequest::new();
        bundle = bundle.push_transaction(tx.rlp_signed(&signature));
        bundle = bundle
            .set_block(block_number + 1)
            .set_simulation_block(block_number)
            .set_simulation_timestamp(0);
        println!("{}", serde_json::json!([&bundle]));
        let sim_bundle: SimulatedBundle = client
            .provider()
            .request("eth_callBundle", [&bundle])
            .await
            .unwrap();
    }

    // #[tokio::test]
    pub async fn test_multi_value_sudo_call() {
        let ws = Ws::connect("ws://localhost:8545").await.unwrap(); //PLEASE DONT EVER PUT A LIVE NODE IN HERE AGAIN YOU IDIOT
        let provider = Provider::new(ws);
        let client = Arc::new(provider);
        let weth_addr = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse::<Address>()
            .unwrap();
        let exec_addr = "0xe25244692b11d9B78387e3C69753DD653631267B"
            .parse::<Address>()
            .unwrap();
        let bot_addr = "0xF7BCAFADe91139b428Acd9494767a972EBab2E69"
            .parse::<Address>()
            .unwrap();
        let sushi_addr = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F"
            .parse::<Address>()
            .unwrap();
        let wbtc_addr = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
            .parse::<Address>()
            .unwrap();
        let pool_address = "0xa069e4e3407ebae16de63f76ff322cbf7e167fbd"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let nft_address = "0xAB054A3b1CF3862FdaeB251FFb32C49d26755ECf"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let api_keys: ApiKeys =
            serde_json::from_reader(std::fs::File::open("apikeys.json").unwrap()).unwrap();
        let wallet = api_keys.bot_key.parse::<LocalWallet>().unwrap();

        let weth = Weth::new(weth_addr, client.clone());
        let executor = Multi::new(exec_addr, client.clone());
        let sushi = UniRouterV2::new(sushi_addr, client.clone());
        let wbtc = ERC20::new(wbtc_addr, client.clone());
        let nft = ERC721::new(nft_address, client.clone());
        let pool = SudoPool {
            pool_address: pool_address,
            pool_type: PoolType::Trade,
            pair_variant: 0,
            contract: SudoContract::ETH(SudoPairETH::new(pool_address, client.clone())),
            nft_contract: nft.clone(),
        };

        let mut calls = Vec::new();

        let contract = match &pool.contract {
            SudoContract::ETH(c) => c,
            SudoContract::ERC20(_) => todo!(),
        };
        let (_, _, _, price, fee) = contract
            .get_buy_nft_quote(U256::from(1))
            .call()
            .await
            .unwrap();
        let (call, ids) = pool.create_buy_txs(price, exec_addr, 1).await.unwrap();
        calls.push(call);
        // println!("{:?}",ids);
        // // let (_, _, _, price, fee) = pool.get_buy_nft_quote(U256::from(1)).call().await.unwrap();
        // // let (call,ids) = create_sudo_buy_txs(&pool, price, bot_addr).await.unwrap();
        // // calls.push(call);

        let call = pool
            .create_sell_txs(price, exec_addr, ids.unwrap(), &nft)
            .await
            .unwrap();
        calls.push(call);
        let calls: Vec<Call> = calls.into_iter().flatten().collect();

        let block_number = U256::from(0); //client.get_block_number().await.unwrap();
        let nonce = client.get_transaction_count(bot_addr, None).await.unwrap();
        let eth_to_coinbase = 69420;
        let header = MulticallHeader::new(false, false, eth_to_coinbase, block_number.as_u64());
        let multicall = Multicall::new(header, calls);
        let multi_params = multicall.encode_parameters();

        let tx = &mut executor.ostium(multi_params).gas(1_000_000).tx;

        tx.set_nonce(nonce);
        tx.set_chain_id(1);
        client.fill_transaction(tx, None).await.unwrap();
        let signature = wallet.sign_transaction(&tx).await.unwrap();
        println!(
            "wbtc before: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "weth before: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth before: {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        let rc = client
            .send_raw_transaction(tx.rlp_signed(&signature))
            .await
            .unwrap();
        println!(
            "wbtc after: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth after {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        println!(
            "weth after: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
    }
    // #[tokio::test]
    pub async fn test_multi_value_nftx_call() {
        let ws = Ws::connect("ws://localhost:8545").await.unwrap(); //PLEASE DONT EVER PUT A LIVE NODE IN HERE AGAIN YOU IDIOT
        let provider = Provider::new(ws);
        let client = Arc::new(provider);
        let weth_addr = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse::<Address>()
            .unwrap();
        let exec_addr = "0xe25244692b11d9B78387e3C69753DD653631267B"
            .parse::<Address>()
            .unwrap();
        let bot_addr = "0xF7BCAFADe91139b428Acd9494767a972EBab2E69"
            .parse::<Address>()
            .unwrap();
        let sushi_addr = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F"
            .parse::<Address>()
            .unwrap();
        let wbtc_addr = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
            .parse::<Address>()
            .unwrap();
        let vault_address = "0x4D3f0E89b46c0f9d7E3cE80Bc577351D3DFe4e51"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let nft_address = "0xA30e99E347f7F69e6eDdceAD7e313Cf7AF443Ce9"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let api_keys: ApiKeys =
            serde_json::from_reader(std::fs::File::open("apikeys.json").unwrap()).unwrap();
        let wallet = api_keys.bot_key.parse::<LocalWallet>().unwrap();

        let weth = Weth::new(weth_addr, client.clone());
        let executor = Multi::new(exec_addr, client.clone());
        let sushi = UniRouterV2::new(sushi_addr, client.clone());
        let wbtc = ERC20::new(wbtc_addr, client.clone());
        let vault = NFTxVault {
            contract: NFTxVaultContract::new(vault_address, client.clone()),
            vault_address,
        };
        let nft = ERC721::new(nft_address, client.clone());

        let mut calls = Vec::new();

        let price = vault
            .get_buy_price(&sushi, 1, weth_addr)
            .await
            .unwrap()
            .unwrap();
        let (call, ids) = vault
            .create_buy_txs(price, 1, exec_addr, &sushi, weth_addr)
            .await
            .unwrap();
        calls.push(call);

        let price = vault
            .get_sell_price(&sushi, 1, weth_addr)
            .await
            .unwrap()
            .unwrap();
        let call = vault
            .create_sell_txs(price, exec_addr, ids.unwrap(), &sushi, &nft, weth_addr)
            .await
            .unwrap();
        calls.push(call);
        let calls: Vec<Call> = calls.into_iter().flatten().collect();

        let block_number = U256::from(0); //client.get_block_number().await.unwrap();
        let nonce = client.get_transaction_count(bot_addr, None).await.unwrap();
        let eth_to_coinbase = 69420;
        let header = MulticallHeader::new(false, false, eth_to_coinbase, block_number.as_u64());
        let multicall = Multicall::new(header, calls);
        let multi_params = multicall.encode_parameters();

        let tx = &mut executor.ostium(multi_params).gas(1_000_000).tx;

        tx.set_nonce(nonce);
        tx.set_chain_id(1);
        client.fill_transaction(tx, None).await.unwrap();
        let signature = wallet.sign_transaction(&tx).await.unwrap();
        println!(
            "wbtc before: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "weth before: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth before: {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        let rc = client
            .send_raw_transaction(tx.rlp_signed(&signature))
            .await
            .unwrap();
        println!(
            "wbtc after: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth after {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        println!(
            "weth after: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
    }
    #[tokio::test]
    pub async fn test_looks_buy() {
        let ws = Ws::connect("ws://localhost:8545").await.unwrap(); //PLEASE DONT EVER PUT A LIVE NODE IN HERE AGAIN YOU IDIOT
        let provider = Provider::new(ws);
        let client = Arc::new(provider);
        let weth_addr = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse::<Address>()
            .unwrap();
        let exec_addr = "0xe25244692b11d9B78387e3C69753DD653631267B"
            .parse::<Address>()
            .unwrap();
        let bot_addr = "0xF7BCAFADe91139b428Acd9494767a972EBab2E69"
            .parse::<Address>()
            .unwrap();
        let sushi_addr = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F"
            .parse::<Address>()
            .unwrap();
        let wbtc_addr = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
            .parse::<Address>()
            .unwrap();
        let exch_addr = "0x59728544B08AB483533076417FbBB2fD0B17CE3a"
            .parse::<Address>()
            .unwrap();
        let api_keys: ApiKeys =
            serde_json::from_reader(std::fs::File::open("apikeys.json").unwrap()).unwrap();
        let wallet = api_keys.bot_key.parse::<LocalWallet>().unwrap();
        let endpoint_uri = api_keys.looks_rare_endpoint;

        let weth = Weth::new(weth_addr, client.clone());
        let executor = Multi::new(exec_addr, client.clone());

        let wbtc = ERC20::new(wbtc_addr, client.clone());
        let exchange = LooksRareExchange::new(exch_addr, client.clone());

        let http_client = reqwest::Client::new();
        let num = 2;
        let max_price = client.get_balance(exec_addr, None).await.unwrap();
        let query = format!("?strategy=0x56244Bb70CbD3EA9Dc8007399F61dFC065190031&currency=0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2&collection=0x1792a96E5668ad7C167ab804a100ce42395Ce54D&status[]=VALID&pagination[first]={}&price[max]={}&sort=PRICE_ASC&isOrderAsk=true",num,max_price);
        let uri = endpoint_uri.to_owned() + "api/v1/orders" + query.as_str();
        let r = http_client
            .get(uri.as_str())
            .header("accept", "application/json");
        let response = r.send().await.unwrap();
        let r: LooksResp = response.json().await.unwrap();
        let quote: &LooksQuote = &r.data[0];

        let mut calls = Vec::new();

        let (call, ids) = quote.create_buy_txs(exec_addr, &exchange).await.unwrap();
        calls.push(call);
        let calls = calls.into_iter().flatten().collect::<Vec<Call>>();

        let block_number = U256::from(0); //client.get_block_number().await.unwrap();
        let nonce = client.get_transaction_count(bot_addr, None).await.unwrap();
        let eth_to_coinbase = 69420;
        let header = MulticallHeader::new(false, false, eth_to_coinbase, block_number.as_u64());
        let multicall = Multicall::new(header, calls);
        let multi_params = multicall.encode_parameters();

        let tx = &mut executor.ostium(multi_params).gas(1_0000_000).tx;

        tx.set_nonce(nonce);
        tx.set_chain_id(1);
        client.fill_transaction(tx, None).await.unwrap();
        let signature = wallet.sign_transaction(&tx).await.unwrap();
        println!(
            "wbtc before: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "weth before: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth before: {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        let rc = client
            .send_raw_transaction(tx.rlp_signed(&signature))
            .await
            .unwrap();
        println!(
            "wbtc after: {}",
            wbtc.balance_of(exec_addr).call().await.unwrap()
        );
        println!(
            "eth after {}",
            client.get_balance(exec_addr, None).await.unwrap()
        );
        println!(
            "weth after: {}",
            weth.balance_of(exec_addr).call().await.unwrap()
        );
    }

    #[tokio::test]
    pub async fn test_state_diff() {
        let ws = Ws::connect("ws://localhost:8546").await.unwrap();
        let provider = Provider::new(ws);
        let client = Arc::new(provider);
        let weth_addr = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            .parse::<Address>()
            .unwrap();
        let exec_addr = "0xe25244692b11d9B78387e3C69753DD653631267B"
            .parse::<Address>()
            .unwrap();
        let bot_addr = "0xF7BCAFADe91139b428Acd9494767a972EBab2E69"
            .parse::<Address>()
            .unwrap();
        let sushi_addr = "0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F"
            .parse::<Address>()
            .unwrap();
        let wbtc_addr = "0x2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"
            .parse::<Address>()
            .unwrap();
        let pool_address = "0xa069e4e3407ebae16de63f76ff322cbf7e167fbd"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let nft_address = "0xAB054A3b1CF3862FdaeB251FFb32C49d26755ECf"
            .parse::<Address>()
            .unwrap(); //sudo inu lol
        let api_keys: ApiKeys =
            serde_json::from_reader(std::fs::File::open("apikeys.json").unwrap()).unwrap();
        let wallet = api_keys.bot_key.parse::<LocalWallet>().unwrap();

        let weth = Weth::new(weth_addr, client.clone());
        let executor = Multi::new(exec_addr, client.clone());
        let sushi = UniRouterV2::new(sushi_addr, client.clone());
        let wbtc = ERC20::new(wbtc_addr, client.clone());
        let nft = ERC721::new(nft_address, client.clone());
        let pool = SudoPool {
            pool_address: pool_address,
            pool_type: PoolType::Trade,
            pair_variant: 0,
            contract: SudoContract::ETH(SudoPairETH::new(pool_address, client.clone())),
            nft_contract: nft.clone(),
        };

        let mut calls = Vec::new();

        let contract = match &pool.contract {
            SudoContract::ETH(c) => c,
            SudoContract::ERC20(_) => todo!(),
        };
        let (_, _, _, price, fee) = contract
            .get_buy_nft_quote(U256::from(1))
            .call()
            .await
            .unwrap();
        let (call, ids) = pool.create_buy_txs(price, exec_addr, 1).await.unwrap();
        calls.push(call);
        // println!("{:?}",ids);
        // // let (_, _, _, price, fee) = pool.get_buy_nft_quote(U256::from(1)).call().await.unwrap();
        // // let (call,ids) = create_sudo_buy_txs(&pool, price, bot_addr).await.unwrap();
        // // calls.push(call);
        let calls: Vec<Call> = calls.into_iter().flatten().collect();

        let block_number = U256::from(0); //client.get_block_number().await.unwrap();
        let nonce = client.get_transaction_count(bot_addr, None).await.unwrap();
        let eth_to_coinbase = 69420;
        let header = MulticallHeader::new(false, false, eth_to_coinbase, block_number.as_u64());
        let multicall = Multicall::new(header, calls);
        let multi_params = multicall.encode_parameters();

        let tx = &mut executor.ostium(multi_params).gas(1_000_000).tx;

        tx.set_nonce(nonce);
        tx.set_chain_id(1);
        tx.set_from(bot_addr);
        client.fill_transaction(tx, None).await.unwrap();
        let signature = wallet.sign_transaction(&tx).await.unwrap();
        let tx = tx.as_eip1559_ref().unwrap();
        let trace_type = TraceType::StateDiff;
        let trace = client
            .trace_call(tx.to_owned(), vec![trace_type], None)
            .await;
        let diff = trace.unwrap().state_diff.unwrap();
        let touched_addresses: HashSet<Address> = diff.0.iter().map(|(k, _)| *k).collect();
        let mut state = spoof::state();
        apply_state_diff(&mut state, diff);
        let price_before = pool.get_buy_price(1, None).await.unwrap().unwrap();
        let price_after = pool.get_buy_price(1, Some(&state)).await.unwrap().unwrap();
        println!(
            "Before: {}, After {}",
            to_eth(price_before),
            to_eth(price_after)
        );
        assert_ne!(price_before, price_after);
    }
}

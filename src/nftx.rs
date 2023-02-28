use ethers::prelude::*;

use crate::abigen::{NFTxVaultContract, UniRouterV2, ERC721};
use crate::multi::{Call, Type};
use crate::ONE;

use eyre::eyre;

#[derive(Debug, Clone)]
pub struct NFTxVault<M: Middleware> {
    pub vault_address: Address,
    pub contract: NFTxVaultContract<M>,
    // pub approved: bool,
}

impl<M> NFTxVault<M>
where
    M: Middleware + 'static,
{
    pub async fn get_buy_price(
        &self,
        router: &UniRouterV2<M>,
        num_nft: usize,
        weth_address: Address,
    ) -> eyre::Result<Option<U256>> {
        let (_, _, redeem_fee, _, _) = self.contract.vault_fees().call().await?;
        let amount = U256::from(num_nft) * (U256::from(ONE) + redeem_fee);
        let path = vec![weth_address, self.vault_address];
        let price = router.get_amounts_in(amount, path).call().await;
        match price {
            Ok(p) => Ok(Some(p[0])),
            Err(e) => Err(eyre!(
                "Vault address: {:?}, Error {}",
                self.vault_address,
                e
            )),
        }
    }
    pub async fn get_sell_price(
        &self,
        router: &UniRouterV2<M>,
        num_nft: usize,
        weth_address: Address,
    ) -> eyre::Result<Option<U256>> {
        let (mint_fee, _, _, _, _) = self.contract.vault_fees().call().await?;
        let amount = U256::from(num_nft) * (U256::from(ONE) - mint_fee);
        let path = vec![self.vault_address, weth_address];
        let price = router.get_amounts_out(amount, path).call().await?;
        Ok(Some(price[1]))
    }
    pub async fn create_buy_txs(
        &self,
        input: U256,
        num_nft: usize,
        address: Address,
        router: &UniRouterV2<M>,
        weth_address: Address,
    ) -> eyre::Result<(Vec<Call>, Option<Vec<U256>>)> {
        let mut calls = Vec::new();
        let contract = &self.contract;
        let mut ids = Vec::new();
        for i in 0..num_nft {
            let id = contract.nft_id_at(U256::from(i)).call().await?;
            ids.push(id);
        }
        let (_, _, redeem_fee, _, _) = contract.vault_fees().call().await?;
        let path = vec![weth_address, contract.address()];
        let payload = router
            .swap_eth_for_exact_tokens(
                U256::from(num_nft) * (U256::from(ONE) + redeem_fee),
                path,
                address,
                U256::from(2655764228_u128),
            )
            .calldata()
            .unwrap();
        calls.push(Call::new(
            router.address(),
            payload[0..4].to_vec(),
            Type::ValueCall,
            Option::from(input),
            payload[4..].to_vec(),
        ));
        let payload = contract
            .redeem_to(U256::from(num_nft), ids.clone(), address)
            .calldata()
            .unwrap();
        calls.push(Call::new(
            contract.address(),
            payload[0..4].to_vec(),
            Type::Call,
            None,
            payload[4..].to_vec(),
        ));
        Ok((calls, Some(ids)))
    }

    pub async fn create_sell_txs(
        &self,
        output: U256,
        address: Address,
        ids: Vec<U256>,
        router: &UniRouterV2<M>,
        nft_contract: &ERC721<M>,
        weth_address: Address,
    ) -> eyre::Result<Vec<Call>> {
        let num_nft = ids.len();
        let mut amts = Vec::new();
        for _ in 0..num_nft {
            amts.push(U256::from(1));
        }
        let mut calls = Vec::new();
        let contract = &self.contract;
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
        };
        let payload = contract.mint(ids, amts).calldata().unwrap();
        calls.push(Call::new(
            contract.address(),
            payload[0..4].to_vec(),
            Type::Call,
            None,
            payload[4..].to_vec(),
        ));
        let allowance = contract.allowance(address, router.address()).call().await?;
        if allowance < U256::MAX {
            let payload = contract
                .approve(router.address(), U256::MAX)
                .calldata()
                .unwrap();
            calls.push(Call::new(
                contract.address(),
                payload[0..4].to_vec(),
                Type::Call,
                None,
                payload[4..].to_vec(),
            ));
        }
        let path = vec![contract.address(), weth_address];
        let (mint_fee, _, _, _, _) = contract.vault_fees().call().await?;
        let payload = router
            .swap_exact_tokens_for_eth(
                U256::from(num_nft) * (U256::from(ONE) - mint_fee), 
                output,
                path,
                address,
                U256::from(2655764228_u128),
            )
            .calldata()
            .unwrap();
        calls.push(Call::new(
            router.address(),
            payload[0..4].to_vec(),
            Type::Call,
            None,
            payload[4..].to_vec(),
        ));
        Ok(calls)
    }
}

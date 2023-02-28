use ethers::contract::abigen;

abigen!(
    SudoPairFactory,
    "./abi/LSSVMPairFactory.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    SudoPairETH,
    "./abi/LSSVMPairETH.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    SudoPairERC20,
    "./abi/LSSVMPairERC20.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    NFTxVaultContract,
    "./abi/NFTXVault.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    NFTxFactory,
    "./abi/NFTXVaultFactory.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    UniRouterV2,
    "./abi/UniRouterV2.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    ERC721,
    "./abi/IERC721.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    ERC20,
    "./abi/IERC20.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    Multi,
    "./abi/0xAMulti.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    Weth,
    "./abi/WETH.json",
    event_derives(serde::Deserialize, serde::Serialize),
);

abigen!(
    LooksRareExchange,
    "./abi/LooksRareExchange.json",
    event_derives(serde::Deserialize, serde::Serialize),
);



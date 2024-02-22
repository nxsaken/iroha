use std::{collections::HashSet, str::FromStr as _};

use eyre::Result;
use iroha_client::{
    client::{self, QueryResult},
    data_model::prelude::*,
};
use test_network::*;

use crate::integration::new_account_with_random_public_key;

#[test]
fn find_accounts_with_asset() -> Result<()> {
    let (_rt, _peer, test_client) = <PeerBuilder>::new().with_port(10_760).start_with_runtime();
    wait_for_genesis_committed(&[test_client.clone()], 0);

    // Registering new asset definition
    let definition_id = AssetDefinitionId::from_str("test_coin#wonderland").expect("Valid");
    let asset_definition = AssetDefinition::numeric(definition_id.clone());
    test_client.submit_blocking(Register::asset_definition(asset_definition.clone()))?;

    // Checking results before all
    let received_asset_definition =
        test_client.request(client::asset::definition_by_id(definition_id.clone()))?;

    assert_eq!(received_asset_definition.id(), asset_definition.id());
    assert!(matches!(
        received_asset_definition.value_type(),
        AssetValueType::Numeric(_)
    ));

    let accounts: [AccountId; 5] = [
        "alice@wonderland".parse().expect("Valid"),
        "mad_hatter@wonderland".parse().expect("Valid"),
        "cheshire_cat@wonderland".parse().expect("Valid"),
        "caterpillar@wonderland".parse().expect("Valid"),
        "white_rabbit@wonderland".parse().expect("Valid"),
    ];

    // Registering accounts
    let register_accounts = accounts
        .iter()
        .skip(1) // Alice has already been registered in genesis
        .cloned()
        .map(|account_id| Register::account(new_account_with_random_public_key(account_id)))
        .collect::<Vec<_>>();
    test_client.submit_all_blocking(register_accounts)?;

    let mint_asset = accounts
        .iter()
        .cloned()
        .map(|account_id| AssetId::new(definition_id.clone(), account_id))
        .map(|asset_id| Mint::asset_numeric(numeric!(1), asset_id))
        .collect::<Vec<_>>();
    test_client.submit_all_blocking(mint_asset)?;

    let accounts = HashSet::from(accounts);

    // Checking results
    let received_asset_definition =
        test_client.request(client::asset::definition_by_id(definition_id.clone()))?;

    assert_eq!(received_asset_definition.id(), asset_definition.id());
    assert_eq!(
        received_asset_definition.value_type(),
        AssetValueType::Numeric(NumericSpec::default()),
    );

    let found_accounts = test_client
        .request(client::account::all_with_asset(definition_id))?
        .collect::<QueryResult<Vec<_>>>()?;
    let found_ids = found_accounts
        .into_iter()
        .map(|account| account.id().clone())
        .collect::<HashSet<_>>();

    assert_eq!(found_ids, accounts);

    Ok(())
}

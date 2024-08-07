//! Query functionality. The common error type is also defined here,
//! alongside functions for converting them into HTTP responses.
use std::cmp::Ordering;

use eyre::Result;
use iroha_data_model::{
    prelude::*,
    query::{
        error::QueryExecutionFail as Error, predicate::PredicateBox, Pagination, QueryOutputBox,
        Sorting,
    },
};
use parity_scale_codec::{Decode, Encode};

use crate::{
    prelude::ValidQuery,
    query::{
        cursor::{Batch as _, Batched},
        pagination::Paginate as _,
    },
    state::{StateReadOnly, WorldReadOnly},
};

/// Represents lazy evaluated query output
pub trait Lazy {
    /// Type of the lazy evaluated query output
    type Lazy<'a>;
}

/// Lazily evaluated equivalent of [`Query::Output`]
pub enum LazyQueryOutput<'a> {
    /// Concrete computed [`Query::Output`]
    QueryOutput(QueryOutputBox),
    /// Iterator over a set of [`Query::Output`]s
    Iter(Box<dyn Iterator<Item = QueryOutputBox> + 'a>),
}

impl LazyQueryOutput<'_> {
    /// If the underlying output is an iterator, apply all the query postprocessing:
    /// - filtering
    /// - sorting
    /// - pagination
    /// - batching
    ///
    /// # Errors
    ///
    /// - if fetch size is too big
    /// - defined pagination parameter for a query that returns singular result
    pub(crate) fn apply_postprocessing(
        self,
        filter: &PredicateBox,
        sorting: &Sorting,
        pagination: Pagination,
        fetch_size: FetchSize,
    ) -> Result<ProcessedQueryOutput, Error> {
        match self {
            // nothing applies to the singular results
            LazyQueryOutput::QueryOutput(output) => {
                if filter != &PredicateBox::default()
                    || sorting != &Sorting::default()
                    || pagination != Pagination::default()
                    || fetch_size != FetchSize::default()
                {
                    return Err(Error::InvalidSingularParameters);
                }

                Ok(ProcessedQueryOutput::Single(output))
            }
            LazyQueryOutput::Iter(iter) => {
                // filter the results
                let iter = iter.filter(move |v| filter.applies(v));

                // sort & paginate
                let output = match &sorting.sort_by_metadata_key {
                    Some(key) => {
                        // if sorting was requested, we need to retrieve all the results first
                        let mut pairs: Vec<(Option<QueryOutputBox>, QueryOutputBox)> = iter
                            .map(|value| {
                                let key = match &value {
                                    QueryOutputBox::Identifiable(IdentifiableBox::Asset(asset)) => {
                                        match asset.value() {
                                            AssetValue::Store(store) => {
                                                store.get(key).cloned().map(Into::into)
                                            }
                                            _ => None,
                                        }
                                    }
                                    QueryOutputBox::Identifiable(v) => {
                                        TryInto::<&dyn HasMetadata>::try_into(v)
                                            .ok()
                                            .and_then(|has_metadata| {
                                                has_metadata.metadata().get(key)
                                            })
                                            .cloned()
                                            .map(Into::into)
                                    }
                                    _ => None,
                                };
                                (key, value)
                            })
                            .collect();
                        pairs.sort_by(|(left_key, _), (right_key, _)| {
                            match (left_key, right_key) {
                                (Some(l), Some(r)) => l.cmp(r),
                                (Some(_), None) => Ordering::Less,
                                (None, Some(_)) => Ordering::Greater,
                                (None, None) => Ordering::Equal,
                            }
                        });
                        pairs
                            .into_iter()
                            .map(|(_, val)| val)
                            .paginate(pagination)
                            .collect::<Vec<_>>()
                    }
                    // no sorting, can just paginate the results without constructing the full output vec
                    None => iter.paginate(pagination).collect::<Vec<_>>(),
                };

                let fetch_size = fetch_size
                    .fetch_size
                    .unwrap_or(iroha_data_model::query::DEFAULT_FETCH_SIZE);
                if fetch_size > iroha_data_model::query::MAX_FETCH_SIZE {
                    return Err(Error::FetchSizeTooBig);
                }

                // split the results into batches of fetch_size
                Ok(ProcessedQueryOutput::Iter(output.batched(fetch_size)))
            }
        }
    }
}

/// An evaluated & post-processed query output that is ready to be sent to the live query store
///
/// It has all the parameters (filtering, sorting, pagination and batching) applied already
pub enum ProcessedQueryOutput {
    /// A single query output
    Single(QueryOutputBox),
    /// An iterable query result, batched into fetch_size-sized chunks
    Iter(Batched<Vec<QueryOutputBox>>),
}

impl Lazy for QueryOutputBox {
    type Lazy<'a> = LazyQueryOutput<'a>;
}

impl<T> Lazy for Vec<T> {
    type Lazy<'a> = Box<dyn Iterator<Item = T> + 'a>;
}

macro_rules! impl_lazy {
    ( $($ident:ty),+ $(,)? ) => { $(
        impl Lazy for $ident {
            type Lazy<'a> = Self;
        } )+
    };
}
impl_lazy! {
    bool,
    iroha_data_model::prelude::Numeric,
    iroha_data_model::role::Role,
    iroha_data_model::asset::Asset,
    iroha_data_model::asset::AssetDefinition,
    iroha_data_model::account::Account,
    iroha_data_model::domain::Domain,
    iroha_data_model::block::BlockHeader,
    iroha_primitives::json::JsonString,
    iroha_data_model::query::TransactionQueryOutput,
    iroha_data_model::executor::ExecutorDataModel,
    iroha_data_model::trigger::Trigger,
    iroha_data_model::parameter::Parameters,
}

/// Query Request statefully validated on the Iroha node side.
#[derive(Debug, Clone, Decode, Encode)]
#[repr(transparent)]
pub struct ValidQueryRequest(SignedQuery);

impl ValidQueryRequest {
    /// Validate query.
    ///
    /// # Errors
    /// - Account doesn't exist
    /// - Account doesn't have the correct public key
    /// - Account has incorrect permissions
    pub fn validate(
        query: SignedQuery,
        state_ro: &impl StateReadOnly,
    ) -> Result<Self, ValidationFail> {
        state_ro.world().executor().validate_query(
            state_ro,
            query.authority(),
            query.query().clone(),
        )?;
        Ok(Self(query))
    }

    /// Execute contained query on the [`StateSnapshot`].
    ///
    /// # Errors
    /// Forwards `self.query.execute` error.
    pub fn execute_and_process<'state>(
        &'state self,
        state_ro: &'state impl StateReadOnly,
    ) -> Result<ProcessedQueryOutput, Error> {
        let query = &self.0;

        query.query().execute(state_ro)?.apply_postprocessing(
            query.filter(),
            query.sorting(),
            query.pagination(),
            query.fetch_size(),
        )

        // We're not handling the LimitedMetadata case, because
        // the predicate when applied to it is ambiguous. We could
        // pattern match on that case, but we should assume that
        // metadata (since it's limited) isn't going to be too
        // difficult to filter client-side. I actually think that
        // Metadata should be restricted in what types it can
        // contain.
    }

    /// Return query authority
    pub fn authority(&self) -> &AccountId {
        self.0.authority()
    }
}

impl ValidQuery for QueryBox {
    fn execute<'state>(
        &self,
        state_ro: &'state impl StateReadOnly,
    ) -> Result<LazyQueryOutput<'state>, Error> {
        iroha_logger::debug!(query=%self, "Executing");

        macro_rules! match_all {
            ( non_iter: {$( $non_iter_query:ident ),+ $(,)?} $( $query:ident, )+ ) => {
                match self { $(
                    QueryBox::$non_iter_query(query) => query.execute(state_ro).map(QueryOutputBox::from).map(LazyQueryOutput::QueryOutput), )+ $(
                    QueryBox::$query(query) => query.execute(state_ro).map(|i| i.map(QueryOutputBox::from)).map(|iter| LazyQueryOutput::Iter(Box::new(iter))), )+
                }
            };
        }

        match_all! {
            non_iter: {
                FindAccountById,
                FindAssetById,
                FindAssetDefinitionById,
                FindAssetQuantityById,
                FindTotalAssetQuantityByAssetDefinitionId,
                FindDomainById,
                FindBlockHeaderByHash,
                FindTransactionByHash,
                FindTriggerById,
                FindRoleByRoleId,
                FindDomainKeyValueByIdAndKey,
                FindAssetKeyValueByIdAndKey,
                FindAccountKeyValueByIdAndKey,
                FindAssetDefinitionKeyValueByIdAndKey,
                FindTriggerKeyValueByIdAndKey,
                FindExecutorDataModel,
                FindAllParameters,
            }

            FindAllAccounts,
            FindAccountsByDomainId,
            FindAccountsWithAsset,
            FindAllAssets,
            FindAllAssetsDefinitions,
            FindAssetsByName,
            FindAssetsByAccountId,
            FindAssetsByAssetDefinitionId,
            FindAssetsByDomainId,
            FindAssetsByDomainIdAndAssetDefinitionId,
            FindAllDomains,
            FindAllPeers,
            FindAllBlocks,
            FindAllBlockHeaders,
            FindAllTransactions,
            FindTransactionsByAccountId,
            FindPermissionsByAccountId,
            FindAllActiveTriggerIds,
            FindTriggersByAuthorityId,
            FindTriggersByAuthorityDomainId,
            FindAllRoles,
            FindAllRoleIds,
            FindRolesByAccountId,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use iroha_crypto::{Hash, HashOf, KeyPair};
    use iroha_data_model::{parameter::TransactionParameters, query::error::FindError};
    use iroha_primitives::json::JsonString;
    use nonzero_ext::nonzero;
    use test_samples::{gen_account_in, ALICE_ID, ALICE_KEYPAIR};
    use tokio::test;

    use super::*;
    use crate::{
        block::*,
        kura::Kura,
        query::store::LiveQueryStore,
        smartcontracts::isi::Registrable as _,
        state::{State, World},
        sumeragi::network_topology::Topology,
        tx::AcceptedTransaction,
    };

    fn world_with_test_domains() -> World {
        let domain_id = DomainId::from_str("wonderland").expect("Valid");
        let domain = Domain::new(domain_id).build(&ALICE_ID);
        let account = Account::new(ALICE_ID.clone()).build(&ALICE_ID);
        let asset_definition_id = AssetDefinitionId::from_str("rose#wonderland").expect("Valid");
        let asset_definition = AssetDefinition::numeric(asset_definition_id).build(&ALICE_ID);
        World::with([domain], [account], [asset_definition])
    }

    fn world_with_test_asset_with_metadata() -> World {
        let asset_definition_id = AssetDefinitionId::from_str("rose#wonderland").expect("Valid");
        let domain = Domain::new(DomainId::from_str("wonderland").expect("Valid")).build(&ALICE_ID);
        let account = Account::new(ALICE_ID.clone()).build(&ALICE_ID);
        let asset_definition =
            AssetDefinition::numeric(asset_definition_id.clone()).build(&ALICE_ID);

        let mut store = Metadata::default();
        store.insert(
            Name::from_str("Bytes").expect("Valid"),
            vec![1_u32, 2_u32, 3_u32],
        );
        let asset_id = AssetId::new(asset_definition_id, account.id().clone());
        let asset = Asset::new(asset_id, AssetValue::Store(store));

        World::with_assets([domain], [account], [asset_definition], [asset])
    }

    fn world_with_test_account_with_metadata() -> Result<World> {
        let mut metadata = Metadata::default();
        metadata.insert(Name::from_str("Bytes")?, vec![1_u32, 2_u32, 3_u32]);

        let domain = Domain::new(DomainId::from_str("wonderland")?).build(&ALICE_ID);
        let account = Account::new(ALICE_ID.clone())
            .with_metadata(metadata)
            .build(&ALICE_ID);
        let asset_definition_id = AssetDefinitionId::from_str("rose#wonderland").expect("Valid");
        let asset_definition = AssetDefinition::numeric(asset_definition_id).build(&ALICE_ID);
        Ok(World::with([domain], [account], [asset_definition]))
    }

    fn state_with_test_blocks_and_transactions(
        blocks: u64,
        valid_tx_per_block: usize,
        invalid_tx_per_block: usize,
    ) -> Result<State> {
        let chain_id = ChainId::from("00000000-0000-0000-0000-000000000000");

        let kura = Kura::blank_kura_for_testing();
        let query_handle = LiveQueryStore::test().start();
        let state = State::new(world_with_test_domains(), kura.clone(), query_handle);
        {
            let mut state_block = state.block();
            let limits = TransactionParameters {
                max_instructions: nonzero!(1000_u64),
                smart_contract_size: nonzero!(1024_u64),
            };
            let huge_limits = TransactionParameters {
                max_instructions: nonzero!(1000_u64),
                smart_contract_size: nonzero!(1024_u64),
            };

            state_block.world.parameters.transaction = limits;

            let valid_tx = {
                let tx = TransactionBuilder::new(chain_id.clone(), ALICE_ID.clone())
                    .with_instructions::<InstructionBox>([])
                    .sign(ALICE_KEYPAIR.private_key());
                AcceptedTransaction::accept(tx, &chain_id, limits)?
            };
            let invalid_tx = {
                let fail_isi = Unregister::domain("dummy".parse().unwrap());
                let tx = TransactionBuilder::new(chain_id.clone(), ALICE_ID.clone())
                    .with_instructions([fail_isi.clone(), fail_isi])
                    .sign(ALICE_KEYPAIR.private_key());
                AcceptedTransaction::accept(tx, &chain_id, huge_limits)?
            };

            let mut transactions = vec![valid_tx; valid_tx_per_block];
            transactions.append(&mut vec![invalid_tx; invalid_tx_per_block]);

            let (peer_public_key, peer_private_key) = KeyPair::random().into_parts();
            let peer_id = PeerId::new("127.0.0.1:8080".parse().unwrap(), peer_public_key);
            let topology = Topology::new(vec![peer_id]);
            let first_block = BlockBuilder::new(transactions.clone())
                .chain(0, &mut state_block)
                .sign(&peer_private_key)
                .unpack(|_| {})
                .commit(&topology)
                .unpack(|_| {})
                .expect("Block is valid");

            let _events = state_block.apply(&first_block, topology.as_ref().to_owned())?;
            kura.store_block(first_block);

            for _ in 1u64..blocks {
                let block = BlockBuilder::new(transactions.clone())
                    .chain(0, &mut state_block)
                    .sign(&peer_private_key)
                    .unpack(|_| {})
                    .commit(&topology)
                    .unpack(|_| {})
                    .expect("Block is valid");

                let _events = state_block.apply(&block, topology.as_ref().to_owned())?;
                kura.store_block(block);
            }
            state_block.commit();
        }

        Ok(state)
    }

    #[test]
    async fn asset_store() -> Result<()> {
        let kura = Kura::blank_kura_for_testing();
        let query_handle = LiveQueryStore::test().start();
        let state = State::new(world_with_test_asset_with_metadata(), kura, query_handle);

        let asset_definition_id = AssetDefinitionId::from_str("rose#wonderland")?;
        let asset_id = AssetId::new(asset_definition_id, ALICE_ID.clone());
        let bytes = FindAssetKeyValueByIdAndKey::new(asset_id, Name::from_str("Bytes")?)
            .execute(&state.view())?;
        assert_eq!(JsonString::from(vec![1_u32, 2_u32, 3_u32,]), bytes,);
        Ok(())
    }

    #[test]
    async fn account_metadata() -> Result<()> {
        let kura = Kura::blank_kura_for_testing();
        let query_handle = LiveQueryStore::test().start();
        let state = State::new(world_with_test_account_with_metadata()?, kura, query_handle);

        let bytes = FindAccountKeyValueByIdAndKey::new(ALICE_ID.clone(), Name::from_str("Bytes")?)
            .execute(&state.view())?;
        assert_eq!(JsonString::from(vec![1_u32, 2_u32, 3_u32,]), bytes,);
        Ok(())
    }

    #[test]
    async fn find_all_blocks() -> Result<()> {
        let num_blocks = 100;

        let state = state_with_test_blocks_and_transactions(num_blocks, 1, 1)?;
        let blocks = FindAllBlocks.execute(&state.view())?.collect::<Vec<_>>();

        assert_eq!(blocks.len() as u64, num_blocks);
        assert!(blocks
            .windows(2)
            .all(|wnd| wnd[0].header() >= wnd[1].header()));

        Ok(())
    }

    #[test]
    async fn find_all_block_headers() -> Result<()> {
        let num_blocks = 100;

        let state = state_with_test_blocks_and_transactions(num_blocks, 1, 1)?;
        let block_headers = FindAllBlockHeaders
            .execute(&state.view())?
            .collect::<Vec<_>>();

        assert_eq!(block_headers.len() as u64, num_blocks);
        assert!(block_headers.windows(2).all(|wnd| wnd[0] >= wnd[1]));

        Ok(())
    }

    #[test]
    async fn find_block_header_by_hash() -> Result<()> {
        let state = state_with_test_blocks_and_transactions(1, 1, 1)?;
        let state_view = state.view();
        let block = state_view.all_blocks().last().expect("state is empty");

        assert_eq!(
            FindBlockHeaderByHash::new(block.hash()).execute(&state_view)?,
            *block.header()
        );

        assert!(
            FindBlockHeaderByHash::new(HashOf::from_untyped_unchecked(Hash::new([42])))
                .execute(&state_view)
                .is_err()
        );

        Ok(())
    }

    #[test]
    async fn find_all_transactions() -> Result<()> {
        let num_blocks = 100;

        let state = state_with_test_blocks_and_transactions(num_blocks, 1, 1)?;
        let state_view = state.view();
        let txs = FindAllTransactions
            .execute(&state_view)?
            .collect::<Vec<_>>();

        assert_eq!(txs.len() as u64, num_blocks * 2);
        assert_eq!(
            txs.iter()
                .filter(|txn| txn.transaction.error.is_some())
                .count() as u64,
            num_blocks
        );
        assert_eq!(
            txs.iter()
                .filter(|txn| txn.transaction.error.is_none())
                .count() as u64,
            num_blocks
        );

        Ok(())
    }

    #[test]
    async fn find_transaction() -> Result<()> {
        let chain_id = ChainId::from("00000000-0000-0000-0000-000000000000");

        let kura = Kura::blank_kura_for_testing();
        let query_handle = LiveQueryStore::test().start();
        let state = State::new(world_with_test_domains(), kura.clone(), query_handle);

        let mut state_block = state.block();
        let tx = TransactionBuilder::new(chain_id.clone(), ALICE_ID.clone())
            .with_instructions::<InstructionBox>([])
            .sign(ALICE_KEYPAIR.private_key());

        let tx_limits = state_block.transaction_executor().limits;
        let va_tx = AcceptedTransaction::accept(tx, &chain_id, tx_limits)?;

        let (peer_public_key, _) = KeyPair::random().into_parts();
        let peer_id = PeerId::new("127.0.0.1:8080".parse().unwrap(), peer_public_key);
        let topology = Topology::new(vec![peer_id]);
        let vcb = BlockBuilder::new(vec![va_tx.clone()])
            .chain(0, &mut state_block)
            .sign(ALICE_KEYPAIR.private_key())
            .unpack(|_| {})
            .commit(&topology)
            .unpack(|_| {})
            .expect("Block is valid");

        let _events = state_block.apply(&vcb, topology.as_ref().to_owned())?;
        kura.store_block(vcb);
        state_block.commit();

        let state_view = state.view();

        let unapplied_tx = TransactionBuilder::new(chain_id, ALICE_ID.clone())
            .with_instructions([Unregister::account(gen_account_in("domain").0)])
            .sign(ALICE_KEYPAIR.private_key());
        let wrong_hash = unapplied_tx.hash();
        let not_found = FindTransactionByHash::new(wrong_hash).execute(&state_view);
        assert!(matches!(
            not_found,
            Err(Error::Find(FindError::Transaction(_)))
        ));

        let found_accepted =
            FindTransactionByHash::new(va_tx.as_ref().hash()).execute(&state_view)?;
        if found_accepted.transaction.error.is_none() {
            assert_eq!(
                va_tx.as_ref().hash(),
                found_accepted.as_ref().as_ref().hash()
            )
        }
        Ok(())
    }

    #[test]
    async fn domain_metadata() -> Result<()> {
        let kura = Kura::blank_kura_for_testing();
        let state = {
            let mut metadata = Metadata::default();
            metadata.insert(Name::from_str("Bytes")?, vec![1_u32, 2_u32, 3_u32]);
            let domain = Domain::new(DomainId::from_str("wonderland")?)
                .with_metadata(metadata)
                .build(&ALICE_ID);
            let account = Account::new(ALICE_ID.clone()).build(&ALICE_ID);
            let asset_definition_id = AssetDefinitionId::from_str("rose#wonderland")?;
            let asset_definition = AssetDefinition::numeric(asset_definition_id).build(&ALICE_ID);
            let query_handle = LiveQueryStore::test().start();
            State::new(
                World::with([domain], [account], [asset_definition]),
                kura,
                query_handle,
            )
        };

        let domain_id = DomainId::from_str("wonderland")?;
        let key = Name::from_str("Bytes")?;
        let bytes = FindDomainKeyValueByIdAndKey::new(domain_id, key).execute(&state.view())?;
        assert_eq!(JsonString::from(vec![1_u32, 2_u32, 3_u32,]), bytes,);
        Ok(())
    }
}

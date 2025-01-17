use massa_consensus_exports::MockConsensusController;
use massa_models::config::MIP_STORE_STATS_BLOCK_CONSIDERED;
use massa_versioning::versioning::MipStatsConfig;
use massa_versioning::versioning::MipStore;
use num::rational::Ratio;
use parking_lot::RwLock;
use std::sync::Arc;

use massa_factory_exports::{
    test_exports::create_empty_block, FactoryChannels, FactoryConfig, FactoryManager,
};
use massa_models::{address::Address, block_id::BlockId, prehash::PreHashMap, slot::Slot};
use massa_pool_exports::MockPoolController;
use massa_pos_exports::MockSelectorController;
use massa_protocol_exports::MockProtocolController;
use massa_signature::KeyPair;
use massa_storage::Storage;
use massa_time::MassaTime;

use crate::start_factory;
use massa_wallet::test_exports::create_test_wallet;

/// This structure store all information and links to creates tests for the factory.
/// The factory will ask that to the the pool, consensus and factory and then will send the block to the consensus.
/// You can use the method `new` to build all the mocks and make the connections
#[allow(dead_code)]
pub struct TestFactory {
    factory_config: FactoryConfig,
    factory_manager: Box<dyn FactoryManager>,
    genesis_blocks: Vec<(BlockId, u64)>,
    pub(crate) storage: Storage,
    keypair: KeyPair,
}

impl TestFactory {
    /// Initialize a new factory and all mocks with default data
    /// Arguments:
    /// - `keypair`: this keypair will be the one added to the wallet that will be used to produce all blocks
    ///
    /// Returns
    /// - `TestFactory`: the structure that will be used to manage the tests
    pub fn new(default_keypair: &KeyPair) -> TestFactory {
        let mut selector_controller = Box::new(MockSelectorController::new());
        let block_selector_controller = Box::new(MockSelectorController::new());
        selector_controller
            .expect_clone_box()
            .return_once(move || block_selector_controller);
        let mut consensus_controller = Box::new(MockConsensusController::new());
        let block_consensus_controller = Box::new(MockConsensusController::new());
        consensus_controller
            .expect_clone_box()
            .return_once(move || block_consensus_controller);
        let mut pool_controller = Box::new(MockPoolController::new());
        let block_pool_controller = Box::new(MockPoolController::new());
        pool_controller
            .expect_clone_box()
            .return_once(move || block_pool_controller);
        let mut protocol_controller = Box::new(MockProtocolController::new());
        let block_protocol_controller = Box::new(MockProtocolController::new());
        protocol_controller
            .expect_clone_box()
            .return_once(move || block_protocol_controller);
        let mut storage = Storage::create_root();
        let mut factory_config = FactoryConfig::default();
        let producer_keypair = default_keypair;
        let producer_address = Address::from_public_key(&producer_keypair.get_public_key());
        let mut accounts = PreHashMap::default();

        let mut genesis_blocks = vec![];
        for i in 0..factory_config.thread_count {
            let block = create_empty_block(producer_keypair, &Slot::new(0, i));
            genesis_blocks.push((block.id, 0));
            storage.store_block(block);
        }

        accounts.insert(producer_address, producer_keypair.clone());
        factory_config.t0 = MassaTime::from_millis(400);
        factory_config.genesis_timestamp = factory_config
            .genesis_timestamp
            .checked_sub(factory_config.t0)
            .unwrap();

        // create an empty default store
        let mip_stats_config = MipStatsConfig {
            block_count_considered: MIP_STORE_STATS_BLOCK_CONSIDERED,
            warn_announced_version_ratio: Ratio::new_raw(30, 100),
        };
        let mip_store =
            MipStore::try_from(([], mip_stats_config)).expect("Cannot create an empty MIP store");

        let wallet = create_test_wallet(Some(accounts));
        let factory_manager = start_factory(
            factory_config.clone(),
            Arc::new(RwLock::new(wallet)),
            FactoryChannels {
                selector: selector_controller,
                consensus: consensus_controller,
                pool: pool_controller,
                protocol: protocol_controller,
                storage: storage.clone_without_refs(),
            },
            mip_store,
        );

        TestFactory {
            factory_config,
            factory_manager,
            genesis_blocks,
            storage,
            keypair: default_keypair.clone(),
        }
    }
}

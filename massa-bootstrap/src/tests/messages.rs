use std::time::Duration;

use crate::settings::BootstrapClientConfig;
use crate::tests::tools::parametric_test;
use crate::{
    BootstrapClientMessage, BootstrapClientMessageDeserializer, BootstrapClientMessageSerializer,
    BootstrapServerMessage, BootstrapServerMessageDeserializer, BootstrapServerMessageSerializer,
};
use massa_models::config::*;
use massa_serialization::{Deserializer, Serializer};

#[test]
fn test_serialize_bootstrap_server_message() {
    let config = BootstrapClientConfig {
        rate_limit: std::u64::MAX,
        max_listeners_per_peer: MAX_LISTENERS_PER_PEER as u32,
        endorsement_count: ENDORSEMENT_COUNT,
        max_advertise_length: MAX_ADVERTISE_LENGTH,
        max_bootstrap_blocks_length: MAX_BOOTSTRAP_BLOCKS,
        max_operations_per_block: MAX_OPERATIONS_PER_BLOCK,
        thread_count: THREAD_COUNT,
        randomness_size_bytes: BOOTSTRAP_RANDOMNESS_SIZE_BYTES,
        max_bootstrap_error_length: MAX_BOOTSTRAP_ERROR_LENGTH,
        max_versioning_elements_size: MAX_BOOTSTRAP_VERSIONING_ELEMENTS_SIZE,
        max_datastore_entry_count: MAX_DATASTORE_ENTRY_COUNT,
        max_datastore_key_length: MAX_DATASTORE_KEY_LENGTH,
        max_datastore_value_length: MAX_DATASTORE_VALUE_LENGTH,
        max_ledger_changes_count: MAX_LEDGER_CHANGES_COUNT,
        max_changes_slot_count: 1000,
        max_rolls_length: MAX_ROLLS_COUNT_LENGTH,
        max_production_stats_length: MAX_PRODUCTION_STATS_LENGTH,
        max_credits_length: MAX_DEFERRED_CREDITS_LENGTH,
        max_executed_ops_length: MAX_EXECUTED_OPS_LENGTH,
        max_ops_changes_length: MAX_EXECUTED_OPS_CHANGES_LENGTH,
        mip_store_stats_block_considered: MIP_STORE_STATS_BLOCK_CONSIDERED,
        max_denunciations_per_block_header: MAX_DENUNCIATIONS_PER_BLOCK_HEADER,
        max_denunciation_changes_length: MAX_DENUNCIATION_CHANGES_LENGTH,
    };

    parametric_test(
        Duration::from_secs(30),
        config,
        vec![
            5577929984194316755,
            9248055555568684907,
        ],
        |config, rng| {
            let msg = BootstrapServerMessage::generate(rng);
            let mut bytes = Vec::new();
            let ser_res = BootstrapServerMessageSerializer::new().serialize(&msg, &mut bytes);
            assert!(
                ser_res.is_ok(),
                "Serialization of bootstrap server message failed"
            );
            assert!(
                bytes.len() < MAX_BOOTSTRAP_MESSAGE_SIZE as usize,
                "(got) {} > {} (max limit)",
                bytes.len(),
                MAX_BOOTSTRAP_MESSAGE_SIZE
            );

            let deser = BootstrapServerMessageDeserializer::new(config.into());
            match deser.deserialize::<massa_serialization::DeserializeError>(&bytes) {
                Ok((rest, msg_res)) => {
                    assert!(rest.is_empty(), "Data left after deserialization");
                    assert!(msg_res.equals(&msg), "BootstrapServerMessages doesn't match after serialization / deserialization process")
                }
                Err(e) => {
                    let mut err_str = e.to_string();
                    if err_str.len() > 550 {
                        err_str = err_str[..550].to_string();
                    }
                    assert!(false, "Error while deserializing: {}", err_str);
                }
            }
        },
    );
}

#[test]
fn test_serialize_bootstrap_client_message() {
    parametric_test(
        Duration::from_secs(30),
        (),
        vec![6186847917072968589],
        |_, rng| {
            let msg = BootstrapClientMessage::generate(rng);
            let mut bytes = Vec::new();
            let ser_res = BootstrapClientMessageSerializer::new().serialize(&msg, &mut bytes);
            assert!(
                ser_res.is_ok(),
                "Serialization of bootstrap server message failed"
            );
            assert!(
                bytes.len() < MAX_BOOTSTRAP_MESSAGE_SIZE as usize,
                "(got) {} > {} (max limit)",
                bytes.len(),
                MAX_BOOTSTRAP_MESSAGE_SIZE
            );
            let deser = BootstrapClientMessageDeserializer::new(
                THREAD_COUNT,
                MAX_DATASTORE_KEY_LENGTH,
                MAX_CONSENSUS_BLOCKS_IDS,
            );
            match deser.deserialize::<massa_serialization::DeserializeError>(&bytes) {
                Ok((rest, msg_res)) => {
                    println!("{bytes:?}");
                    println!("{rest:?}");
                    assert!(rest.is_empty(), "Data left after deserialization");
                    println!("{msg_res:?}");
                    println!("{msg:?}");
                    assert!(msg_res.equals(&msg), "BootstrapClientMessages doesn't match after serialization / deserialization process")
                }
                Err(e) => {
                    let mut err_str = e.to_string();
                    if err_str.len() > 550 {
                        err_str = err_str[..550].to_string();
                    }
                    assert!(false, "Error while deserializing: {}", err_str);
                }
            }
        },
    );
}

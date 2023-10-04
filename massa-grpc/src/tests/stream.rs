use std::{ops::Add, time::Duration};

use crate::tests::mock::{grpc_public_service, MockExecutionCtrl, MockPoolCtrl};
use massa_execution_exports::{ExecutionOutput, SlotExecutionOutput};
use massa_models::{
    address::Address, block::FilledBlock, operation::OperationSerializer,
    secure_share::SecureShareSerializer, slot::Slot, stats::ExecutionStats,
};
use massa_proto_rs::massa::{
    api::v1::{
        public_service_client::PublicServiceClient, NewBlocksRequest, NewFilledBlocksRequest,
        NewOperationsRequest, NewSlotExecutionOutputsRequest, SendOperationsRequest,
        TransactionsThroughputRequest,
    },
    model::v1::{Addresses, Slot as ProtoSlot, SlotRange},
};
use massa_protocol_exports::{
    test_exports::tools::{
        create_block, create_block_with_operations, create_endorsement,
        create_operation_with_expire_period,
    },
    MockProtocolController,
};
use massa_serialization::Serializer;
use massa_signature::KeyPair;
use massa_time::MassaTime;
use serial_test::serial;
use tokio_stream::StreamExt;

const GRPC_SERVER_URL: &str = "grpc://localhost:8888";

#[tokio::test]
#[serial]
async fn transactions_throughput_stream() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();

    let mut exec_ctrl = MockExecutionCtrl::new();

    exec_ctrl.expect_clone().returning(|| {
        let mut exec_ctrl = MockExecutionCtrl::new();
        exec_ctrl.expect_get_stats().returning(|| {
            let now = MassaTime::now().unwrap();
            let futur = MassaTime::from_millis(
                now.to_millis()
                    .add(Duration::from_secs(30).as_millis() as u64),
            );

            ExecutionStats {
                time_window_start: now.clone(),
                time_window_end: futur,
                final_block_count: 10,
                final_executed_operations_count: 2000,
                active_cursor: massa_models::slot::Slot {
                    period: 2,
                    thread: 10,
                },
                final_cursor: massa_models::slot::Slot {
                    period: 3,
                    thread: 15,
                },
            }
        });
        exec_ctrl
    });

    exec_ctrl.expect_clone_box().returning(|| {
        let mut exec_ctrl = MockExecutionCtrl::new();
        exec_ctrl.expect_get_stats().returning(|| {
            let now = MassaTime::now().unwrap();
            let futur = MassaTime::from_millis(
                now.to_millis()
                    .add(Duration::from_secs(30).as_millis() as u64),
            );

            ExecutionStats {
                time_window_start: now.clone(),
                time_window_end: futur,
                final_block_count: 10,
                final_executed_operations_count: 2000,
                active_cursor: massa_models::slot::Slot {
                    period: 2,
                    thread: 10,
                },
                final_cursor: massa_models::slot::Slot {
                    period: 3,
                    thread: 15,
                },
            }
        });
        Box::new(exec_ctrl)
    });

    public_server.execution_controller = Box::new(exec_ctrl);

    let stop_handle = public_server.serve(&config).await.unwrap();

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    // channel for bi-directional streaming
    let (tx, rx) = tokio::sync::mpsc::channel(10);

    // Create a stream from the receiver.
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let mut resp_stream = public_client
        .transactions_throughput(request_stream)
        .await
        .unwrap()
        .into_inner();

    tx.send(TransactionsThroughputRequest { interval: Some(1) })
        .await
        .unwrap();

    let mut count = 0;
    let mut now = std::time::Instant::now();
    while let Some(received) = resp_stream.next().await {
        let received = received.unwrap();
        assert_eq!(received.throughput, 66);

        let time_to_get_msg = now.elapsed().as_secs_f64().round();

        if count < 2 {
            assert!(time_to_get_msg < 1.5);
        } else if count >= 2 && count < 4 {
            assert!(time_to_get_msg < 3.5 && time_to_get_msg > 2.5);
        } else {
            break;
        }

        now = std::time::Instant::now();

        count += 1;
        if count == 2 {
            // update interval to 3 seconds
            tx.send(TransactionsThroughputRequest { interval: Some(3) })
                .await
                .unwrap();
        }
    }

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn new_operations() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();
    let (op_tx, _op_rx) = tokio::sync::broadcast::channel(10);
    let keypair = massa_signature::KeyPair::generate(0).unwrap();
    let address = Address::from_public_key(&keypair.get_public_key());
    public_server.pool_channels.operation_sender = op_tx.clone();

    let stop_handle = public_server.serve(&config).await.unwrap();

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();
    let op = create_operation_with_expire_period(&keypair, 10);
    let (op_send_signal, mut rx_op_send) = tokio::sync::mpsc::channel(10);

    let (tx_request, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let tx_cloned = op_tx.clone();
    let op_cloned = op.clone();
    tokio::spawn(async move {
        loop {
            // when receive signal, broadcast op
            let _: () = rx_op_send.recv().await.unwrap();

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // send op
            tx_cloned.send(op_cloned.clone()).unwrap();
        }
    });

    let mut resp_stream = public_client
        .new_operations(request_stream)
        .await
        .unwrap()
        .into_inner();

    let filter = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::OperationIds(
                massa_proto_rs::massa::model::v1::OperationIds {
                    operation_ids: vec![
                        "O1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC".to_string()
                    ],
                },
            ),
        ),
    };

    // send filter with unknow op id
    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    op_send_signal.send(()).await.unwrap();

    // wait for response
    // should be timed out because of unknow op id
    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    // send filter with known op id
    let filter_id = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::OperationIds(
                massa_proto_rs::massa::model::v1::OperationIds {
                    operation_ids: vec![op.id.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_id.clone()],
        })
        .await
        .unwrap();

    op_send_signal.send(()).await.unwrap();

    // wait for response
    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap();
    let received = result.unwrap();
    assert_eq!(
        received.signed_operation.unwrap().content_creator_pub_key,
        keypair.get_public_key().to_string()
    );

    let mut filter_type = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::OperationTypes(
                massa_proto_rs::massa::model::v1::OpTypes {
                    op_types: vec![massa_proto_rs::massa::model::v1::OpType::CallSc as i32],
                },
            ),
        ),
    };

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_type],
        })
        .await
        .unwrap();

    op_send_signal.send(()).await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;

    assert!(result.is_err());

    filter_type = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::OperationTypes(
                massa_proto_rs::massa::model::v1::OpTypes {
                    op_types: vec![massa_proto_rs::massa::model::v1::OpType::Transaction as i32],
                },
            ),
        ),
    };

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_type.clone()],
        })
        .await
        .unwrap();

    op_send_signal.send(()).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap();
    let received = result.unwrap();
    assert_eq!(
        received.signed_operation.unwrap().content_creator_pub_key,
        keypair.get_public_key().to_string()
    );

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_type, filter_id],
        })
        .await
        .unwrap();
    op_send_signal.send(()).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        received.signed_operation.unwrap().content_creator_pub_key,
        keypair.get_public_key().to_string()
    );

    let mut filter_addr = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::Addresses(
                massa_proto_rs::massa::model::v1::Addresses {
                    addresses: vec![
                        "AU12BTfZ7k1z6PsLEUZeHYNirz6WJ3NdrWto9H4TkVpkV9xE2TJg2".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_addr.clone()],
        })
        .await
        .unwrap();
    op_send_signal.send(()).await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter_addr = massa_proto_rs::massa::api::v1::NewOperationsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_operations_filter::Filter::Addresses(
                massa_proto_rs::massa::model::v1::Addresses {
                    addresses: vec![address.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(NewOperationsRequest {
            filters: vec![filter_addr.clone()],
        })
        .await
        .unwrap();
    op_send_signal.send(()).await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        received.signed_operation.unwrap().content_creator_pub_key,
        keypair.get_public_key().to_string()
    );

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn new_blocks() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();
    let (block_tx, _block_rx) = tokio::sync::broadcast::channel(10);

    public_server.consensus_channels.block_sender = block_tx.clone();

    let stop_handle = public_server.serve(&config).await.unwrap();

    let keypair = KeyPair::generate(0).unwrap();
    let address = Address::from_public_key(&keypair.get_public_key());
    let op = create_operation_with_expire_period(&keypair, 4);
    // let address = Address::from_public_key(&keypair.get_public_key());

    let block_op = create_block_with_operations(
        &keypair,
        Slot {
            period: 1,
            thread: 4,
        },
        vec![op.clone()],
    );

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    let (tx_request, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let mut resp_stream = public_client
        .new_blocks(request_stream)
        .await
        .unwrap()
        .into_inner();

    let mut filter_slot = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::SlotRange(SlotRange {
                start_slot: Some(ProtoSlot {
                    period: 1,
                    thread: 1,
                }),
                end_slot: None,
            }),
        ),
    };
    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_slot.clone()],
        })
        .await
        .unwrap();

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_block.is_some());

    filter_slot = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::SlotRange(SlotRange {
                start_slot: Some(ProtoSlot {
                    period: 1,
                    thread: 15,
                }),
                end_slot: None,
            }),
        ),
    };

    // update filter
    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_slot],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    // elapsed
    assert!(result.is_err());

    filter_slot = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::SlotRange(SlotRange {
                start_slot: None,
                end_slot: Some(ProtoSlot {
                    period: 1,
                    thread: 15,
                }),
            }),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_slot],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_block.is_some());

    let mut filter_addr = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::Addresses(Addresses {
                addresses: vec!["AU12BTfZ7k1z6PsLEUZeHYNirz6WJ3NdrWto9H4TkVpkV9xE2TJg2".to_string()],
            }),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_addr],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    // elapsed
    assert!(result.is_err());

    filter_addr = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::Addresses(Addresses {
                addresses: vec![address.to_string()],
            }),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_addr.clone()],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_block.is_some());

    let mut filter_ids = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![
                        "B1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    // elapsed
    assert!(result.is_err());

    filter_ids = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![block_op.id.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // send block
    block_tx.send(block_op.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_block.is_some());

    filter_addr = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::Addresses(Addresses {
                addresses: vec!["massa".to_string()],
            }),
        ),
    };

    tx_request
        .send(NewBlocksRequest {
            filters: vec![filter_addr],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let result = tokio::time::timeout(Duration::from_secs(3), resp_stream.next())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.unwrap_err().message(), "invalid address: massa");

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn new_endorsements() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();

    let (endorsement_tx, _endorsement_rx) = tokio::sync::broadcast::channel(10);

    public_server.pool_channels.endorsement_sender = endorsement_tx.clone();

    let stop_handle = public_server.serve(&config).await.unwrap();

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    let endorsement = create_endorsement();

    let (tx_request, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let mut resp_stream = public_client
        .new_endorsements(request_stream)
        .await
        .unwrap()
        .into_inner();

    let mut filter_ids = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::EndorsementIds(
                massa_proto_rs::massa::model::v1::EndorsementIds {
                    endorsement_ids: vec![
                        "E1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter_ids = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::EndorsementIds(
                massa_proto_rs::massa::model::v1::EndorsementIds {
                    endorsement_ids: vec![endorsement.id.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_endorsement.is_some());

    let mut filter_addr = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::Addresses(
                massa_proto_rs::massa::model::v1::Addresses {
                    addresses: vec![
                        "AU12BTfZ7k1z6PsLEUZeHYNirz6WJ3NdrWto9H4TkVpkV9xE2TJg2".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_addr],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter_addr = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::Addresses(
                massa_proto_rs::massa::model::v1::Addresses {
                    addresses: vec![endorsement.content_creator_address.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_addr],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_endorsement.is_some());

    let mut filter_block_ids = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![
                        "B1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_block_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter_block_ids = massa_proto_rs::massa::api::v1::NewEndorsementsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_endorsements_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![endorsement.content.endorsed_block.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(massa_proto_rs::massa::api::v1::NewEndorsementsRequest {
            filters: vec![filter_block_ids],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    endorsement_tx.send(endorsement.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.signed_endorsement.is_some());

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn new_filled_blocks() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();

    let (filled_block_tx, _filled_block_rx) = tokio::sync::broadcast::channel(10);

    public_server.consensus_channels.filled_block_sender = filled_block_tx.clone();

    let stop_handle = public_server.serve(&config).await.unwrap();

    let (tx_request, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let keypair = KeyPair::generate(0).unwrap();
    let address = Address::from_public_key(&keypair.get_public_key());
    let block = create_block(&keypair);

    let filled_block = FilledBlock {
        header: block.content.header.clone(),
        operations: vec![],
    };

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    let mut resp_stream = public_client
        .new_filled_blocks(request_stream)
        .await
        .unwrap()
        .into_inner();

    let mut filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::SlotRange(SlotRange {
                start_slot: Some(ProtoSlot {
                    period: 1,
                    thread: 0,
                }),
                end_slot: None,
            }),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.filled_block.is_some());

    filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::SlotRange(SlotRange {
                start_slot: Some(ProtoSlot {
                    period: 1,
                    thread: 5,
                }),
                end_slot: None,
            }),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![
                        "B1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC".to_string()
                    ],
                },
            ),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::BlockIds(
                massa_proto_rs::massa::model::v1::BlockIds {
                    block_ids: vec![filled_block.header.id.to_string()],
                },
            ),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.filled_block.is_some());

    filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::Addresses(Addresses {
                addresses: vec!["AU12BTfZ7k1z6PsLEUZeHYNirz6WJ3NdrWto9H4TkVpkV9xE2TJg2".to_string()],
            }),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    filter = massa_proto_rs::massa::api::v1::NewBlocksFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_blocks_filter::Filter::Addresses(Addresses {
                addresses: vec![address.to_string()],
            }),
        ),
    };

    tx_request
        .send(NewFilledBlocksRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    filled_block_tx.send(filled_block.clone()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.filled_block.is_some());

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn new_slot_execution_outputs() {
    let mut public_server = grpc_public_service();
    let config = public_server.grpc_config.clone();

    let (slot_tx, _slot_rx) = tokio::sync::broadcast::channel(10);

    public_server
        .execution_channels
        .slot_execution_output_sender = slot_tx.clone();

    let stop_handle = public_server.serve(&config).await.unwrap();

    let exec_output_1 = ExecutionOutput {
        slot: Slot::new(1, 5),
        block_info: None,
        state_changes: massa_final_state::StateChanges::default(),
        events: Default::default(),
    };

    let (tx_request, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let keypair = KeyPair::generate(0).unwrap();
    let address = Address::from_public_key(&keypair.get_public_key());

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    let mut resp_stream = public_client
        .new_slot_execution_outputs(request_stream)
        .await
        .unwrap()
        .into_inner();

    let mut filter = massa_proto_rs::massa::api::v1::NewSlotExecutionOutputsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_slot_execution_outputs_filter::Filter::SlotRange(
                SlotRange {
                    start_slot: Some(ProtoSlot {
                        period: 1,
                        thread: 0,
                    }),
                    end_slot: None,
                },
            ),
        ),
    };

    tx_request
        .send(NewSlotExecutionOutputsRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    slot_tx
        .send(SlotExecutionOutput::ExecutedSlot(exec_output_1.clone()))
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.output.is_some());

    filter = massa_proto_rs::massa::api::v1::NewSlotExecutionOutputsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_slot_execution_outputs_filter::Filter::SlotRange(
                SlotRange {
                    start_slot: Some(ProtoSlot {
                        period: 1,
                        thread: 0,
                    }),
                    end_slot: Some(ProtoSlot {
                        period: 1,
                        thread: 7,
                    }),
                },
            ),
        ),
    };

    tx_request
        .send(NewSlotExecutionOutputsRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    slot_tx
        .send(SlotExecutionOutput::ExecutedSlot(exec_output_1.clone()))
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert!(result.output.is_some());

    filter = massa_proto_rs::massa::api::v1::NewSlotExecutionOutputsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_slot_execution_outputs_filter::Filter::SlotRange(
                SlotRange {
                    start_slot: Some(ProtoSlot {
                        period: 1,
                        thread: 7,
                    }),
                    end_slot: None,
                },
            ),
        ),
    };

    tx_request
        .send(NewSlotExecutionOutputsRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    slot_tx
        .send(SlotExecutionOutput::ExecutedSlot(exec_output_1.clone()))
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    assert!(result.is_err());

    // TODO add test when filter is updated

    /*     filter = massa_proto_rs::massa::api::v1::NewSlotExecutionOutputsFilter {
        filter: Some(
            massa_proto_rs::massa::api::v1::new_slot_execution_outputs_filter::Filter::EventFilter(
                massa_proto_rs::massa::api::v1::ExecutionEventFilter {
                    filter: Some(
                        massa_proto_rs::massa::api::v1::execution_event_filter::Filter::OriginalOperationId( "O1q4CBcuYo8YANEV34W4JRWVHrzcYns19VJfyAB7jT4qfitAnMC"
                                    .to_string()
                        ),
                    ),
                },
            ),
        ),
    };

    tx_request
        .send(NewSlotExecutionOutputsRequest {
            filters: vec![filter],
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    slot_tx
        .send(SlotExecutionOutput::ExecutedSlot(exec_output_1.clone()))
        .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), resp_stream.next()).await;
    dbg!(&result);
    assert!(result.is_err()); */

    stop_handle.stop();
}

#[tokio::test]
#[serial]
async fn send_operations() {
    let mut public_server = grpc_public_service();

    let mut pool_ctrl = MockPoolCtrl::new();
    pool_ctrl.expect_clone_box().returning(|| {
        let mut ctrl = MockPoolCtrl::new();

        ctrl.expect_add_operations().returning(|_| ());

        Box::new(ctrl)
    });

    let mut protocol_ctrl = MockProtocolController::new();
    protocol_ctrl.expect_clone_box().returning(|| {
        let mut ctrl = MockProtocolController::new();

        ctrl.expect_propagate_operations().returning(|_| Ok(()));

        Box::new(ctrl)
    });

    public_server.pool_controller = Box::new(pool_ctrl);
    public_server.protocol_controller = Box::new(protocol_ctrl);

    let config = public_server.grpc_config.clone();

    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    let stop_handle = public_server.serve(&config).await.unwrap();

    let mut public_client = PublicServiceClient::connect(GRPC_SERVER_URL).await.unwrap();

    let mut resp_stream = public_client
        .send_operations(request_stream)
        .await
        .unwrap()
        .into_inner();

    let keypair = KeyPair::generate(0).unwrap();
    let op = create_operation_with_expire_period(&keypair, 4);

    tx.send(SendOperationsRequest {
        operations: vec![op.clone().serialized_data],
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    match result.result.unwrap() {
        massa_proto_rs::massa::api::v1::send_operations_response::Result::OperationIds(_) => {
            panic!("should be error");
        }
        massa_proto_rs::massa::api::v1::send_operations_response::Result::Error(err) => {
            assert!(err.message.contains("invalid operation"));
        }
    }

    let mut buffer: Vec<u8> = Vec::new();
    SecureShareSerializer::new()
        .serialize(&op, &mut buffer)
        .unwrap();

    tx.send(SendOperationsRequest {
        operations: vec![buffer],
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    match result.result.unwrap() {
        massa_proto_rs::massa::api::v1::send_operations_response::Result::Error(err) => {
            assert!(err
                .message
                .contains("Operation expire_period is lower than the current period of this node"));
        }
        _ => {
            panic!("should be error");
        }
    }

    let op2 = create_operation_with_expire_period(&keypair, 150000);
    let mut buffer: Vec<u8> = Vec::new();
    SecureShareSerializer::new()
        .serialize(&op2, &mut buffer)
        .unwrap();

    tx.send(SendOperationsRequest {
        operations: vec![buffer.clone()],
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .result
        .unwrap();

    match result {
        massa_proto_rs::massa::api::v1::send_operations_response::Result::OperationIds(ope_id) => {
            assert_eq!(ope_id.operation_ids.len(), 1);
            assert_eq!(ope_id.operation_ids[0], op2.id.to_string());
        }
        massa_proto_rs::massa::api::v1::send_operations_response::Result::Error(_) => {
            panic!("should be ok")
        }
    }

    tx.send(SendOperationsRequest {
        operations: vec![buffer.clone(), buffer.clone(), buffer.clone()],
    })
    .await
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), resp_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    match result.result.unwrap() {
        massa_proto_rs::massa::api::v1::send_operations_response::Result::Error(err) => {
            assert_eq!(err.message, "too many operations per message");
        }
        _ => {
            panic!("should be error");
        }
    }

    stop_handle.stop();
}

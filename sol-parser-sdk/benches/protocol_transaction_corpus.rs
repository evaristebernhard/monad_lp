use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use sol_parser_sdk::grpc::parse_subscribe_update_transaction_low_latency;
use sol_parser_sdk::{convert_rpc_to_grpc, parse_rpc_transaction, parse_rpc_transaction_cost};
use solana_transaction_status::{
    option_serializer::OptionSerializer, EncodedConfirmedTransactionWithStatusMeta, UiInstruction,
};
use std::hint::black_box;
use yellowstone_grpc_proto::prelude::{SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo};

const FIXTURES: &[(&str, &str)] = &[
    ("pumpfun", include_str!("../tests/fixtures/pumpfun_rpc_transaction.json")),
    ("pumpswap", include_str!("../tests/fixtures/pumpswap_rpc_transaction.json")),
    ("raydium_cpmm", include_str!("../tests/fixtures/raydium_cpmm_rpc_transaction.json")),
    ("meteora_dlmm_orca", include_str!("../tests/fixtures/meteora_dlmm_orca_rpc_transaction.json")),
];

fn normalized_yellowstone(
    rpc: &EncodedConfirmedTransactionWithStatusMeta,
) -> SubscribeUpdateTransaction {
    let (meta, transaction) = convert_rpc_to_grpc(rpc).expect("convert RPC fixture");
    let signature = transaction.signatures.first().cloned().expect("transaction signature");
    SubscribeUpdateTransaction {
        transaction: Some(SubscribeUpdateTransactionInfo {
            signature,
            is_vote: false,
            transaction: Some(transaction),
            meta: Some(meta),
            index: u64::from(rpc.transaction_index.unwrap_or_default()),
        }),
        slot: rpc.slot,
    }
}

fn benchmark(c: &mut Criterion) {
    let fixtures: Vec<_> = FIXTURES
        .iter()
        .map(|(name, json)| {
            let rpc: EncodedConfirmedTransactionWithStatusMeta =
                serde_json::from_str(json).expect("valid RPC fixture");
            let yellowstone = normalized_yellowstone(&rpc);
            (*name, rpc, yellowstone)
        })
        .collect();

    let mut rpc_group = c.benchmark_group("protocol_corpus/rpc_end_to_end");
    for (name, rpc, _) in &fixtures {
        rpc_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            b.iter(|| {
                black_box(parse_rpc_transaction(black_box(rpc), None).expect("parse RPC fixture"));
            });
        });
    }
    rpc_group.finish();

    let mut cost_group = c.benchmark_group("protocol_corpus/rpc_cost");
    for (name, rpc, _) in &fixtures {
        cost_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            b.iter(|| {
                black_box(parse_rpc_transaction_cost(black_box(rpc)).expect("parse RPC cost"));
            });
        });
    }
    cost_group.finish();

    let mut conversion_group = c.benchmark_group("protocol_corpus/rpc_conversion");
    for (name, rpc, _) in &fixtures {
        conversion_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            b.iter(|| {
                black_box(convert_rpc_to_grpc(black_box(rpc)).expect("convert RPC fixture"));
            });
        });
    }
    conversion_group.finish();

    let mut binary_group = c.benchmark_group("protocol_corpus/rpc_binary_decode");
    for (name, rpc, _) in &fixtures {
        binary_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            b.iter(|| {
                black_box(
                    rpc.transaction.transaction.decode().expect("decode RPC transaction binary"),
                );
            });
        });
    }
    binary_group.finish();

    let mut inner_data_group = c.benchmark_group("protocol_corpus/rpc_inner_base58_decode");
    for (name, rpc, _) in &fixtures {
        inner_data_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            let meta = rpc.transaction.meta.as_ref().expect("transaction metadata");
            let inner = match &meta.inner_instructions {
                OptionSerializer::Some(inner) => inner.as_slice(),
                _ => &[],
            };
            b.iter(|| {
                let mut decoded_bytes = 0usize;
                for instruction in inner.iter().flat_map(|group| group.instructions.iter()) {
                    if let UiInstruction::Compiled(instruction) = instruction {
                        decoded_bytes += bs58::decode(black_box(&instruction.data))
                            .into_vec()
                            .expect("decode inner instruction")
                            .len();
                    }
                }
                black_box(decoded_bytes);
            });
        });
    }
    inner_data_group.finish();

    let mut turbo_group = c.benchmark_group("protocol_corpus/rpc_inner_base58_turbo_decode");
    for (name, rpc, _) in &fixtures {
        turbo_group.bench_with_input(BenchmarkId::from_parameter(name), rpc, |b, rpc| {
            let meta = rpc.transaction.meta.as_ref().expect("transaction metadata");
            let inner = match &meta.inner_instructions {
                OptionSerializer::Some(inner) => inner.as_slice(),
                _ => &[],
            };
            b.iter(|| {
                let mut decoded_bytes = 0usize;
                for instruction in inner.iter().flat_map(|group| group.instructions.iter()) {
                    if let UiInstruction::Compiled(instruction) = instruction {
                        decoded_bytes += base58_turbo::BITCOIN
                            .decode(black_box(&instruction.data))
                            .expect("decode inner instruction")
                            .len();
                    }
                }
                black_box(decoded_bytes);
            });
        });
    }
    turbo_group.finish();

    let mut yellowstone_group = c.benchmark_group("protocol_corpus/yellowstone_normalized");
    for (name, _, yellowstone) in &fixtures {
        yellowstone_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            yellowstone,
            |b, yellowstone| {
                b.iter(|| {
                    black_box(parse_subscribe_update_transaction_low_latency(
                        black_box(yellowstone),
                        0,
                        None,
                        None,
                    ));
                });
            },
        );
    }
    yellowstone_group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);

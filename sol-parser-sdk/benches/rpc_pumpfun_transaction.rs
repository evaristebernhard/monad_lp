use criterion::{criterion_group, criterion_main, Criterion};
use sol_parser_sdk::{grpc::EventTypeFilter, parse_rpc_transaction};
use solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta;
use std::hint::black_box;

const FIXTURE: &str = include_str!("../tests/fixtures/pumpfun_rpc_transaction.json");

fn benchmark(c: &mut Criterion) {
    let transaction: EncodedConfirmedTransactionWithStatusMeta =
        serde_json::from_str(FIXTURE).expect("valid RPC transaction fixture");

    c.bench_function("rpc_transaction/pumpfun_mainnet", |b| {
        b.iter(|| {
            black_box(
                parse_rpc_transaction(black_box(&transaction), Option::<&EventTypeFilter>::None)
                    .expect("parse PumpFun RPC fixture"),
            );
        })
    });
}

criterion_group!(benches, benchmark);
criterion_main!(benches);

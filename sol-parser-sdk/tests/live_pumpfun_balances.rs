use sol_parser_sdk::grpc::{
    ClientConfig, EventType, EventTypeFilter, OrderMode, Protocol, TransactionFilter,
    YellowstoneGrpc,
};
use sol_parser_sdk::DexEvent;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_pumpfun_trade_has_consistent_balances() {
    if std::env::var("RUN_LIVE_GRPC_TEST").as_deref() != Ok("1") {
        return;
    }

    let endpoint = std::env::var("GRPC_URL").expect("GRPC_URL must be set");
    let token = std::env::var("GRPC_TOKEN").expect("GRPC_TOKEN must be set");
    let config = ClientConfig {
        enable_metrics: false,
        connection_timeout_ms: 10_000,
        request_timeout_ms: 30_000,
        enable_tls: endpoint.starts_with("https://"),
        order_mode: OrderMode::Unordered,
        ..Default::default()
    };
    let grpc = YellowstoneGrpc::new_with_config(endpoint, Some(token), config)
        .expect("gRPC client should be created");
    let protocols = vec![Protocol::PumpFun];
    let event_filter = EventTypeFilter::include_only(vec![
        EventType::PumpFunBuy,
        EventType::PumpFunSell,
        EventType::PumpFunBuyExactSolIn,
    ]);
    let queue = grpc
        .subscribe_dex_events(
            vec![TransactionFilter::for_protocols(&protocols)],
            Vec::new(),
            Some(event_filter),
        )
        .await
        .expect("PumpFun subscription should start");

    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(event) = queue.pop() {
            let trade = match &event {
                DexEvent::PumpFunBuy(trade)
                | DexEvent::PumpFunSell(trade)
                | DexEvent::PumpFunBuyExactSolIn(trade) => trade,
                _ => continue,
            };
            let token_balance = trade.token_balance.expect("final token balance should be present");
            let sol_balance = trade.sol_balance.expect("final SOL balance should be present");
            println!(
                "verified {} {}: user {}, quote {}, event amount {}, final token balance {}, final SOL balance {} lamports",
                trade.metadata.signature,
                if trade.is_buy { "buy" } else { "sell" },
                trade.user,
                trade.quote_mint,
                trade.token_amount,
                token_balance,
                sol_balance
            );
            return;
        }

        assert!(Instant::now() < deadline, "timed out waiting for a PumpFun trade");
        tokio::task::yield_now().await;
    }
}

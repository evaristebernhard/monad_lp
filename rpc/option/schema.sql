CREATE TABLE IF NOT EXISTS option_snapshots (
    id INTEGER PRIMARY KEY,
    venue TEXT NOT NULL,
    underlying TEXT NOT NULL,
    tenor TEXT NOT NULL,
    generated_at INTEGER NOT NULL,
    expiry_timestamp INTEGER,
    days_to_expiry REAL,
    index_price REAL,
    status TEXT NOT NULL,
    UNIQUE(venue, underlying, tenor, generated_at)
);

CREATE INDEX IF NOT EXISTS idx_option_snapshots_latest
    ON option_snapshots(venue, underlying, tenor, generated_at DESC);

CREATE TABLE IF NOT EXISTS option_quotes (
    snapshot_id INTEGER NOT NULL REFERENCES option_snapshots(id) ON DELETE CASCADE,
    instrument_name TEXT NOT NULL,
    option_type TEXT,
    strike REAL,
    expiration_timestamp INTEGER,
    contract_size REAL,
    settlement_currency TEXT,
    best_bid_usd REAL,
    best_ask_usd REAL,
    mark_price_usd REAL,
    mark_iv REAL,
    open_interest REAL,
    raw_json TEXT,
    PRIMARY KEY(snapshot_id, instrument_name)
);

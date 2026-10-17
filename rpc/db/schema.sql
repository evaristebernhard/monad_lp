PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings(key, value) VALUES
    ('listen', '127.0.0.1:8545'),
    ('request_timeout_ms', '8000'),
    ('max_request_bytes', '10485760');

CREATE TABLE IF NOT EXISTS chains (
    id INTEGER PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    network_slug TEXT NOT NULL,
    protocol TEXT NOT NULL CHECK (protocol IN ('evm', 'solana')),
    chain_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);

CREATE TABLE IF NOT EXISTS chain_aliases (
    alias TEXT PRIMARY KEY COLLATE NOCASE,
    chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS rpc_endpoints (
    id INTEGER PRIMARY KEY,
    chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    provider TEXT NOT NULL,
    tier TEXT NOT NULL CHECK (tier IN ('free', 'paid')),
    priority INTEGER NOT NULL DEFAULT 100,
    max_concurrency INTEGER NOT NULL DEFAULT 32 CHECK (max_concurrency > 0),
    url TEXT NOT NULL,
    supports_historical INTEGER CHECK (supports_historical IN (0, 1)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    UNIQUE(chain_id, title)
);

CREATE TABLE IF NOT EXISTS rpc_benchmarks (
    id INTEGER PRIMARY KEY,
    endpoint_id INTEGER NOT NULL REFERENCES rpc_endpoints(id) ON DELETE CASCADE,
    measured_at INTEGER NOT NULL,
    method TEXT NOT NULL,
    target_rps INTEGER NOT NULL,
    success_rate REAL NOT NULL,
    p50_ms REAL,
    p95_ms REAL,
    sustainable INTEGER NOT NULL DEFAULT 0 CHECK (sustainable IN (0, 1)),
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_chain_aliases_chain_id ON chain_aliases(chain_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_chains_network_slug ON chains(network_slug);
CREATE INDEX IF NOT EXISTS idx_rpc_endpoints_chain ON rpc_endpoints(chain_id, enabled, tier, priority);
CREATE INDEX IF NOT EXISTS idx_rpc_benchmarks_latest ON rpc_benchmarks(endpoint_id, measured_at DESC, id DESC);

-- Paid endpoints are always a fallback tier. Keep their numeric priority in a
-- visibly lower-priority range as well, so SQL clients and admin tools cannot
-- accidentally present a paid endpoint above a free endpoint.
CREATE TRIGGER IF NOT EXISTS rpc_paid_priority_guard_insert
BEFORE INSERT ON rpc_endpoints
WHEN NEW.tier = 'paid' AND NEW.priority < 1000
BEGIN
    SELECT RAISE(ABORT, 'paid RPC priority must be >= 1000');
END;

CREATE TRIGGER IF NOT EXISTS rpc_paid_priority_guard_update
BEFORE UPDATE OF tier, priority ON rpc_endpoints
WHEN NEW.tier = 'paid' AND NEW.priority < 1000
BEGIN
    SELECT RAISE(ABORT, 'paid RPC priority must be >= 1000');
END;

-- Human-friendly lookup names. This lets the SQL client search by canonical
-- slug, network slug, or any alias without knowing which column owns it.
CREATE VIEW IF NOT EXISTS chain_names AS
SELECT slug AS chain_name, id AS chain_row_id FROM chains
UNION
SELECT network_slug AS chain_name, id AS chain_row_id FROM chains
UNION
SELECT alias AS chain_name, chain_id AS chain_row_id FROM chain_aliases;

DROP VIEW IF EXISTS chain_rpc_catalog;
CREATE VIEW chain_rpc_catalog AS
SELECT
    n.chain_name,
    c.slug,
    c.network_slug,
    c.protocol,
    c.chain_id,
    e.id AS endpoint_id,
    e.title,
    e.provider,
    e.tier,
    e.priority,
    e.max_concurrency,
    e.url,
    e.enabled
FROM chain_names n
JOIN chains c ON c.id = n.chain_row_id
JOIN rpc_endpoints e ON e.chain_id = c.id
WHERE c.enabled = 1;

CREATE VIEW IF NOT EXISTS active_chain_rpc_catalog AS
SELECT *
FROM chain_rpc_catalog
WHERE enabled = 1;

DROP VIEW IF EXISTS chain_active_rpc_summary;
CREATE VIEW chain_active_rpc_summary AS
SELECT
    c.slug,
    c.network_slug,
    c.protocol,
    c.chain_id,
    (
        SELECT COUNT(*)
        FROM rpc_endpoints e
        WHERE e.chain_id = c.id AND e.enabled = 1
    ) AS active_rpc_count,
    COALESCE(
        (
            SELECT GROUP_CONCAT(line, char(10))
            FROM (
                SELECT
                    e.title || ' | ' || e.provider || ' | ' || e.tier ||
                    ' | priority=' || e.priority || ' | ' || e.url AS line
                FROM rpc_endpoints e
                WHERE e.chain_id = c.id AND e.enabled = 1
                ORDER BY CASE e.tier WHEN 'free' THEN 0 ELSE 1 END,
                         e.priority, e.title
            )
        ),
        '[no enabled RPC]'
    ) AS active_rpcs
FROM chains c
WHERE c.enabled = 1;

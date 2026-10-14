-- Vigil schema. Shared by the Indexer (cursor, per-block logs), the Risk Engine (positions,
-- processed events, reverse deltas, intents) and the Liquidator (transaction tracking).

-- Indexer cursor: the last block whose events were published (RS-03).
CREATE TABLE IF NOT EXISTS indexer_cursor (
    id           SMALLINT PRIMARY KEY CHECK (id = 1),
    block_number BIGINT NOT NULL,
    block_hash   TEXT NOT NULL,
    parent_hash  TEXT NOT NULL
);

-- Messages published per recent block, kept so a reorg after a restart can still retract them.
CREATE TABLE IF NOT EXISTS indexer_block_logs (
    block_number BIGINT NOT NULL,
    block_hash   TEXT NOT NULL,
    log_index    BIGINT NOT NULL,
    message      JSONB NOT NULL,
    PRIMARY KEY (block_hash, log_index)
);
CREATE INDEX IF NOT EXISTS indexer_block_logs_number ON indexer_block_logs (block_number);

-- Exactly-once processing (RS-05). The key is inserted in the same transaction as the state change.
CREATE TABLE IF NOT EXISTS processed_events (
    block_hash TEXT NOT NULL,
    log_index  BIGINT NOT NULL,
    PRIMARY KEY (block_hash, log_index)
);

-- Position state. Amounts are decimal strings, because uint256 does not fit in SQL integers.
CREATE TABLE IF NOT EXISTS positions (
    borrower     TEXT PRIMARY KEY,
    bitmap       SMALLINT NOT NULL DEFAULT 0,
    debt_scaled  TEXT NOT NULL DEFAULT '0',
    amount_0     TEXT NOT NULL DEFAULT '0',
    amount_1     TEXT NOT NULL DEFAULT '0',
    amount_2     TEXT NOT NULL DEFAULT '0'
);

-- Reverse deltas (RS-06): the position as it was before each applied event, undone in reverse
-- order when that event is retracted.
CREATE TABLE IF NOT EXISTS applied_events (
    block_hash TEXT NOT NULL,
    log_index  BIGINT NOT NULL,
    borrower   TEXT NOT NULL,
    before     JSONB,
    PRIMARY KEY (block_hash, log_index)
);

-- Pre-signed borrower intents (SC-20). Borrowers sign at position setup. The Liquidator only
-- ever reads them.
CREATE TABLE IF NOT EXISTS intents (
    borrower           TEXT NOT NULL,
    nonce              TEXT NOT NULL,
    collateral_asset   TEXT NOT NULL,
    max_repay          TEXT NOT NULL,
    min_collateral_out TEXT NOT NULL,
    deadline           BIGINT NOT NULL,
    signature          TEXT NOT NULL,
    PRIMARY KEY (borrower, nonce)
);

-- Liquidator transactions (RS-13 to RS-15). Survives restarts, so reconciliation has a record.
CREATE TABLE IF NOT EXISTS liquidator_txs (
    nonce        BIGINT PRIMARY KEY,
    kind         TEXT NOT NULL,
    borrower     TEXT NOT NULL,
    tx_hash      TEXT NOT NULL,
    max_fee_wei  TEXT NOT NULL,
    priority_wei TEXT NOT NULL,
    sent_block   BIGINT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'replaced', 'reorged', 'gap_filled', 'failed')),
    trace_id     TEXT NOT NULL
);

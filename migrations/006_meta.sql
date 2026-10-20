-- Per-data-dir facts. The same shape as the TUI branch's meta table, so either migration may create it first.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- §11: one lease owner per data dir, so a restarted process reclaims the leases it left behind at once.
INSERT OR IGNORE INTO meta (key, value) VALUES ('lease_owner', 'pr-review-' || lower(hex(randomblob(16))));

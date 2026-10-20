-- §9.4 per-delivery backoff: a delivery that keeps failing waits its turn, then goes dead with its reason.
ALTER TABLE webhook_events ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE webhook_events ADD COLUMN next_attempt_at INTEGER;
ALTER TABLE webhook_events ADD COLUMN last_error TEXT;
DROP INDEX webhook_events_unprocessed;
CREATE INDEX webhook_events_due ON webhook_events (processed_at, next_attempt_at, received_at);

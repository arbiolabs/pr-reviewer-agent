-- TUI M3: guarded commands over the control socket. A command's idempotency key is recorded with its accepted result,
-- so a replay answers the same operation and has no second effect. A cancel from the TUI records its reason before the
-- durable task is aborted, since the abort itself carries none.
CREATE TABLE control_commands (
  idempotency_key TEXT PRIMARY KEY,
  command TEXT NOT NULL,
  result_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
ALTER TABLE review_revisions ADD COLUMN cancel_reason TEXT;

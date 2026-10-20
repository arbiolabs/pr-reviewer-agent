-- §9.3 outbox bookkeeping and its own audit trail; the review machine's audit stays in audit_events.
ALTER TABLE publication_outbox ADD COLUMN last_error TEXT;
CREATE INDEX publication_outbox_status ON publication_outbox (status);

CREATE TABLE outbox_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  outbox_id TEXT NOT NULL REFERENCES publication_outbox(id),
  from_status TEXT NOT NULL,
  event TEXT NOT NULL,
  to_status TEXT NOT NULL,
  detail TEXT,
  at TEXT NOT NULL
);
CREATE INDEX outbox_events_outbox ON outbox_events (outbox_id, seq);

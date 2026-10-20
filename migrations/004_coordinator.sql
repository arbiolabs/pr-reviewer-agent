-- §9.2 inbox payloads, §11 leases and supersede requests, and the debounce deadline per PR.
ALTER TABLE webhook_events ADD COLUMN payload_json TEXT;
ALTER TABLE webhook_events ADD COLUMN outcome TEXT;
CREATE INDEX webhook_events_unprocessed ON webhook_events (processed_at, received_at);

ALTER TABLE pull_requests ADD COLUMN review_due_at INTEGER;
ALTER TABLE review_revisions ADD COLUMN superseded_requested_at TEXT;

CREATE TABLE leases (
  pr_id TEXT PRIMARY KEY REFERENCES pull_requests(id),
  owner TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

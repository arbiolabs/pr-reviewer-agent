-- Journal rows that belong to no revision (account selection) need a nullable revision_id. SQLite cannot drop NOT NULL,
-- so the table is rebuilt. legacy_alter_table keeps the rename from re-checking 050's triggers, which name audit_events
-- and resolve to the rebuilt table once it has that name.
CREATE TABLE audit_events_new (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  revision_id TEXT REFERENCES review_revisions(id),
  from_state TEXT NOT NULL,
  event TEXT NOT NULL,
  to_state TEXT NOT NULL,
  detail TEXT,
  at TEXT NOT NULL
);
INSERT INTO audit_events_new (seq, revision_id, from_state, event, to_state, detail, at)
  SELECT seq, revision_id, from_state, event, to_state, detail, at FROM audit_events;
PRAGMA legacy_alter_table = ON;
DROP TABLE audit_events;
ALTER TABLE audit_events_new RENAME TO audit_events;
PRAGMA legacy_alter_table = OFF;
CREATE INDEX audit_events_revision ON audit_events (revision_id, seq);

-- Control stream (TUI protocol v1). audit_events is the journal: its seq is the stream sequence. These triggers journal
-- the writes no review transition carries, in the transaction that makes them. Journal-only rows keep
-- from_state = to_state, and AppStore.auditEvents leaves them out.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT OR IGNORE INTO meta (key, value) VALUES ('stream_epoch', lower(hex(randomblob(16))));

CREATE TRIGGER journal_revision_created AFTER INSERT ON review_revisions BEGIN
  INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (NEW.id, NEW.state, 'PR_UPDATED', NEW.state, NULL, NEW.created_at);
  INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at) VALUES (NEW.id, NEW.state, 'REVISION_CREATED', NEW.state, NULL, NEW.created_at);
END;

CREATE TRIGGER journal_finding_observed AFTER INSERT ON finding_observations BEGIN
  INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at)
    SELECT id, state, 'FINDING_OBSERVED', state, NEW.finding_id, NEW.created_at FROM review_revisions WHERE id = NEW.revision_id;
END;

CREATE TRIGGER journal_finding_updated AFTER UPDATE OF status ON findings WHEN NEW.status IS NOT OLD.status BEGIN
  INSERT INTO audit_events (revision_id, from_state, event, to_state, detail, at)
    SELECT id, state, 'FINDING_UPDATED', state, NEW.id, NEW.updated_at FROM review_revisions
    WHERE id = coalesce((SELECT revision_id FROM finding_observations WHERE finding_id = NEW.id ORDER BY created_at DESC LIMIT 1), NEW.first_revision_id);
END;

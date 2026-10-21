-- Finding tracking across heads (docs/design/finding-tracking.md). An observation records the finding state it ended in,
-- the states it walked, and why; a finding records the GitHub review that first carried its inline comment, so the
-- column, not the status, says whether it is on GitHub; an outbox records which findings its payload comments on.
ALTER TABLE finding_observations ADD COLUMN path_json TEXT;
ALTER TABLE finding_observations ADD COLUMN basis TEXT;
ALTER TABLE findings ADD COLUMN github_review_id INTEGER;
ALTER TABLE publication_outbox ADD COLUMN inline_finding_ids_json TEXT;

-- A finding published before this migration was posted by the review of the revision that observed it.
UPDATE findings SET github_review_id = (
  SELECT o.github_review_id FROM publication_outbox o
  JOIN finding_observations fo ON fo.revision_id = o.revision_id
  WHERE fo.finding_id = findings.id AND o.status = 'published' ORDER BY o.updated_at LIMIT 1
) WHERE status != 'DETECTED' AND github_review_id IS NULL;

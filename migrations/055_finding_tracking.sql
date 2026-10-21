-- Finding tracking across heads (docs/design/finding-tracking.md). An observation records the finding state it ended in,
-- the states it walked, and why; a finding records the GitHub review that first carried its inline comment, so the
-- column, not the status, says whether it is on GitHub; an outbox records which findings its payload comments on.
ALTER TABLE finding_observations ADD COLUMN path_json TEXT;
ALTER TABLE finding_observations ADD COLUMN basis TEXT;
ALTER TABLE findings ADD COLUMN github_review_id INTEGER;
ALTER TABLE publication_outbox ADD COLUMN inline_finding_ids_json TEXT;

-- A finding commented on before this migration was posted by the review of the revision that observed it, at the file
-- and line one of that payload's comments names. A finding the review listed without a comment stays unposted.
UPDATE findings SET github_review_id = (
  SELECT o.github_review_id FROM publication_outbox o
  JOIN finding_observations fo ON fo.revision_id = o.revision_id
  WHERE fo.finding_id = findings.id AND o.status = 'published'
    AND EXISTS (
      SELECT 1 FROM json_each(o.payload_json, '$.comments') c
      WHERE json_extract(c.value, '$.path') = json_extract(fo.location_json, '$.file')
        AND json_extract(c.value, '$.line') = json_extract(fo.location_json, '$.newLine')
    )
  ORDER BY o.updated_at LIMIT 1
) WHERE github_review_id IS NULL;

-- Checked resolution (docs/design/finding-tracking.md, build step 4). A submission carries the model's resolution
-- claims next to its candidates, and VERIFY records a verdict per claim next to the candidate dispositions.
ALTER TABLE candidate_sets ADD COLUMN resolutions_json TEXT;
ALTER TABLE candidate_sets ADD COLUMN resolution_verdicts_json TEXT;

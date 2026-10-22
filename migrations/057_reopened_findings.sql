-- Self-correcting resolution (docs/design/finding-tracking.md). A resolved finding stays watched for the life of the
-- PR; when its code is found at a later head, a new finding is seeded from it and records which resolved finding it
-- reopens. A resolved finding with a successor is watched no more, so each is reopened at most once.
ALTER TABLE findings ADD COLUMN reopens TEXT REFERENCES findings(id);

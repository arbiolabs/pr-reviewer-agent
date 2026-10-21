-- The PR's title as GitHub last reported it, so a PR reads by name before any review has collected its context.
ALTER TABLE pull_requests ADD COLUMN title TEXT;

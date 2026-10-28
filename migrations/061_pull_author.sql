-- The PR author's login as GitHub last reported it, refreshed on every read like the title; NULL until the next read.
ALTER TABLE pull_requests ADD COLUMN author TEXT;

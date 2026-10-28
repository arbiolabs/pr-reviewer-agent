-- A reader preference, not a review state: an archived PR leaves the clients' lists until it is unarchived or a review
-- is started on it. NULL means not archived.
ALTER TABLE pull_requests ADD COLUMN archived_at TEXT;

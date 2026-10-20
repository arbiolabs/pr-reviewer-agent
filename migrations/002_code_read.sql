-- Set by the first successful read_diff, read_file or search_code of a revision; an empty submission needs it (§2 invariant 9).
ALTER TABLE review_revisions ADD COLUMN code_read_at TEXT;

-- One row per model response of a review, with the tokens and the cost pi-ai reported at the model's catalog price.
-- Written from the reviewer's afterResponse hook, which runs once per live response: a resumed task never replays one.
-- Reviews before this migration have no rows; their activity lines held only output tokens.
CREATE TABLE model_usage (
  id INTEGER PRIMARY KEY,
  revision_id TEXT NOT NULL,
  pr_id TEXT NOT NULL,
  at TEXT NOT NULL,
  model TEXT NOT NULL,
  input INTEGER NOT NULL,
  output INTEGER NOT NULL,
  cache_read INTEGER NOT NULL,
  cache_write INTEGER NOT NULL,
  cost_input REAL NOT NULL,
  cost_output REAL NOT NULL,
  cost_cache_read REAL NOT NULL,
  cost_cache_write REAL NOT NULL,
  cost_total REAL NOT NULL
);
CREATE INDEX model_usage_at ON model_usage (at);

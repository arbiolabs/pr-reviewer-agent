CREATE TABLE pull_requests (
  id TEXT PRIMARY KEY,
  installation_id INTEGER NOT NULL,
  repository_id INTEGER NOT NULL,
  number INTEGER NOT NULL,
  status TEXT NOT NULL,
  latest_base_sha TEXT NOT NULL,
  latest_head_sha TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  repo_full_name TEXT NOT NULL,
  UNIQUE (installation_id, repository_id, number)
);

CREATE TABLE review_revisions (
  id TEXT PRIMARY KEY,
  pr_id TEXT NOT NULL REFERENCES pull_requests(id),
  base_sha TEXT NOT NULL,
  head_sha TEXT NOT NULL,
  policy_version TEXT NOT NULL,
  reviewer_version TEXT NOT NULL,
  state TEXT NOT NULL,
  durable_task_id TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  UNIQUE (pr_id, base_sha, head_sha, policy_version, reviewer_version)
);

CREATE TABLE findings (
  id TEXT PRIMARY KEY,
  pr_id TEXT NOT NULL REFERENCES pull_requests(id),
  first_revision_id TEXT NOT NULL REFERENCES review_revisions(id),
  fingerprint TEXT NOT NULL,
  status TEXT NOT NULL,
  current_severity TEXT NOT NULL,
  github_thread_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE finding_observations (
  id TEXT PRIMARY KEY,
  finding_id TEXT NOT NULL REFERENCES findings(id),
  revision_id TEXT NOT NULL REFERENCES review_revisions(id),
  disposition TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  location_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (finding_id, revision_id)
);

CREATE TABLE webhook_events (
  delivery_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  processed_at TEXT,
  received_at TEXT NOT NULL
);

CREATE TABLE publication_outbox (
  id TEXT PRIMARY KEY,
  revision_id TEXT NOT NULL REFERENCES review_revisions(id),
  idempotency_key TEXT NOT NULL UNIQUE,
  payload_hash TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL,
  github_review_id INTEGER,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);

CREATE TABLE context_snapshots (
  id TEXT PRIMARY KEY,
  revision_id TEXT NOT NULL UNIQUE REFERENCES review_revisions(id),
  pr_json TEXT NOT NULL,
  changed_files_json TEXT NOT NULL,
  diff TEXT NOT NULL,
  limitations_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE decision_records (
  id TEXT PRIMARY KEY,
  revision_id TEXT NOT NULL UNIQUE REFERENCES review_revisions(id),
  input_hash TEXT NOT NULL,
  provider TEXT NOT NULL,
  model_id TEXT NOT NULL,
  raw_response_json TEXT,
  error TEXT,
  normalized_json TEXT,
  selected_mode TEXT NOT NULL,
  reasons_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE candidate_sets (
  id TEXT PRIMARY KEY,
  revision_id TEXT NOT NULL UNIQUE REFERENCES review_revisions(id),
  candidates_json TEXT NOT NULL,
  dispositions_json TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE audit_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  revision_id TEXT NOT NULL REFERENCES review_revisions(id),
  from_state TEXT NOT NULL,
  event TEXT NOT NULL,
  to_state TEXT NOT NULL,
  detail TEXT,
  at TEXT NOT NULL
);

CREATE INDEX audit_events_revision ON audit_events (revision_id, seq);
CREATE INDEX findings_pr ON findings (pr_id);

-- The reviewer's merge risk, one note per aspect; NULL for submissions before this.
ALTER TABLE candidate_sets ADD COLUMN risk_json TEXT;

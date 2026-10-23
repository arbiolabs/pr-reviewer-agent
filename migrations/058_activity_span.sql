-- The waterfall's span of a TOOL_ACTIVITY row (protocol ActivitySpan, as JSON); NULL for a plain activity line.
ALTER TABLE audit_events ADD COLUMN span_json TEXT;

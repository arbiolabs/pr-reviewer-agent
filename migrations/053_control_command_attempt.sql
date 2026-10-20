-- A command's key is bound to one attempt of its run (the reopening count the command ended or opened). The same key
-- sent for a later attempt is a new request, re-evaluated instead of answered from the record. NULL for review.start,
-- whose key names a head rather than an attempt.
ALTER TABLE control_commands ADD COLUMN attempt INTEGER;

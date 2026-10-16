ALTER TABLE targets ADD COLUMN untracked_at TEXT;
ALTER TABLE targets ADD COLUMN purged_at TEXT;
-- a place members paid to walk that none tracks any more
UPDATE targets SET untracked_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), enabled = 0
 WHERE NOT EXISTS (SELECT 1 FROM tracks k WHERE k.target_id = targets.id)
   AND EXISTS (SELECT 1 FROM runs r JOIN token_ledger l ON l.run_id = r.id WHERE r.target_id = targets.id AND l.kind = 'charge');

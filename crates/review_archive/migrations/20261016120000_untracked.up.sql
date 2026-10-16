ALTER TABLE targets ADD COLUMN untracked_at TEXT;
-- `other`: what purged places' charges and kick-backs point at, so a ledger keeps its rows
INSERT INTO targets (id, kind, place_id, lang, interval_secs, enabled, created_at, label)
VALUES (0, 'maps', '', 'en', 3600, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 'other');
INSERT INTO runs (id, target_id, started_at, finished_at, status, error)
VALUES (0, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 'failed', 'a place since purged');
-- a place members paid to walk that none tracks any more
UPDATE targets SET untracked_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), enabled = 0
 WHERE id > 0
   AND NOT EXISTS (SELECT 1 FROM tracks k WHERE k.target_id = targets.id)
   AND EXISTS (SELECT 1 FROM runs r JOIN token_ledger l ON l.run_id = r.id WHERE r.target_id = targets.id AND l.kind = 'charge');

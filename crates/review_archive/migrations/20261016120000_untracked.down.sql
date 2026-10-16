DELETE FROM runs WHERE id = 0;
DELETE FROM targets WHERE id = 0;
ALTER TABLE targets DROP COLUMN untracked_at;

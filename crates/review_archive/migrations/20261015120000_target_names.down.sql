ALTER TABLE targets RENAME COLUMN label TO given;
ALTER TABLE targets ADD COLUMN label TEXT NOT NULL DEFAULT '';
UPDATE targets SET label = COALESCE(given, name, place_id);
ALTER TABLE targets DROP COLUMN given;
ALTER TABLE targets DROP COLUMN name;

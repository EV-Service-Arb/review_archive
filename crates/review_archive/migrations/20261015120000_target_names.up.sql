-- `label` was a person's label or the place's name; it is now only a person's, and `name` the
-- place's as Google last showed it. Every label so far becomes a name: none is known to be a
-- person's, and the next walk replaces them with Google's. The previous release reads `label`
-- as NOT NULL, so this needs the down migration to roll back.
ALTER TABLE targets ADD COLUMN name TEXT;
UPDATE targets SET name = NULLIF(label, place_id);
ALTER TABLE targets DROP COLUMN label;
ALTER TABLE targets ADD COLUMN label TEXT;

-- Additive only: the previous release keeps working on this schema.

-- The place's star average, as the source last gave it.
ALTER TABLE targets ADD COLUMN rating REAL;
-- Of `listed`, how many the last scan that read as deep as scans go left beyond the archive.
ALTER TABLE targets ADD COLUMN unreachable INTEGER NOT NULL DEFAULT 0;

-- Additive only: the previous release's trips get the epoch, so a job probes them at once.
ALTER TABLE maps_breaker ADD COLUMN job_probe_after TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z';

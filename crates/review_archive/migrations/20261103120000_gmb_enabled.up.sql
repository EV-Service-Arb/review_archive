-- Additive only: on until the conductor's next snapshot says otherwise.
ALTER TABLE gmb_mirror ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1));

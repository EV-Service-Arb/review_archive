-- a note's own tier (sa_alerts::Tier), where it is not its rule's
ALTER TABLE alert_notes ADD COLUMN tier TEXT CHECK (tier IN ('info', 'important', 'urgent'));

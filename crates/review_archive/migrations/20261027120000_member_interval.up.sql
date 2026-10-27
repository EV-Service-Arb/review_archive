-- a member's scan interval for every place they pay for; NULL: defaults.interval
ALTER TABLE people ADD COLUMN interval_secs INTEGER;

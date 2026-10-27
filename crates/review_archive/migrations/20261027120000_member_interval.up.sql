-- a member's scan interval for every place they track; NULL: each place's own
ALTER TABLE people ADD COLUMN interval_secs INTEGER;

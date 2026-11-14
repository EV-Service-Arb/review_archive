-- the feed of what walks did, whoever paid; no FK to runs: purging a place leaves it
CREATE TABLE walk_usage (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	run_id INTEGER NOT NULL,
	-- a `sa_core::prices::Item`, `walk.*`
	item TEXT NOT NULL,
	units INTEGER NOT NULL CHECK (units > 0),
	at TEXT NOT NULL,
	UNIQUE (run_id, item)
);

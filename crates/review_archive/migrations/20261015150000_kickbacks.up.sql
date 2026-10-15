-- A run that failed, or met a failure, gives its charges back: pending here until the member claims them.
CREATE TABLE kickbacks (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	run_id INTEGER NOT NULL REFERENCES runs (id),
	amount INTEGER NOT NULL CHECK (amount > 0),
	at TEXT NOT NULL,
	claimed_at TEXT
);
CREATE INDEX kickbacks_pending ON kickbacks (person_id) WHERE claimed_at IS NULL;

CREATE TABLE token_ledger_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	at TEXT NOT NULL,
	delta INTEGER NOT NULL,
	kind TEXT NOT NULL CHECK (kind IN ('accrual', 'grant', 'purchase', 'set', 'charge', 'kickback')),
	run_id INTEGER REFERENCES runs (id),
	by_email TEXT,
	note TEXT,
	CHECK ((kind IN ('charge', 'kickback')) = (run_id IS NOT NULL))
);
INSERT INTO token_ledger_new SELECT * FROM token_ledger;
DROP INDEX token_ledger_person;
DROP TABLE token_ledger;
ALTER TABLE token_ledger_new RENAME TO token_ledger;
CREATE INDEX token_ledger_person ON token_ledger (person_id, kind, at);

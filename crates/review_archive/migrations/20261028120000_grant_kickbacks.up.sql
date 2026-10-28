-- A kick-back is a failed run's charge, or an admin's grant with its note.
CREATE TABLE kickbacks_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	run_id INTEGER REFERENCES runs (id),
	amount INTEGER NOT NULL CHECK (amount > 0),
	at TEXT NOT NULL,
	claimed_at TEXT,
	by_email TEXT,
	note TEXT,
	CHECK ((run_id IS NULL) = (note IS NOT NULL) AND (note IS NULL) = (by_email IS NULL))
);
INSERT INTO kickbacks_new (id, person_id, run_id, amount, at, claimed_at)
SELECT id, person_id, run_id, amount, at, claimed_at FROM kickbacks;
DROP INDEX kickbacks_pending;
DROP TABLE kickbacks;
ALTER TABLE kickbacks_new RENAME TO kickbacks;
CREATE INDEX kickbacks_pending ON kickbacks (person_id) WHERE claimed_at IS NULL;

CREATE TABLE token_ledger_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	at TEXT NOT NULL,
	delta INTEGER NOT NULL,
	kind TEXT NOT NULL CHECK (kind IN ('accrual', 'add', 'set', 'charge', 'kickback')),
	run_id INTEGER REFERENCES runs (id),
	by_email TEXT,
	note TEXT,
	CHECK (CASE kind WHEN 'charge' THEN run_id IS NOT NULL WHEN 'kickback' THEN (run_id IS NULL) = (note IS NOT NULL) ELSE run_id IS NULL END)
);
INSERT INTO token_ledger_new (id, person_id, at, delta, kind, run_id, by_email, note)
SELECT id, person_id, at, delta, CASE WHEN kind IN ('grant', 'purchase') THEN 'add' ELSE kind END, run_id, by_email, note
FROM token_ledger;
DROP INDEX token_ledger_person;
DROP TABLE token_ledger;
ALTER TABLE token_ledger_new RENAME TO token_ledger;
CREATE INDEX token_ledger_person ON token_ledger (person_id, kind, at);

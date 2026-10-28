-- Pending grants are dropped; a claimed one stays in the balance, as a grant.
CREATE TABLE kickbacks_old (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	run_id INTEGER NOT NULL REFERENCES runs (id),
	amount INTEGER NOT NULL CHECK (amount > 0),
	at TEXT NOT NULL,
	claimed_at TEXT
);
INSERT INTO kickbacks_old (id, person_id, run_id, amount, at, claimed_at)
SELECT id, person_id, run_id, amount, at, claimed_at FROM kickbacks WHERE run_id IS NOT NULL;
DROP INDEX kickbacks_pending;
DROP TABLE kickbacks;
ALTER TABLE kickbacks_old RENAME TO kickbacks;
CREATE INDEX kickbacks_pending ON kickbacks (person_id) WHERE claimed_at IS NULL;

CREATE TABLE token_ledger_old (
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
INSERT INTO token_ledger_old (id, person_id, at, delta, kind, run_id, by_email, note)
SELECT id, person_id, at, delta, CASE WHEN kind = 'add' OR (kind = 'kickback' AND run_id IS NULL) THEN 'grant' ELSE kind END, run_id, by_email, note
FROM token_ledger;
DROP INDEX token_ledger_person;
DROP TABLE token_ledger;
ALTER TABLE token_ledger_old RENAME TO token_ledger;
CREATE INDEX token_ledger_person ON token_ledger (person_id, kind, at);

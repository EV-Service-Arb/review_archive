-- A claimed kick-back stays in the balance, as a grant.
CREATE TABLE token_ledger_old (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	at TEXT NOT NULL,
	delta INTEGER NOT NULL,
	kind TEXT NOT NULL CHECK (kind IN ('accrual', 'grant', 'purchase', 'set', 'charge')),
	run_id INTEGER REFERENCES runs (id),
	by_email TEXT,
	note TEXT,
	CHECK ((kind = 'charge') = (run_id IS NOT NULL))
);
INSERT INTO token_ledger_old (id, person_id, at, delta, kind, run_id, by_email, note)
SELECT id, person_id, at, delta,
	CASE kind WHEN 'kickback' THEN 'grant' ELSE kind END,
	CASE kind WHEN 'kickback' THEN NULL ELSE run_id END,
	by_email,
	CASE kind WHEN 'kickback' THEN 'kick-back of run ' || run_id ELSE note END
FROM token_ledger;
DROP INDEX token_ledger_person;
DROP TABLE token_ledger;
ALTER TABLE token_ledger_old RENAME TO token_ledger;
CREATE INDEX token_ledger_person ON token_ledger (person_id, kind, at);

DROP INDEX kickbacks_pending;
DROP TABLE kickbacks;

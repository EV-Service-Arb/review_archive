-- no-transaction
-- Back to gmails of the archive's own, under people by `sub`; balances and kick-backs start
-- empty, as the conductor keeps them.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

INSERT INTO people (sub, email, name, first_seen)
SELECT DISTINCT m.owner, '', '', strftime('%Y-%m-%dT%H:%M:%SZ', 'now') FROM gmb_mirror m WHERE NOT EXISTS (SELECT 1 FROM people p WHERE p.sub = m.owner);

CREATE TABLE managing_gmails (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	gmail TEXT NOT NULL,
	created_at TEXT NOT NULL,
	enabled INTEGER NOT NULL DEFAULT 1,
	UNIQUE (person_id, gmail)
);
INSERT OR IGNORE INTO managing_gmails (id, person_id, gmail, created_at)
SELECT m.gmail_id, p.id, m.gmail, strftime('%Y-%m-%dT%H:%M:%SZ', 'now') FROM gmb_mirror m JOIN people p ON p.sub = m.owner GROUP BY m.gmail_id;

CREATE TABLE tracks_old (
	managing_gmail_id INTEGER NOT NULL REFERENCES managing_gmails (id) ON DELETE CASCADE,
	target_id INTEGER NOT NULL REFERENCES targets (id),
	created_at TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, favorite INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY (managing_gmail_id, target_id)
);
INSERT OR IGNORE INTO tracks_old (managing_gmail_id, target_id, created_at, enabled, favorite)
SELECT m.gmail_id, k.target_id, k.created_at, k.enabled, k.favorite FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id;
DROP INDEX tracks_target;
DROP TABLE tracks;
ALTER TABLE tracks_old RENAME TO tracks;
CREATE INDEX tracks_target ON tracks (target_id);

CREATE TABLE reinstatements_old (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	managing_gmail_id INTEGER NOT NULL REFERENCES managing_gmails (id),
	review_id INTEGER NOT NULL REFERENCES reviews (id),
	requested_at TEXT NOT NULL,
	withdrawn_at TEXT,
	reinstated_at TEXT
);
INSERT INTO reinstatements_old (id, managing_gmail_id, review_id, requested_at, withdrawn_at, reinstated_at)
SELECT x.id, m.gmail_id, x.review_id, x.requested_at, x.withdrawn_at, x.reinstated_at FROM reinstatements x JOIN gmb_mirror m ON m.id = x.gmb_id;
DROP INDEX reinstatements_open;
DROP INDEX reinstatements_review;
DROP TABLE reinstatements;
ALTER TABLE reinstatements_old RENAME TO reinstatements;
CREATE UNIQUE INDEX reinstatements_open ON reinstatements (managing_gmail_id, review_id) WHERE withdrawn_at IS NULL AND reinstated_at IS NULL;
CREATE INDEX reinstatements_review ON reinstatements (review_id);

CREATE TABLE tg_channels_old (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	-- NULL: every place the member tracks
	managing_gmail_id INTEGER REFERENCES managing_gmails (id) ON DELETE CASCADE,
	-- as the member pasted it: `@channel`, `-100…`, `<group>/<topic>`
	destination TEXT NOT NULL,
	-- JSON array of event names
	events TEXT NOT NULL,
	created_at TEXT NOT NULL
);
INSERT INTO tg_channels_old (id, person_id, managing_gmail_id, destination, events, created_at)
SELECT c.id, c.person_id, (SELECT g.id FROM managing_gmails g WHERE g.id = c.gmail_id), c.destination, c.events, c.created_at FROM tg_channels c;
DROP INDEX tg_channels_person;
DROP TABLE tg_channels;
ALTER TABLE tg_channels_old RENAME TO tg_channels;
CREATE INDEX tg_channels_person ON tg_channels (person_id);

CREATE TABLE kickbacks (
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
CREATE INDEX kickbacks_pending ON kickbacks (person_id) WHERE claimed_at IS NULL;
CREATE TABLE token_ledger (
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
CREATE INDEX token_ledger_person ON token_ledger (person_id, kind, at);

INSERT INTO targets (id, kind, place_id, lang, interval_secs, enabled, created_at, label)
VALUES (0, 'maps', '', 'en', 3600, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 'other');
INSERT INTO runs (id, target_id, started_at, finished_at, status, error)
VALUES (0, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), 'failed', 'a place since purged');

DROP TABLE token_charges;
DROP TABLE mirror_cursor;
DROP TABLE mirror_balances;
DROP INDEX gmb_mirror_owner;
DROP TABLE gmb_mirror;

PRAGMA foreign_key_check;
COMMIT;
PRAGMA foreign_keys = ON;

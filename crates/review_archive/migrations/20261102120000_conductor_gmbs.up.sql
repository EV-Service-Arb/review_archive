-- no-transaction
-- Gmails, their GMBs and tokens are the conductor's. A GMB is a gmail's place: the archive
-- keeps a mirror of the conductor's (upserted by id, never deleted), its balances and its
-- cursor over the charge feed. GMB ids are computed as scripts/export_members.sql computes
-- them for the conductor's import.
-- Outside a transaction: dropping `managing_gmails` must not cascade into what references it.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- a named CHECK fails the migration with its name as the message
CREATE TEMP TABLE refused (
	email TEXT,
	CONSTRAINT "every person with a gmail signed in (has a sub) before this release" CHECK (email IS NULL)
);
INSERT INTO refused SELECT p.email FROM people p WHERE p.sub IS NULL AND EXISTS (SELECT 1 FROM managing_gmails g WHERE g.person_id = p.id);

CREATE TEMP TABLE gmb_ids AS
SELECT ROW_NUMBER() OVER (ORDER BY gmail_id, place_id) AS id, gmail_id, place_id FROM (
	SELECT k.managing_gmail_id AS gmail_id, t.place_id FROM tracks k JOIN targets t ON t.id = k.target_id
	UNION
	SELECT x.managing_gmail_id, t.place_id FROM reinstatements x JOIN reviews r ON r.id = x.review_id JOIN targets t ON t.id = r.target_id
);

CREATE TABLE gmb_mirror (
	-- the conductor's ids
	id INTEGER PRIMARY KEY,
	gmail_id INTEGER NOT NULL,
	gmail TEXT NOT NULL,
	-- the gmail owner's concierge `sub`
	owner TEXT NOT NULL,
	removed INTEGER NOT NULL CHECK (removed IN (0, 1)),
	place_id TEXT NOT NULL
);
CREATE INDEX gmb_mirror_owner ON gmb_mirror (owner);
INSERT INTO gmb_mirror (id, gmail_id, gmail, owner, removed, place_id)
SELECT b.id, b.gmail_id, g.gmail, p.sub, 0, b.place_id
FROM gmb_ids b JOIN managing_gmails g ON g.id = b.gmail_id JOIN people p ON p.id = g.person_id;

-- the conductor's balances as of its last snapshot, which included the charges up to `charges_through`
CREATE TABLE mirror_balances (
	sub TEXT PRIMARY KEY,
	balance INTEGER NOT NULL CHECK (balance >= 0)
);
CREATE TABLE mirror_cursor (
	id INTEGER PRIMARY KEY CHECK (id = 1),
	charges_through INTEGER NOT NULL
);
INSERT INTO mirror_cursor (id, charges_through) VALUES (1, 0);

-- the feed the conductor charges from; no FK to runs: purging a place leaves it
CREATE TABLE token_charges (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	run_id INTEGER NOT NULL,
	-- JSON array of subs
	payers TEXT NOT NULL CHECK (json_array_length(payers) > 0),
	spent INTEGER NOT NULL CHECK (spent > 0),
	kick_back INTEGER NOT NULL CHECK (kick_back IN (0, 1)),
	label TEXT NOT NULL,
	at TEXT NOT NULL
);

-- a gmail has no switch any more: a disabled one's tracks go off instead
CREATE TABLE tracks_new (
	gmb_id INTEGER NOT NULL REFERENCES gmb_mirror (id),
	target_id INTEGER NOT NULL REFERENCES targets (id),
	created_at TEXT NOT NULL,
	enabled INTEGER NOT NULL DEFAULT 1,
	favorite INTEGER NOT NULL DEFAULT 0,
	PRIMARY KEY (gmb_id, target_id)
);
INSERT INTO tracks_new (gmb_id, target_id, created_at, enabled, favorite)
SELECT b.id, k.target_id, k.created_at, k.enabled AND g.enabled, k.favorite
FROM tracks k
JOIN managing_gmails g ON g.id = k.managing_gmail_id
JOIN targets t ON t.id = k.target_id
JOIN gmb_ids b ON b.gmail_id = k.managing_gmail_id AND b.place_id = t.place_id;
DROP INDEX tracks_target;
DROP TABLE tracks;
ALTER TABLE tracks_new RENAME TO tracks;
CREATE INDEX tracks_target ON tracks (target_id);

CREATE TABLE reinstatements_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	gmb_id INTEGER NOT NULL REFERENCES gmb_mirror (id),
	review_id INTEGER NOT NULL REFERENCES reviews (id),
	requested_at TEXT NOT NULL,
	withdrawn_at TEXT,
	reinstated_at TEXT
);
INSERT INTO reinstatements_new (id, gmb_id, review_id, requested_at, withdrawn_at, reinstated_at)
SELECT x.id, b.id, x.review_id, x.requested_at, x.withdrawn_at, x.reinstated_at
FROM reinstatements x
JOIN reviews r ON r.id = x.review_id
JOIN targets t ON t.id = r.target_id
JOIN gmb_ids b ON b.gmail_id = x.managing_gmail_id AND b.place_id = t.place_id;
DROP INDEX reinstatements_open;
DROP INDEX reinstatements_review;
DROP TABLE reinstatements;
ALTER TABLE reinstatements_new RENAME TO reinstatements;
CREATE UNIQUE INDEX reinstatements_open ON reinstatements (gmb_id, review_id) WHERE withdrawn_at IS NULL AND reinstated_at IS NULL;
CREATE INDEX reinstatements_review ON reinstatements (review_id);

CREATE TABLE tg_channels_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	-- the conductor's gmail id; NULL: every place the member tracks
	gmail_id INTEGER,
	-- as the member pasted it: `@channel`, `-100…`, `<group>/<topic>`
	destination TEXT NOT NULL,
	-- JSON array of event names
	events TEXT NOT NULL,
	created_at TEXT NOT NULL
);
INSERT INTO tg_channels_new (id, person_id, gmail_id, destination, events, created_at)
SELECT id, person_id, managing_gmail_id, destination, events, created_at FROM tg_channels;
DROP INDEX tg_channels_person;
DROP TABLE tg_channels;
ALTER TABLE tg_channels_new RENAME TO tg_channels;
CREATE INDEX tg_channels_person ON tg_channels (person_id);

DROP INDEX token_ledger_person;
DROP TABLE token_ledger;
DROP INDEX kickbacks_pending;
DROP TABLE kickbacks;
DROP TABLE managing_gmails;

-- `other` held purged places' ledger rows, which are the conductor's now
DELETE FROM runs WHERE id = 0;
DELETE FROM targets WHERE id = 0;

DROP TABLE refused;
DROP TABLE gmb_ids;

PRAGMA foreign_key_check;
COMMIT;
PRAGMA foreign_keys = ON;

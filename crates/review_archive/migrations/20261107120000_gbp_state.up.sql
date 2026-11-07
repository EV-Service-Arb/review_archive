-- no-transaction
-- What tracks are derived from: the conductor's GBP state, the Maps language of its country,
-- its name and address. Filled from the target the GMB tracks, else one it appealed on; a GMB
-- with neither is referenced by nothing, and is forgotten until the next snapshot tells it.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- a named CHECK fails the migration with its name as the message
CREATE TEMP TABLE refused (
	gmb_id INTEGER,
	CONSTRAINT "every GMB tracks one place at most before this release" CHECK (gmb_id IS NULL)
);
INSERT INTO refused SELECT gmb_id FROM tracks GROUP BY gmb_id HAVING COUNT(*) > 1;

CREATE TEMP TABLE source AS
SELECT m.id, COALESCE(
	(SELECT MIN(k.target_id) FROM tracks k WHERE k.gmb_id = m.id),
	(SELECT MIN(r.target_id) FROM reinstatements x JOIN reviews r ON r.id = x.review_id WHERE x.gmb_id = m.id)
) AS target_id
FROM gmb_mirror m;

CREATE TABLE gmb_mirror_new (
	-- the conductor's ids
	id INTEGER PRIMARY KEY,
	gmail_id INTEGER NOT NULL,
	gmail TEXT NOT NULL,
	-- the gmail owner's concierge `sub`
	owner TEXT NOT NULL,
	removed INTEGER NOT NULL CHECK (removed IN (0, 1)),
	enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
	state TEXT NOT NULL CHECK (state IN ('verifying', 'verified', 'suspended')),
	lang TEXT CHECK (lang IS NOT NULL OR removed),
	name TEXT NOT NULL,
	address TEXT,
	place_id TEXT NOT NULL
);
INSERT INTO gmb_mirror_new (id, gmail_id, gmail, owner, removed, enabled, state, lang, name, address, place_id)
SELECT m.id, m.gmail_id, m.gmail, m.owner, m.removed, m.enabled, 'verified', t.lang, COALESCE(t.name, t.label, t.place_id), t.address, m.place_id
FROM gmb_mirror m JOIN source s ON s.id = m.id JOIN targets t ON t.id = s.target_id;
DROP INDEX gmb_mirror_owner;
DROP TABLE gmb_mirror;
ALTER TABLE gmb_mirror_new RENAME TO gmb_mirror;
CREATE INDEX gmb_mirror_owner ON gmb_mirror (owner);

DROP TABLE refused;
DROP TABLE source;
PRAGMA foreign_key_check;
COMMIT;
PRAGMA foreign_keys = ON;

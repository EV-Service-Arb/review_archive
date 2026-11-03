-- The channels and what they were owed are gone: they come back empty.
CREATE TABLE tg_channels (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	person_id INTEGER NOT NULL REFERENCES people (id),
	gmail_id INTEGER,
	destination TEXT NOT NULL,
	events TEXT NOT NULL,
	created_at TEXT NOT NULL
);
CREATE INDEX tg_channels_person ON tg_channels (person_id);

CREATE TABLE webhook_deliveries_old (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	webhook_id INTEGER REFERENCES webhooks (id) ON DELETE CASCADE,
	tg_channel_id INTEGER REFERENCES tg_channels (id) ON DELETE CASCADE,
	event TEXT NOT NULL,
	payload TEXT NOT NULL,
	created_at TEXT NOT NULL,
	attempts INTEGER NOT NULL DEFAULT 0,
	next_attempt_at TEXT NOT NULL,
	delivered_at TEXT,
	failed_at TEXT,
	last_error TEXT,
	CHECK ((webhook_id IS NULL) <> (tg_channel_id IS NULL))
);
INSERT INTO webhook_deliveries_old (id, webhook_id, event, payload, created_at, attempts, next_attempt_at, delivered_at, failed_at, last_error)
SELECT id, webhook_id, event, payload, created_at, attempts, next_attempt_at, delivered_at, failed_at, last_error FROM webhook_deliveries;
DROP INDEX webhook_deliveries_due;
DROP TABLE webhook_deliveries;
ALTER TABLE webhook_deliveries_old RENAME TO webhook_deliveries;
CREATE INDEX webhook_deliveries_due ON webhook_deliveries (next_attempt_at) WHERE delivered_at IS NULL AND failed_at IS NULL;

DROP INDEX alert_notes_created;
DROP TABLE alert_notes;

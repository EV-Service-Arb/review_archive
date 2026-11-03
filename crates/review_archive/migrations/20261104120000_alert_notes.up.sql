-- Alerts are the conductor's: the archive publishes rendered notes it pulls, and keeps no
-- Telegram channels of its own.
CREATE TABLE alert_notes (
	seq INTEGER PRIMARY KEY AUTOINCREMENT,
	id BLOB NOT NULL UNIQUE CHECK (length(id) = 16),
	rule TEXT NOT NULL,
	-- JSON: sa_alerts::NoteText
	text TEXT NOT NULL,
	photo TEXT,
	-- JSON array of the conductor's GMB ids
	gmbs TEXT NOT NULL,
	created_at TEXT NOT NULL
) STRICT;
CREATE INDEX alert_notes_created ON alert_notes (created_at);

CREATE TABLE webhook_deliveries_new (
	id INTEGER PRIMARY KEY AUTOINCREMENT,
	webhook_id INTEGER NOT NULL REFERENCES webhooks (id) ON DELETE CASCADE,
	event TEXT NOT NULL,
	payload TEXT NOT NULL,
	created_at TEXT NOT NULL,
	attempts INTEGER NOT NULL DEFAULT 0,
	next_attempt_at TEXT NOT NULL,
	delivered_at TEXT,
	failed_at TEXT,
	last_error TEXT
);
INSERT INTO webhook_deliveries_new (id, webhook_id, event, payload, created_at, attempts, next_attempt_at, delivered_at, failed_at, last_error)
SELECT id, webhook_id, event, payload, created_at, attempts, next_attempt_at, delivered_at, failed_at, last_error FROM webhook_deliveries WHERE webhook_id IS NOT NULL;
DROP INDEX webhook_deliveries_due;
DROP TABLE webhook_deliveries;
ALTER TABLE webhook_deliveries_new RENAME TO webhook_deliveries;
CREATE INDEX webhook_deliveries_due ON webhook_deliveries (next_attempt_at) WHERE delivered_at IS NULL AND failed_at IS NULL;

DROP INDEX tg_channels_person;
DROP TABLE tg_channels;

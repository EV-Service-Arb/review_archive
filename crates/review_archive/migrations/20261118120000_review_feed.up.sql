-- the conductor's feed of its GBPs' reviews: a row each time one appears, goes or comes back, per GMB tracking its place;
-- no FK: purging a place leaves it. A review without stars has no row.
CREATE TABLE review_feed (
	seq INTEGER PRIMARY KEY AUTOINCREMENT,
	-- Google's id
	review_id TEXT NOT NULL,
	gmb_id INTEGER NOT NULL,
	rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
	text TEXT NOT NULL,
	first_seen TEXT NOT NULL,
	gone_at TEXT
) STRICT;
INSERT INTO review_feed (review_id, gmb_id, rating, text, first_seen, gone_at)
SELECT r.source_review_id, k.gmb_id, r.rating, COALESCE(r.text, ''), r.first_seen, r.gone_at
FROM reviews r JOIN tracks k ON k.target_id = r.target_id
WHERE r.rating IS NOT NULL
ORDER BY r.first_seen, r.id, k.gmb_id;

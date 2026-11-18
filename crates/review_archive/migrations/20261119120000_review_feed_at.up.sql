-- when each row's move happened, and the review's author; a return from before this release has no `at`
ALTER TABLE review_feed ADD COLUMN at TEXT;
ALTER TABLE review_feed ADD COLUMN author TEXT;
UPDATE review_feed SET at = COALESCE(gone_at, first_seen)
WHERE gone_at IS NOT NULL OR NOT EXISTS (SELECT 1 FROM review_feed p WHERE p.review_id = review_feed.review_id AND p.gmb_id = review_feed.gmb_id AND p.seq < review_feed.seq);
CREATE INDEX reviews_source_id_tmp ON reviews (source_review_id);
UPDATE review_feed SET author = (SELECT r.author FROM reviews r WHERE r.source_review_id = review_feed.review_id ORDER BY r.id LIMIT 1);
DROP INDEX reviews_source_id_tmp;
CREATE INDEX review_feed_gmb_at ON review_feed (gmb_id, at);

-- What the conductor imports (`panel import review-archive <file>`): people, their gmails and
-- GMBs, the token ledger and pending kick-backs, as one JSON document. Run on the database
-- before the release that moves them: `sqlite3 <db> < scripts/export_members.sql > members.json`.
-- GMB ids are the ones that release's migration computes, with the same SQL.

WITH gmb_ids AS (
	SELECT ROW_NUMBER() OVER (ORDER BY gmail_id, place_id) AS id, gmail_id, place_id FROM (
		SELECT k.managing_gmail_id AS gmail_id, t.place_id FROM tracks k JOIN targets t ON t.id = k.target_id
		UNION
		SELECT x.managing_gmail_id, t.place_id FROM reinstatements x JOIN reviews r ON r.id = x.review_id JOIN targets t ON t.id = r.target_id
	)
),
-- when a (gmail, place) pair first showed up here
gmb_since AS (
	SELECT gmail_id, place_id, MIN(at) AS at FROM (
		SELECT k.managing_gmail_id AS gmail_id, t.place_id, k.created_at AS at FROM tracks k JOIN targets t ON t.id = k.target_id
		UNION ALL
		SELECT x.managing_gmail_id, t.place_id, x.requested_at FROM reinstatements x JOIN reviews r ON r.id = x.review_id JOIN targets t ON t.id = r.target_id
	) GROUP BY gmail_id, place_id
),
exported AS (
	SELECT p.* FROM people p
	WHERE EXISTS (SELECT 1 FROM managing_gmails g WHERE g.person_id = p.id)
	   OR EXISTS (SELECT 1 FROM token_ledger l WHERE l.person_id = p.id)
	   OR EXISTS (SELECT 1 FROM kickbacks k WHERE k.person_id = p.id AND k.claimed_at IS NULL)
)
SELECT json_object(
	'people', (SELECT json_group_array(json_object('sub', sub, 'email', email, 'name', name, 'first_seen', first_seen)) FROM (SELECT * FROM exported ORDER BY id)),
	'gmails', (SELECT json_group_array(json_object(
			'id', g.id, 'sub', p.sub, 'email', p.email, 'gmail', g.gmail, 'created_at', g.created_at))
		FROM (SELECT * FROM managing_gmails ORDER BY id) g JOIN people p ON p.id = g.person_id),
	'gmbs', (SELECT json_group_array(json_object(
			'id', b.id, 'gmail_id', b.gmail_id, 'place_id', b.place_id,
			-- the place as the oldest of its targets that Google named knows it
			'name', (SELECT t.name FROM targets t WHERE t.place_id = b.place_id AND t.name IS NOT NULL ORDER BY t.id LIMIT 1),
			'address', (SELECT t.address FROM targets t WHERE t.place_id = b.place_id AND t.name IS NOT NULL ORDER BY t.id LIMIT 1),
			'created_at', s.at))
		FROM (SELECT * FROM gmb_ids ORDER BY id) b JOIN gmb_since s ON s.gmail_id = b.gmail_id AND s.place_id = b.place_id),
	'ledger', (SELECT json_group_array(json_object(
			'sub', p.sub, 'email', p.email, 'at', l.at, 'delta', l.delta, 'kind', l.kind, 'run_id', l.run_id,
			'label', (SELECT COALESCE(t.label, t.name, t.place_id) FROM runs r JOIN targets t ON t.id = r.target_id WHERE r.id = l.run_id),
			'by_email', l.by_email, 'note', l.note))
		FROM (SELECT * FROM token_ledger ORDER BY id) l JOIN people p ON p.id = l.person_id),
	'kickbacks', (SELECT json_group_array(json_object(
			'sub', p.sub, 'email', p.email, 'at', k.at, 'amount', k.amount, 'run_id', k.run_id,
			'label', (SELECT COALESCE(t.label, t.name, t.place_id) FROM runs r JOIN targets t ON t.id = r.target_id WHERE r.id = k.run_id),
			'by_email', k.by_email, 'note', k.note))
		FROM (SELECT * FROM kickbacks WHERE claimed_at IS NULL ORDER BY id) k JOIN people p ON p.id = k.person_id)
);

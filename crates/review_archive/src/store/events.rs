//! Events are written beside what caused them: a delivery per hook that wants it, and a note in
//! the conductor's Alerts feed when a live track names the target.

use std::collections::HashSet;

use eyre::WrapErr;
use jiff::{SignedDuration, Timestamp};
use review_archive_core::{
	ReviewId,
	dto::{Event, EventPayload},
	fmt_ts,
};
use sa_alerts::{Note, NotePage, NoteText, Origin, Rule};
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::{RunId, Store, parse_ts, review_by_id, run_by_id, webhooks::parse_events};

/// How long the conductor has to pull a note.
const NOTE_RETENTION: SignedDuration = SignedDuration::from_hours(24);

/// Telegram's limits, in UTF-16 code units.
const MESSAGE_MAX: usize = 4096;
const CAPTION_MAX: usize = 1024;

pub(super) struct Emitter {
	hooks: Vec<(i64, HashSet<Event>)>,
	/// A target's first scan: what it finds is the archive's start, not news to its owners.
	baseline: bool,
	now: Timestamp,
}

impl Emitter {
	pub(super) async fn load(tx: &mut SqliteConnection, now: Timestamp, baseline: bool) -> eyre::Result<Self> {
		let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, events FROM webhooks").fetch_all(&mut *tx).await.wrap_err("loading webhooks")?;
		let hooks = rows
			.into_iter()
			.map(|(id, events)| Ok((id, parse_events(id, &events)?.into_iter().collect())))
			.collect::<eyre::Result<_>>()?;
		Ok(Self { hooks, baseline, now })
	}

	/// A `review.*` event about this review, as it is now in the transaction.
	pub(super) async fn review(&mut self, tx: &mut SqliteConnection, event: Event, id: ReviewId) -> eyre::Result<()> {
		let review = review_by_id(&mut *tx, id).await?.ok_or_else(|| eyre::eyre!("review {id} vanished inside its own transaction"))?;
		let payload = EventPayload {
			event,
			occurred_at: fmt_ts(self.now),
			target_id: review.target_id,
			review: Some(review),
			run: None,
		};
		self.insert(tx, &payload).await
	}

	/// `run.failed` for this run, as just recorded.
	pub(super) async fn run_failed(&mut self, tx: &mut SqliteConnection, run: RunId) -> eyre::Result<()> {
		let run = run_by_id(&mut *tx, run).await?.ok_or_else(|| eyre::eyre!("run {} vanished inside its own transaction", run.0))?;
		let payload = EventPayload {
			event: Event::RunFailed,
			occurred_at: fmt_ts(self.now),
			target_id: run.target_id,
			review: None,
			run: Some(run),
		};
		self.insert(tx, &payload).await
	}

	async fn insert(&self, tx: &mut SqliteConnection, payload: &EventPayload) -> eyre::Result<()> {
		let body = serde_json::to_string(payload)?;
		let now = fmt_ts(self.now);
		for (hook, events) in &self.hooks {
			if !events.contains(&payload.event) {
				continue;
			}
			sqlx::query("INSERT INTO webhook_deliveries (webhook_id, event, payload, created_at, next_attempt_at) VALUES (?, ?, ?, ?, ?)")
				.bind(hook)
				.bind(payload.event.as_ref())
				.bind(&body)
				.bind(&now)
				.bind(&now)
				.execute(&mut *tx)
				.await
				.wrap_err("queueing a delivery")?;
		}
		if self.baseline && payload.event == Event::ReviewNew {
			return Ok(());
		}
		let gmbs: Vec<i64> = sqlx::query_scalar(
			"SELECT DISTINCT m.id FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id
			 WHERE k.target_id = ? AND k.enabled AND m.enabled AND NOT m.removed ORDER BY m.id",
		)
		.bind(payload.target_id)
		.fetch_all(&mut *tx)
		.await
		.wrap_err("loading the event's GMBs")?;
		if gmbs.is_empty() {
			return Ok(());
		}
		let label: String = sqlx::query_scalar("SELECT COALESCE(label, name, place_id) FROM targets WHERE id = ?")
			.bind(payload.target_id)
			.fetch_one(&mut *tx)
			.await
			.wrap_err("loading the event's place")?;
		let (text, photo) = render(payload, &label);
		sqlx::query("INSERT INTO alert_notes (id, rule, text, photo, gmbs, created_at) VALUES (?, ?, ?, ?, ?, ?)")
			.bind(Uuid::new_v4().as_bytes().as_slice())
			.bind(rule(payload.event).as_str())
			.bind(serde_json::to_string(&text)?)
			.bind(photo)
			.bind(serde_json::to_string(&gmbs)?)
			.bind(&now)
			.execute(&mut *tx)
			.await
			.wrap_err("publishing an alert note")?;
		Ok(())
	}
}

fn rule(event: Event) -> Rule {
	match event {
		Event::ReviewNew => Rule::ReviewNew,
		Event::ReviewChanged => Rule::ReviewChanged,
		Event::ReviewGone => Rule::ReviewGone,
		Event::ReviewReappeared => Rule::ReviewReappeared,
		Event::RunFailed => Rule::ScanFailed,
	}
}

/// What an owner reads: what happened, where, and the review as it is now; a removed review's
/// first capture as its photo. Reviews are public, so there is no private-chat variant.
fn render(p: &EventPayload, label: &str) -> (NoteText, Option<String>) {
	let headline = match p.event {
		Event::ReviewNew => "New review",
		Event::ReviewChanged => "Review edited",
		Event::ReviewGone => "Review removed",
		Event::ReviewReappeared => "Review back",
		Event::RunFailed => "Scan failed",
	};
	let mut text = format!("{headline} · {label}");
	if let Some(r) = &p.review {
		let stars = r.rating.map(|n| "★".repeat(n.clamp(0, 5) as usize)).unwrap_or_default(); // an unrated review shows no stars
		text.push_str(&format!("\n{stars} {}", r.author));
		if let Some(t) = &r.text {
			text.push_str(&format!("\n{t}"));
		}
	}
	if let Some(e) = p.run.as_ref().and_then(|r| r.error.as_deref()) {
		text.push_str(&format!("\n{e}"));
	}
	let photo = match (p.event, p.review.as_ref().and_then(|r| r.capture_sha256.as_deref())) {
		(Event::ReviewGone, Some(sha)) => Some(format!("/api/conductor/captures/{sha}.png")),
		_ => None,
	};
	let text = truncate(text, if photo.is_some() { CAPTION_MAX } else { MESSAGE_MAX });
	let bold = serde_json::json!({"type": "bold", "offset": 0, "length": headline.encode_utf16().count()});
	(NoteText { text, entities: vec![bold] }, photo)
}

/// At most `max` UTF-16 code units, an ellipsis included when cut.
fn truncate(s: String, max: usize) -> String {
	if s.encode_utf16().count() <= max {
		return s;
	}
	let mut used = '…'.len_utf16();
	let (end, _) = s
		.char_indices()
		.find(|&(_, c)| {
			used += c.len_utf16();
			used > max
		})
		.expect("longer than max, so some char crosses it");
	format!("{}…", &s[..end])
}

type NoteRow = (i64, Vec<u8>, String, String, Option<String>, String, String);

impl Store {
	/// The Alerts feed past `after`, `limit` notes at most.
	pub async fn notes(&self, after: i64, limit: u32) -> eyre::Result<NotePage> {
		let rows: Vec<NoteRow> = sqlx::query_as("SELECT seq, id, rule, text, photo, gmbs, created_at FROM alert_notes WHERE seq > ? ORDER BY seq LIMIT ?")
			.bind(after)
			.bind(limit)
			.fetch_all(&self.pool)
			.await
			.wrap_err("loading alert notes")?;
		let notes = rows
			.into_iter()
			.map(|(seq, id, rule, text, photo, gmbs, created_at)| {
				let note = Note {
					seq,
					rule: rule.parse()?,
					event_id: Uuid::from_slice(&id)?,
					created_at: parse_ts(&created_at)?,
					text: serde_json::from_str(&text).wrap_err_with(|| format!("note {seq}'s text"))?,
					text_pii: None,
					skip_user: None,
					buttons: vec![],
					gmbs: serde_json::from_str(&gmbs).wrap_err_with(|| format!("note {seq}'s GMBs"))?,
					photo,
				};
				note.check(Origin::ReviewArchive)?;
				Ok(note)
			})
			.collect::<eyre::Result<Vec<_>>>()?;
		Ok(NotePage {
			next: notes.last().map_or(after, |n| n.seq),
			notes,
		})
	}

	/// Drops the notes older than [`NOTE_RETENTION`]; how many.
	pub(crate) async fn prune_notes(&self, now: Timestamp) -> eyre::Result<u64> {
		let done = sqlx::query("DELETE FROM alert_notes WHERE created_at < ?")
			.bind(fmt_ts(now - NOTE_RETENTION))
			.execute(&self.pool)
			.await
			.wrap_err("pruning alert notes")?;
		Ok(done.rows_affected())
	}
}

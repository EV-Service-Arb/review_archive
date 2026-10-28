//! Members pay for the walks of the places they track: a ledger renewed daily up to a cap,
//! charges split across trackers with the run's end, and no walk for a place whose trackers
//! are out of tokens, nor past the account's hourly limit. What a member paid adds up by day.

use std::{path::Path, sync::Mutex};

use jiff::{SignedDuration, Timestamp};
use review_archive::{
	Archive,
	config::Config,
	core::{
		Coverage, Known, PersonId, Rejected, Scan, Target, TargetId, Warning,
		dto::{BalanceChange, KickbackWhy, LocationSummary, NewGmail, NewTarget, ScanState, TokenKind, UsageDay},
		schedule::Schedule,
		tokens::{Meter, Tokens},
	},
	record::Recorder,
	sources::ReviewSource,
	store::{
		Store,
		blobs::BlobStore,
		people::{Claim, Seen},
	},
};

/// A walk that costs `.0` tokens.
struct Spends(i64);

/// A walk Google stops.
struct Fails;

impl ReviewSource for Fails {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		eyre::bail!("blocked")
	}
}

/// A walk that costs `.0` tokens and falls short as `.1` says.
struct Warns(i64, Warning);

impl ReviewSource for Warns {
	async fn scan(&self, target: &Target, known: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		let mut scan = Spends(self.0).scan(target, known, meter).await?;
		scan.warnings.push(self.1.clone());
		Ok(scan)
	}
}

/// A walk that costs `.0` tokens, then breaks.
struct BreaksAfter(i64);

impl ReviewSource for BreaksAfter {
	async fn scan(&self, _: &Target, _: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		meter.spend(self.0);
		eyre::bail!("the page went away")
	}
}

/// A walk that reads `member`'s card for its place while it goes.
struct Peeks<'a> {
	archive: &'a Archive,
	member: PersonId,
	seen: Mutex<Option<ScanState>>,
}

impl ReviewSource for Peeks<'_> {
	async fn scan(&self, target: &Target, known: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		*self.seen.lock().unwrap() = card(self.archive, self.member, target.id).await.scan;
		Spends(0).scan(target, known, meter).await
	}
}

async fn card(archive: &Archive, member: PersonId, t: TargetId) -> LocationSummary {
	archive
		.overview(member)
		.await
		.unwrap()
		.into_iter()
		.flat_map(|g| g.locations)
		.find(|l| l.target.id == t.0)
		.unwrap()
}

fn recorder<'a>(archive: &'a Archive, blobs: &'a BlobStore) -> Recorder<'a> {
	Recorder {
		store: archive.store().unwrap(),
		blobs,
		now: Timestamp::now,
		schedule: Box::leak(Box::default()),
		tokens: Box::leak(Box::default()),
	}
}

impl ReviewSource for Spends {
	async fn scan(&self, _: &Target, _: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		meter.spend(self.0);
		Ok(Scan {
			reviews: vec![],
			coverage: Coverage::DownTo(None),
			warnings: vec![],
			cut_after: None,
			listed: None,
			post: None,
			name: None,
			address: None,
		})
	}
}

fn tokens() -> Tokens {
	Tokens {
		start_cap_multiple: Some(2.0),
		..Tokens::default()
	}
}

async fn open(dir: &Path) -> Archive {
	Archive::open(Config {
		data_dir: Some(dir.to_owned()),
		tokens: tokens(),
		..Config::default()
	})
	.await
	.unwrap()
}

async fn target(archive: &Archive, place: &str) -> Target {
	let t = NewTarget {
		place: Some(place.to_owned()),
		..Default::default()
	};
	archive.add_target(&t).await.unwrap().target
}

async fn person(store: &Store, sub: &str) -> PersonId {
	let email = format!("{sub}@x");
	let seen = Seen {
		sub,
		email: &email,
		email_verified: true,
		name: sub,
	};
	match store.person(&seen, Timestamp::now()).await.unwrap() {
		Claim::Person(p) => p,
		refused => panic!("{sub}: {refused:?}"),
	}
}

/// `member` tracks `t` under a gmail of their own; returns the gmail.
async fn tracks(archive: &Archive, member: PersonId, t: TargetId) -> i64 {
	let g = archive.add_gmail(member, &NewGmail { gmail: format!("{member}.ops") }).await.unwrap();
	archive.assign(g.id, t).await.unwrap();
	g.id
}

async fn set(archive: &Archive, member: PersonId, n: i64) {
	archive
		.store()
		.unwrap()
		.change_balance(member, BalanceChange::Set(n), "root", None, Timestamp::now(), &tokens())
		.await
		.unwrap();
}

#[tokio::test]
async fn renewal_stops_at_the_cap_and_never_takes_back() {
	let dir = tempfile::tempdir().unwrap();
	let store = open(dir.path()).await.store().unwrap().clone();
	let cfg = tokens();
	let a = person(&store, "a").await;
	let t0: Timestamp = "2026-10-01T09:00:00Z".parse().unwrap();
	let day = |n: i64| t0 + SignedDuration::from_hours(24 * n);
	assert_eq!(store.balance(a, t0, &cfg).await.unwrap(), 1000, "twice the cap on first sight");
	assert_eq!(store.balance(a, day(3), &cfg).await.unwrap(), 1000, "nothing renews above the cap");

	assert_eq!(store.change_balance(a, BalanceChange::Set(400), "root", None, day(3), &cfg).await.unwrap(), 400);
	assert_eq!(store.balance(a, day(4), &cfg).await.unwrap(), 415);
	assert_eq!(store.balance(a, day(4) + SignedDuration::from_hours(5), &cfg).await.unwrap(), 415, "once a day");
	assert_eq!(store.balance(a, day(100), &cfg).await.unwrap(), 500);

	let bought = store.change_balance(a, BalanceChange::Add(200), "root", Some("inv-1"), day(100), &cfg).await.unwrap();
	assert_eq!(bought, 700);
	assert_eq!(store.balance(a, day(130), &cfg).await.unwrap(), 700, "above the cap stays");

	assert_eq!(store.change_balance(a, BalanceChange::Set(8), "root", None, day(130), &cfg).await.unwrap(), 8);
	assert_eq!(store.balance(a, day(130), &cfg).await.unwrap(), 8);
	// days spent at the cap renew nothing afterwards: renewal counts from the last day it ran
	assert_eq!(store.balance(a, day(131), &cfg).await.unwrap(), 23);
}

#[tokio::test]
async fn trackers_split_a_walk_within_their_balances() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest000000split").await;
	let poor = person(archive.store().unwrap(), "poor").await;
	let rich = person(archive.store().unwrap(), "rich").await;
	tracks(&archive, poor, t.id).await;
	tracks(&archive, rich, t.id).await;
	set(&archive, poor, 5).await;
	set(&archive, rich, 100).await;

	let rec = archive.record(&Spends(20), &t).await.unwrap();
	assert_eq!(archive.runs(t.id, &Default::default()).await.unwrap()[0].tokens, 20);
	for (member, left, paid) in [(poor, 0, -5), (rich, 85, -15)] {
		assert_eq!(archive.tokens(member).await.unwrap().balance, left, "{member}");
		let charge = archive.ledger(member).await.unwrap().into_iter().find(|l| l.kind == TokenKind::Charge).unwrap();
		assert_eq!((charge.delta, charge.run_id, charge.target_id), (paid, Some(rec.run.0), Some(t.id.0)), "{member}");
	}

	// the operator's own look is no member's to pay for
	let job = archive.enqueue_scan(t.id).await.unwrap();
	archive.store().unwrap().claim_job(Timestamp::now()).await.unwrap().unwrap();
	recorder(&archive, &BlobStore::new(dir.path().join("blobs"))).record(&Spends(10), &t, Some(job)).await.unwrap();
	assert_eq!(archive.tokens(rich).await.unwrap().balance, 85);
}

#[tokio::test]
async fn a_place_whose_trackers_are_out_of_tokens_waits() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest00000broke").await;
	let broke = person(archive.store().unwrap(), "broke").await;
	tracks(&archive, broke, t.id).await;
	set(&archive, broke, 0).await;
	let now = Timestamp::now();
	assert!(archive.due(now).await.unwrap().is_empty());
	assert_eq!(archive.next_due(now).await.unwrap(), None, "nothing to wake up for until a balance renews");

	tracks(&archive, person(archive.store().unwrap(), "flush").await, t.id).await;
	assert_eq!(archive.due(now).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [t.id]);
}

#[tokio::test]
async fn the_hour_closes_maps_whoever_pays() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let spent = target(&archive, "ChIJtokenstest00000spent").await;
	let waiting = target(&archive, "ChIJtokenstest0000waiting").await;
	archive.record(&Spends(Tokens::default().per_hour), &spent).await.unwrap();
	let now = Timestamp::now();
	assert!(archive.due(now).await.unwrap().is_empty());
	assert!(archive.enqueue_scan(waiting.id).await.is_err());
	let reopens = archive.next_due(now).await.unwrap().unwrap();
	assert!(reopens > now + SignedDuration::from_mins(59), "{reopens}");
}

#[tokio::test]
async fn usage_sums_a_members_own_charges_by_utc_day() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let store = archive.store().unwrap();
	let shared = target(&archive, "ChIJtokenstest000usage01").await;
	let own = target(&archive, "ChIJtokenstest000usage02").await;
	let me = person(store, "me").await;
	let other = person(store, "other").await;
	let g = archive.add_gmail(me, &NewGmail { gmail: "me.ops".into() }).await.unwrap();
	archive.assign(g.id, shared.id).await.unwrap();
	archive.assign(g.id, own.id).await.unwrap();
	tracks(&archive, other, shared.id).await;
	set(&archive, me, 1000).await;
	set(&archive, other, 1000).await;

	let blobs = BlobStore::new(dir.path().join("blobs"));
	let walk = |now: fn() -> Timestamp, t: Target, cost: i64| {
		let (archive, blobs) = (&archive, &blobs);
		async move {
			let rec = Recorder {
				store: archive.store().unwrap(),
				blobs,
				now,
				schedule: &Schedule::default(),
				tokens: &tokens(),
			};
			rec.record(&Spends(cost), &t, None).await.unwrap();
		}
	};
	walk(|| "2026-10-04T23:30:00Z".parse().unwrap(), own.clone(), 7).await;
	walk(|| "2026-10-05T00:30:00Z".parse().unwrap(), shared.clone(), 10).await; // split with `other`
	walk(|| "2026-10-05T02:00:00Z".parse().unwrap(), own.clone(), 3).await;
	walk(|| "2026-08-01T12:00:00Z".parse().unwrap(), own.clone(), 50).await; // past the window

	let usage = store.usage(me, "2026-10-06T08:00:00Z".parse().unwrap()).await.unwrap();
	assert_eq!(usage.days.len(), 30);
	assert_eq!(usage.days[0].day, "2026-09-07");
	let charged: Vec<&UsageDay> = usage.days.iter().filter(|d| d.walks > 0).collect();
	assert_eq!(
		charged,
		[
			&UsageDay {
				day: "2026-10-04".into(),
				walks: 1,
				tokens: 7
			},
			&UsageDay {
				day: "2026-10-05".into(),
				walks: 2,
				tokens: 8
			},
		]
	);
	assert_eq!(
		usage.days.last().unwrap(),
		&UsageDay {
			day: "2026-10-06".into(),
			walks: 0,
			tokens: 0
		}
	);
	assert_eq!(usage.places_tracked, 2);
	assert_eq!(store.usage(other, "2026-10-06T08:00:00Z".parse().unwrap()).await.unwrap().places_tracked, 1);
}

#[tokio::test]
async fn a_scan_a_member_asks_for_is_theirs_alone_to_pay() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest00000asked").await;
	let a = person(archive.store().unwrap(), "a").await;
	let b = person(archive.store().unwrap(), "b").await;
	let g = tracks(&archive, a, t.id).await;
	tracks(&archive, b, t.id).await;
	set(&archive, a, 100).await;
	set(&archive, b, 100).await;

	let job = archive.scan_tracked(a, g, t.id).await.unwrap();
	assert_eq!(archive.store().unwrap().claim_job(Timestamp::now()).await.unwrap().unwrap().id, job);
	let blobs = BlobStore::new(dir.path().join("blobs"));
	recorder(&archive, &blobs).record(&Spends(10), &t, Some(job)).await.unwrap();
	assert_eq!(archive.tokens(a).await.unwrap().balance, 90);
	assert_eq!(archive.tokens(b).await.unwrap().balance, 100);
}

#[tokio::test]
async fn a_member_out_of_tokens_cannot_ask_for_a_scan() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest000000poor").await;
	let broke = person(archive.store().unwrap(), "broke").await;
	let g = tracks(&archive, broke, t.id).await;
	set(&archive, broke, 0).await;
	let refused = archive.scan_tracked(broke, g, t.id).await.unwrap_err();
	assert!(matches!(refused.downcast_ref::<Rejected>(), Some(Rejected::Busy(_))), "{refused:#}");
}

#[tokio::test]
async fn a_scan_a_member_asks_for_leaves_the_schedule_alone() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest00schedule").await;
	let m = person(archive.store().unwrap(), "m").await;
	let g = tracks(&archive, m, t.id).await;
	set(&archive, m, 100).await;
	archive.record(&Spends(5), &t).await.unwrap();
	let due = archive.due_at(&t).await.unwrap().unwrap();

	let blobs = BlobStore::new(dir.path().join("blobs"));
	let store = archive.store().unwrap();
	let job = archive.scan_tracked(m, g, t.id).await.unwrap();
	store.claim_job(Timestamp::now()).await.unwrap().unwrap();
	recorder(&archive, &blobs).record(&Spends(5), &t, Some(job)).await.unwrap();
	assert_eq!(archive.due_at(&t).await.unwrap(), Some(due), "a scan-now is not the schedule's run");

	let job = archive.scan_tracked(m, g, t.id).await.unwrap();
	store.claim_job(Timestamp::now()).await.unwrap().unwrap();
	recorder(&archive, &blobs).record(&Fails, &t, Some(job)).await.unwrap();
	assert_eq!(archive.due_at(&t).await.unwrap(), Some(due), "nor is its failure the schedule's backoff");
}

#[tokio::test]
async fn the_card_shows_a_scan_queued_then_running_then_gone() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest00000card1").await;
	let other = target(&archive, "ChIJtokenstest00000card2").await;
	let m = person(archive.store().unwrap(), "m").await;
	let g = tracks(&archive, m, t.id).await;
	set(&archive, m, 100).await;
	assert_eq!(card(&archive, m, t.id).await.scan, None);

	archive.enqueue_scan(other.id).await.unwrap();
	let job = archive.scan_tracked(m, g, t.id).await.unwrap();
	assert!(matches!(card(&archive, m, t.id).await.scan, Some(ScanState::Queued { ahead: 1, .. })));

	let store = archive.store().unwrap();
	store.claim_job(Timestamp::now()).await.unwrap().unwrap();
	assert_eq!(store.claim_job(Timestamp::now()).await.unwrap().unwrap().id, job);
	let peek = Peeks {
		archive: &archive,
		member: m,
		seen: Mutex::new(None),
	};
	let blobs = BlobStore::new(dir.path().join("blobs"));
	recorder(&archive, &blobs).record(&peek, &t, Some(job)).await.unwrap();
	assert!(matches!(*peek.seen.lock().unwrap(), Some(ScanState::Running { .. })));
	assert_eq!(card(&archive, m, t.id).await.scan, None);
}

#[tokio::test]
async fn a_failure_is_kicked_back() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest000kickback").await;
	let a = person(archive.store().unwrap(), "a").await;
	tracks(&archive, a, t.id).await;
	set(&archive, a, 100).await;

	let rec = archive.record(&Warns(20, Warning::Failure("screenshot of x failed".into())), &t).await.unwrap();
	let tokens = archive.tokens(a).await.unwrap();
	assert_eq!(tokens.balance, 80, "charged all the same");
	assert_eq!(
		tokens.kickbacks.iter().map(|k| (k.amount, &k.why)).collect::<Vec<_>>(),
		[(
			20,
			&KickbackWhy::Failed {
				run_id: rec.run.0,
				target_label: t.place_id.clone(),
				error: "screenshot of x failed".into()
			}
		)]
	);

	let claimed = archive.claim_kickbacks(a).await.unwrap();
	assert_eq!((claimed.balance, claimed.kickbacks.len()), (100, 0));
	let back = archive.ledger(a).await.unwrap().into_iter().find(|l| l.kind == TokenKind::Kickback).unwrap();
	assert_eq!((back.delta, back.run_id, back.target_id), (20, Some(rec.run.0), Some(t.id.0)));
	assert_eq!(archive.claim_kickbacks(a).await.unwrap().balance, 100, "claimed once");

	archive.record(&Warns(10, Warning::Limit("out of tokens".into())), &t).await.unwrap();
	let tokens = archive.tokens(a).await.unwrap();
	assert_eq!((tokens.balance, tokens.kickbacks.len()), (90, 0), "a limit is no failure");

	let b = person(archive.store().unwrap(), "b").await;
	tracks(&archive, b, t.id).await;
	set(&archive, b, 4).await;
	archive.record(&BreaksAfter(20), &t).await.unwrap();
	assert_eq!(archive.tokens(a).await.unwrap().balance, 74);
	assert_eq!(archive.tokens(b).await.unwrap().balance, 0);
	assert_eq!(archive.claim_kickbacks(a).await.unwrap().balance, 90, "each gets back what they paid");
	assert_eq!(archive.claim_kickbacks(b).await.unwrap().balance, 4);
}

/// A purged place leaves nothing behind but what its members were charged and are owed, under `other`.
#[tokio::test]
async fn a_purged_places_tokens_stay_under_other() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let t = target(&archive, "ChIJtokenstest00000purged").await;
	let a = person(archive.store().unwrap(), "a").await;
	let gmail = tracks(&archive, a, t.id).await;
	set(&archive, a, 100).await;
	archive.record(&Spends(20), &t).await.unwrap();
	archive.record(&Warns(10, Warning::Failure("screenshot of x failed".into())), &t).await.unwrap();
	archive.untrack(a, gmail, t.id).await.unwrap();

	archive.purge_untracked(Timestamp::now() + SignedDuration::from_hours(24 * 7)).await.unwrap();
	let tokens = archive.tokens(a).await.unwrap();
	assert_eq!(tokens.balance, 70);
	let labels: Vec<_> = tokens
		.kickbacks
		.iter()
		.map(|k| match &k.why {
			KickbackWhy::Failed { target_label, .. } => (target_label.as_str(), k.amount),
			KickbackWhy::Granted { .. } => unreachable!("none granted"),
		})
		.collect();
	assert_eq!(labels, [("other", 10)]);
	let charges: Vec<_> = archive
		.ledger(a)
		.await
		.unwrap()
		.into_iter()
		.filter(|l| l.kind == TokenKind::Charge)
		.map(|l| (l.delta, l.target_label))
		.collect();
	assert_eq!(charges, [(-10, Some("other".into())), (-20, Some("other".into()))]);
	assert!(archive.store().unwrap().targets().await.unwrap().is_empty(), "`other` is no target");
	assert_eq!(archive.claim_kickbacks(a).await.unwrap().balance, 80);
}

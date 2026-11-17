//! Members pay for the walks of the places they track: what they hold is the conductor's
//! balance less the charges the feed holds past its cursor, a walk's charge goes into the feed
//! with the run's end, and no walk for a place whose trackers are out of tokens, nor past the
//! account's hourly limit.

use std::{cell::RefCell, path::Path, sync::Mutex, time::Duration};

use jiff::{SignedDuration, Timestamp};
use review_archive::{
	Archive,
	config::Config,
	core::{
		Coverage, Known, PersonId, Rejected, Scan, Target, TargetId, Warning,
		dto::{Charge, Country, LocationSummary, Managed, NewTarget, OnGoogle, ScanState, Snapshot},
		fmt_ts,
		tokens::{Meter, Tokens, WalkStep},
	},
	record::Recorder,
	sources::ReviewSource,
	store::{
		Store,
		blobs::BlobStore,
		people::{Claim, Seen},
	},
};
use sa_core::prices::Item;
use tokio::sync::Notify;
use uuid::Uuid;

#[path = "../support/priced.rs"]
mod priced;

/// A walk that costs `.0` tokens: that many scrolls, at [`priced::snapshot`]'s prices.
struct Spends(i64);

fn scrolls(meter: &mut Meter, n: i64) {
	for _ in 0..n {
		meter.spend(WalkStep::Step);
	}
}

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
		scrolls(meter, self.0);
		eyre::bail!("the page went away")
	}
}

/// A walk that opens the place, its reviews and their sort, then scrolls `.0` times.
struct Opens(i64);

impl ReviewSource for Opens {
	async fn scan(&self, target: &Target, known: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		opens(meter);
		Spends(self.0).scan(target, known, meter).await
	}
}

fn opens(meter: &mut Meter) {
	for s in [WalkStep::Open, WalkStep::Reviews, WalkStep::Sort] {
		meter.spend(s);
	}
}

/// A walk that opens and scrolls `before` times, waits for `gate`, then scrolls `after` times more.
struct Gated {
	before: i64,
	gate: Notify,
	after: i64,
}

impl ReviewSource for Gated {
	async fn scan(&self, target: &Target, known: &Known, meter: &mut Meter) -> eyre::Result<Scan> {
		opens(meter);
		scrolls(meter, self.before);
		self.gate.notified().await;
		Spends(self.after).scan(target, known, meter).await
	}
}

/// Past the walk's step checkpoint, with room for its write.
const PAST_A_CHECKPOINT: Duration = Duration::from_secs(12);

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
		.flat_map(|g| g.gmbs)
		.map(|b| b.track)
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
		scrolls(meter, self.0);
		Ok(Scan {
			reviews: vec![],
			coverage: Coverage::DownTo(None),
			warnings: vec![],
			cut_after: None,
			listed: None,
			rating: None,
			deepest: false,
			post: None,
			name: None,
			address: None,
		})
	}
}

async fn open(dir: &Path) -> Archive {
	Archive::open(Config {
		data_dir: Some(dir.to_owned()),
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

/// The concierge `sub` of the member named `name`, 16 bytes at most.
fn owner(name: &str) -> Uuid {
	let mut b = [0; 16];
	b[..name.len()].copy_from_slice(name.as_bytes());
	Uuid::from_bytes(b)
}

async fn person(store: &Store, name: &str) -> PersonId {
	let email = format!("{name}@x");
	let sub = owner(name).to_string();
	let seen = Seen {
		sub: &sub,
		email: &email,
		email_verified: true,
		name,
	};
	match store.person(&seen, Timestamp::now()).await.unwrap() {
		Claim::Person(p) => p,
		refused => panic!("{name}: {refused:?}"),
	}
}

/// What the conductor pushed last.
struct Conductor(RefCell<Snapshot>);

impl Conductor {
	fn new() -> Self {
		Self(RefCell::new(priced::snapshot()))
	}

	async fn push(&self, archive: &Archive) {
		let snap = self.0.borrow().clone();
		archive.apply_snapshot(&snap).await.unwrap();
	}

	/// `sub` tracks `t` under a verified GMB of their own on its place and language; returns the GMB.
	async fn tracks(&self, archive: &Archive, sub: &str, t: &Target) -> i64 {
		let id = {
			let mut snap = self.0.borrow_mut();
			let id = snap.gmbs.len() as i64 + 1;
			snap.gmbs.push(Managed {
				id,
				gmail_id: id,
				gmail: format!("{sub}.ops"),
				owner: owner(sub),
				removed: false,
				enabled: true,
				place_id: t.place_id.clone(),
				state: OnGoogle::Verified,
				country: Some(match t.lang.as_str() {
					"fr" => Country::FR,
					"en" => Country::US,
					other => panic!("no GBP's country reads {other}"),
				}),
				name: sub.into(),
				address: None,
			});
			id
		};
		self.push(archive).await;
		id
	}

	/// The conductor removes `gmb`.
	async fn remove(&self, archive: &Archive, gmb: i64) {
		self.0.borrow_mut().gmbs.iter_mut().find(|g| g.id == gmb).unwrap().removed = true;
		self.push(archive).await;
	}

	/// The conductor's switch of `gmb`.
	async fn switch(&self, archive: &Archive, gmb: i64, on: bool) {
		self.0.borrow_mut().gmbs.iter_mut().find(|g| g.id == gmb).unwrap().enabled = on;
		self.push(archive).await;
	}

	/// `sub` holds `n`: the conductor has charged everything the feed holds so far.
	async fn set(&self, archive: &Archive, sub: &str, n: i64) {
		let through = archive.charges(0, 500).await.unwrap().next;
		{
			let mut snap = self.0.borrow_mut();
			snap.balances.insert(owner(sub), n);
			snap.charges_through = through;
		}
		self.push(archive).await;
	}
}

async fn holds(archive: &Archive, sub: &str) -> i64 {
	archive.store().unwrap().holdings(&[owner(sub).to_string()]).await.unwrap()
}

async fn feed(archive: &Archive) -> Vec<Charge> {
	archive.charges(0, 500).await.unwrap().charges
}

#[tokio::test]
async fn trackers_split_a_walk_within_their_balances() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest000000split").await;
	person(archive.store().unwrap(), "poor").await;
	person(archive.store().unwrap(), "rich").await;
	conductor.tracks(&archive, "poor", &t).await;
	conductor.tracks(&archive, "rich", &t).await;
	conductor.set(&archive, "poor", 5).await;
	conductor.set(&archive, "rich", 100).await;

	let rec = archive.record(&Spends(20), &t).await.unwrap();
	assert_eq!(archive.runs(t.id, &Default::default()).await.unwrap()[0].tokens, 20);
	let charged = feed(&archive).await;
	assert_eq!(charged.len(), 1);
	let c = &charged[0];
	assert_eq!(
		(c.key.as_str(), c.payers.as_slice(), c.spent, c.kick_back, c.label.as_str()),
		(format!("run:{}", rec.run.0).as_str(), [owner("poor"), owner("rich")].as_slice(), 20, false, t.place_id.as_str())
	);
	for (sub, left) in [("poor", 0), ("rich", 90)] {
		assert_eq!(holds(&archive, sub).await, left, "{sub}");
	}

	// the operator's own look is no member's to pay for
	let job = archive.enqueue_scan(t.id).await.unwrap();
	archive.store().unwrap().claim_job(Timestamp::now()).await.unwrap().unwrap();
	recorder(&archive, &BlobStore::new(dir.path().join("blobs"))).record(&Spends(10), &t, Some(job)).await.unwrap();
	assert_eq!(feed(&archive).await.len(), 1);
}

#[tokio::test]
async fn a_place_whose_trackers_are_out_of_tokens_waits() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest00000broke").await;
	person(archive.store().unwrap(), "broke").await;
	conductor.tracks(&archive, "broke", &t).await;
	conductor.set(&archive, "broke", 0).await;
	let now = Timestamp::now();
	assert!(archive.due(now).await.unwrap().is_empty());
	assert_eq!(archive.next_due(now).await.unwrap(), None, "nothing to wake up for until a balance renews");

	person(archive.store().unwrap(), "flush").await;
	conductor.tracks(&archive, "flush", &t).await;
	conductor.set(&archive, "flush", 100).await;
	assert_eq!(archive.due(now).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [t.id]);
}

#[tokio::test]
async fn a_gmb_the_conductor_switched_off_neither_walks_nor_pays() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest0000switch").await;
	let off = person(archive.store().unwrap(), "off").await;
	person(archive.store().unwrap(), "on").await;
	let off_gmb = conductor.tracks(&archive, "off", &t).await;
	let on_gmb = conductor.tracks(&archive, "on", &t).await;
	conductor.set(&archive, "off", 100).await;
	conductor.set(&archive, "on", 100).await;
	conductor.switch(&archive, off_gmb, false).await;
	conductor.switch(&archive, on_gmb, false).await;
	let now = Timestamp::now();
	assert!(archive.due(now).await.unwrap().is_empty());
	assert_eq!(archive.next_due(now).await.unwrap(), None);
	let refused = archive.scan_tracked(off, off_gmb, t.id).await.unwrap_err();
	assert!(matches!(refused.downcast_ref::<Rejected>(), Some(Rejected::Busy(_))), "{refused:#}");
	let gmb = archive.overview(off).await.unwrap().into_iter().flat_map(|g| g.gmbs).find(|b| b.id == off_gmb).unwrap();
	assert!(!gmb.enabled);
	assert!(gmb.track.enabled, "the track's own switch stays the member's");
	assert_eq!(archive.my_gmbs(off).await.unwrap().places_tracked, 0);

	conductor.switch(&archive, on_gmb, true).await;
	assert_eq!(archive.due(now).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [t.id]);
	archive.record(&Spends(20), &t).await.unwrap();
	assert_eq!(feed(&archive).await[0].payers, [owner("on")]);
	assert_eq!(holds(&archive, "off").await, 100);
}

#[tokio::test]
async fn the_hour_closes_maps_whoever_pays() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let spent = target(&archive, "ChIJtokenstest00000spent").await;
	let waiting = target(&archive, "ChIJtokenstest0000waiting").await;
	Conductor::new().push(&archive).await;
	archive.record(&Spends(Tokens::default().per_hour), &spent).await.unwrap();
	let now = Timestamp::now();
	assert!(archive.due(now).await.unwrap().is_empty());
	assert!(archive.enqueue_scan(waiting.id).await.is_err());
	let reopens = archive.next_due(now).await.unwrap().unwrap();
	assert!(reopens > now + SignedDuration::from_mins(59), "{reopens}");
}

#[tokio::test]
async fn a_scan_a_member_asks_for_is_theirs_alone_to_pay() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest00000asked").await;
	let a = person(archive.store().unwrap(), "a").await;
	person(archive.store().unwrap(), "b").await;
	let g = conductor.tracks(&archive, "a", &t).await;
	conductor.tracks(&archive, "b", &t).await;
	conductor.set(&archive, "a", 100).await;
	conductor.set(&archive, "b", 100).await;

	let job = archive.scan_tracked(a, g, t.id).await.unwrap();
	assert_eq!(archive.store().unwrap().claim_job(Timestamp::now()).await.unwrap().unwrap().id, job);
	let blobs = BlobStore::new(dir.path().join("blobs"));
	recorder(&archive, &blobs).record(&Spends(10), &t, Some(job)).await.unwrap();
	assert_eq!(holds(&archive, "a").await, 90);
	assert_eq!(holds(&archive, "b").await, 100);
}

#[tokio::test]
async fn a_member_out_of_tokens_cannot_ask_for_a_scan() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest000000poor").await;
	let broke = person(archive.store().unwrap(), "broke").await;
	let g = conductor.tracks(&archive, "broke", &t).await;
	conductor.set(&archive, "broke", 0).await;
	let refused = archive.scan_tracked(broke, g, t.id).await.unwrap_err();
	assert!(matches!(refused.downcast_ref::<Rejected>(), Some(Rejected::Busy(_))), "{refused:#}");
}

#[tokio::test]
async fn a_scan_a_member_asks_for_leaves_the_schedule_alone() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest00schedule").await;
	let m = person(archive.store().unwrap(), "m").await;
	let g = conductor.tracks(&archive, "m", &t).await;
	conductor.set(&archive, "m", 100).await;
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
	let conductor = Conductor::new();
	let m = person(archive.store().unwrap(), "m").await;
	let g = conductor.tracks(&archive, "m", &t).await;
	conductor.set(&archive, "m", 100).await;
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
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest000kickback").await;
	person(archive.store().unwrap(), "a").await;
	conductor.tracks(&archive, "a", &t).await;
	conductor.set(&archive, "a", 100).await;

	archive.record(&Warns(20, Warning::Failure("screenshot of x failed".into())), &t).await.unwrap();
	assert_eq!(holds(&archive, "a").await, 80, "charged all the same");
	archive.record(&Warns(10, Warning::Limit("out of tokens".into())), &t).await.unwrap();
	archive.record(&BreaksAfter(20), &t).await.unwrap();
	let kicked: Vec<_> = feed(&archive).await.iter().map(|c| (c.spent, c.kick_back)).collect();
	assert_eq!(kicked, [(20, true), (10, false), (20, true)], "a limit is no failure");
}

/// A purged place leaves the charges of its runs in the feed.
#[tokio::test]
async fn a_purged_places_charges_stay_in_the_feed() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest00000purged").await;
	person(archive.store().unwrap(), "a").await;
	let gmb = conductor.tracks(&archive, "a", &t).await;
	conductor.set(&archive, "a", 100).await;
	archive.record(&Spends(20), &t).await.unwrap();
	conductor.remove(&archive, gmb).await;

	archive.purge(Timestamp::now() + SignedDuration::from_hours(24 * 7)).await.unwrap();
	assert!(archive.store().unwrap().targets().await.unwrap().is_empty());
	assert_eq!(feed(&archive).await.iter().map(|c| c.spent).collect::<Vec<_>>(), [20]);
	assert_eq!(holds(&archive, "a").await, 80);
}

/// The browser's work is in the usage feed once per run, whoever pays, a failed run's too.
#[tokio::test]
async fn every_walk_is_used_whoever_pays() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let paid = target(&archive, "ChIJtokenstest000000paid").await;
	let operators = target(&archive, "ChIJtokenstest00operator").await;
	person(archive.store().unwrap(), "a").await;
	conductor.tracks(&archive, "a", &paid).await;
	conductor.set(&archive, "a", 100).await;

	archive.record(&Opens(3), &paid).await.unwrap();
	archive.record(&BreaksAfter(2), &operators).await.unwrap();
	archive.record(&Fails, &operators).await.unwrap();
	assert_eq!(feed(&archive).await.len(), 1, "only the paid walk is charged");

	let used = archive.usage(0, 500).await.unwrap();
	let rows: Vec<_> = used.used.iter().map(|u| (u.item, u.units)).collect();
	let walk = |s| Item::Walk(s);
	assert_eq!(
		rows,
		[
			(walk(WalkStep::Open), 1),
			(walk(WalkStep::Reviews), 1),
			(walk(WalkStep::Sort), 1),
			(walk(WalkStep::Step), 3),
			(walk(WalkStep::Step), 2)
		]
	);
	let ended: Vec<_> = archive.runs(paid.id, &Default::default()).await.unwrap().iter().map(|r| r.finished_at.clone()).collect();
	assert_eq!(
		used.used[..4].iter().map(|u| Some(fmt_ts(u.at))).collect::<Vec<_>>(),
		vec![ended[0].clone(); 4],
		"dated at the run's end"
	);

	let first = archive.usage(0, 4).await.unwrap();
	assert_eq!((first.used.as_slice(), first.next), (&used.used[..4], used.used[3].id));
	let rest = archive.usage(first.next, 4).await.unwrap();
	assert_eq!((rest.used.as_slice(), rest.next), (&used.used[4..], used.used[4].id));
	let none = archive.usage(rest.next, 4).await.unwrap();
	assert_eq!((none.used.len(), none.next), (0, rest.next));
}

fn usage_rows(page: &review_archive::core::dto::UsagePage) -> Vec<(Item, i64)> {
	page.used.iter().map(|u| (u.item, u.units)).collect()
}

fn opened_and_scrolled(n: i64) -> Vec<(Item, i64)> {
	[(WalkStep::Open, 1), (WalkStep::Reviews, 1), (WalkStep::Sort, 1), (WalkStep::Step, n)]
		.map(|(s, n)| (Item::Walk(s), n))
		.to_vec()
}

/// A walk its process died in is counted as far as it was kept, once the next start fails its run, and only once.
#[tokio::test]
async fn a_walk_a_restart_cut_off_is_used_once() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	Conductor::new().push(&archive).await;
	let t = target(&archive, "ChIJtokenstest00000killed").await;
	let dies = Gated {
		before: 2,
		gate: Notify::new(),
		after: 0,
	};
	tokio::time::timeout(PAST_A_CHECKPOINT, archive.record(&dies, &t)).await.expect_err("the gate never opens");
	assert!(archive.usage(0, 500).await.unwrap().used.is_empty(), "nothing is final while the run is open");
	archive.close().await;

	let archive = open(dir.path()).await;
	archive.recover().await.unwrap();
	let used = archive.usage(0, 500).await.unwrap();
	assert_eq!(usage_rows(&used), opened_and_scrolled(2));
	archive.recover().await.unwrap();
	assert_eq!(archive.usage(0, 500).await.unwrap().used, used.used, "a second start counts it again");
}

/// A walk that outlives its checkpoints is counted at its end, as it ended, once.
#[tokio::test]
async fn a_checkpointed_walk_is_used_once_at_its_end() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	Conductor::new().push(&archive).await;
	let t = target(&archive, "ChIJtokenstest00000longer").await;
	let walk = Gated {
		before: 2,
		gate: Notify::new(),
		after: 3,
	};
	let (recorded, ()) = tokio::join!(archive.record(&walk, &t), async {
		tokio::time::sleep(PAST_A_CHECKPOINT).await;
		walk.gate.notify_one();
	});
	recorded.unwrap();
	archive.recover().await.unwrap();
	assert_eq!(usage_rows(&archive.usage(0, 500).await.unwrap()), opened_and_scrolled(5));
}

/// A hand-run walk still going when `serve` starts is counted as the start found it; its own end adds nothing to what was published.
#[tokio::test]
async fn a_walk_that_outlives_the_sweep_is_used_once_as_swept() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	Conductor::new().push(&archive).await;
	let t = target(&archive, "ChIJtokenstest0000outlive").await;
	let walk = Gated {
		before: 2,
		gate: Notify::new(),
		after: 3,
	};
	let (recorded, swept) = tokio::join!(archive.record(&walk, &t), async {
		tokio::time::sleep(PAST_A_CHECKPOINT).await;
		archive.recover().await.unwrap();
		let swept = archive.usage(0, 500).await.unwrap();
		walk.gate.notify_one();
		swept
	});
	recorded.unwrap();
	assert_eq!(usage_rows(&swept), opened_and_scrolled(2));
	assert_eq!(archive.usage(0, 500).await.unwrap().used, swept.used);
}

#[tokio::test]
async fn nothing_walks_unpriced_nor_past_the_hour() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let conductor = Conductor::new();
	let t = target(&archive, "ChIJtokenstest00unpriced").await;
	let m = person(archive.store().unwrap(), "m").await;
	let now = Timestamp::now();
	let closed = |e: eyre::Report| match e.downcast_ref::<Rejected>() {
		Some(Rejected::Busy(why)) => why.clone(),
		_ => panic!("{e:#}"),
	};
	assert!(archive.due(now).await.unwrap().is_empty(), "before the first snapshot nothing is priced");
	assert!(closed(archive.enqueue_scan(t.id).await.unwrap_err()).contains("not priced"));

	conductor.0.borrow_mut().prices.remove(&Item::Walk(WalkStep::SortRetry));
	let g = conductor.tracks(&archive, "m", &t).await;
	conductor.set(&archive, "m", 100).await;
	assert!(archive.due(now).await.unwrap().is_empty());
	assert!(closed(archive.scan_tracked(m, g, t.id).await.unwrap_err()).contains("\"Sort, again\" has no price"));
	assert_eq!(card(&archive, m, t.id).await.scan_cost, None);

	conductor.0.borrow_mut().prices = priced::snapshot().prices;
	conductor.push(&archive).await;
	assert_eq!(archive.due(now).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [t.id]);
	assert_eq!(card(&archive, m, t.id).await.scan_cost, Some(15), "a place never walked costs its first screen");

	conductor.0.borrow_mut().prices.insert(WalkStep::Open.into(), Tokens::default().per_hour);
	conductor.push(&archive).await;
	assert!(archive.due(now).await.unwrap().is_empty());
	assert!(closed(archive.enqueue_scan(t.id).await.unwrap_err()).contains("more than tokens.per_hour"));
}

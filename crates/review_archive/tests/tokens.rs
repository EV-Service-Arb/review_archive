//! Members pay for the walks of the places they track: what they hold is the conductor's
//! balance less the charges the feed holds past its cursor, a walk's charge goes into the feed
//! with the run's end, and no walk for a place whose trackers are out of tokens, nor past the
//! account's hourly limit.

use std::{cell::RefCell, path::Path, sync::Mutex};

use jiff::{SignedDuration, Timestamp};
use review_archive::{
	Archive,
	config::Config,
	core::{
		Coverage, Known, PersonId, Rejected, Scan, Target, TargetId, Warning,
		dto::{ChargeDto, GmbSnapshot, LocationSummary, MirroredGmb, NewTarget, ScanState},
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
		.flat_map(|g| g.gmbs)
		.flat_map(|b| b.tracks)
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

/// What the conductor pushed last.
struct Conductor(RefCell<GmbSnapshot>);

impl Conductor {
	fn new() -> Self {
		Self(RefCell::new(GmbSnapshot {
			gmbs: vec![],
			balances: Default::default(),
			charges_through: 0,
		}))
	}

	async fn push(&self, archive: &Archive) {
		let snap = self.0.borrow().clone();
		archive.apply_snapshot(&snap).await.unwrap();
	}

	/// `sub` tracks `t` under a GMB of their own; returns the GMB.
	async fn tracks(&self, archive: &Archive, sub: &str, t: &Target) -> i64 {
		let id = {
			let mut snap = self.0.borrow_mut();
			let id = snap.gmbs.len() as i64 + 1;
			snap.gmbs.push(MirroredGmb {
				id,
				gmail_id: id,
				gmail: format!("{sub}.ops"),
				owner: sub.into(),
				removed: false,
				enabled: true,
				place_id: t.place_id.clone(),
			});
			id
		};
		self.push(archive).await;
		archive.assign(id, t.id).await.unwrap();
		id
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
			snap.balances.insert(sub.into(), n);
			snap.charges_through = through;
		}
		self.push(archive).await;
	}
}

async fn holds(archive: &Archive, sub: &str) -> i64 {
	archive.store().unwrap().holdings(&[sub.into()]).await.unwrap()
}

async fn feed(archive: &Archive) -> Vec<ChargeDto> {
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
		(
			format!("run:{}", rec.run.0).as_str(),
			["poor".to_owned(), "rich".to_owned()].as_slice(),
			20,
			false,
			t.place_id.as_str()
		)
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
	assert!(gmb.tracks[0].enabled, "the track's own switch stays the member's");
	assert_eq!(archive.my_gmbs(off).await.unwrap().places_tracked, 0);

	conductor.switch(&archive, on_gmb, true).await;
	assert_eq!(archive.due(now).await.unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [t.id]);
	archive.record(&Spends(20), &t).await.unwrap();
	assert_eq!(feed(&archive).await[0].payers, ["on"]);
	assert_eq!(holds(&archive, "off").await, 100);
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
	let a = person(archive.store().unwrap(), "a").await;
	let gmb = conductor.tracks(&archive, "a", &t).await;
	conductor.set(&archive, "a", 100).await;
	archive.record(&Spends(20), &t).await.unwrap();
	archive.untrack(a, gmb, t.id).await.unwrap();

	archive.purge_untracked(Timestamp::now() + SignedDuration::from_hours(24 * 7)).await.unwrap();
	assert!(archive.store().unwrap().targets().await.unwrap().is_empty());
	assert_eq!(feed(&archive).await.iter().map(|c| c.spent).collect::<Vec<_>>(), [20]);
	assert_eq!(holds(&archive, "a").await, 80);
}

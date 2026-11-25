//! Whether reviews stick: of those a place's archive watched from their first listing, the share still
//! listed some days after. A review that came back counts as never gone.

use std::collections::{BTreeMap, HashMap};

use jiff::{SignedDuration, Timestamp, civil::Date, tz::TimeZone};

use crate::{
	TargetId,
	dto::{Stick, StickGmb, StickGroup, StickPoint, StickReport},
	relative_date,
};

/// The ages, in days, a rate is told at.
const HORIZONS: [u32; 7] = [1, 3, 7, 14, 30, 60, 90];

const BUCKETS: [&str; 4] = ["1", "2–4", "5–9", "10+"];

/// A GMB's place, and the time its scans watched it.
#[derive(Clone, Debug)]
pub struct Tracked {
	/// The conductor's id of the GMB.
	pub gmb: i64,
	/// Its name.
	pub name: String,
	/// Its place.
	pub target: TargetId,
	/// When the place's first scan began.
	pub from: Timestamp,
	/// When its latest ended: a listed review is known to have stayed until then, no longer.
	pub to: Timestamp,
}

/// An archived review of a tracked place.
#[derive(Clone, Debug)]
pub struct Watched {
	/// Its place.
	pub target: TargetId,
	/// Its stars.
	pub stars: Option<i64>,
	/// See [`crate::KnownReview`].
	pub published_raw: Option<String>,
	/// See [`crate::KnownReview`].
	pub published_est: Option<Timestamp>,
	/// When a scan first listed it.
	pub first_seen: Timestamp,
	/// Set while it is gone.
	pub gone_at: Option<Timestamp>,
}

/// The stick rates of these GMBs' places.
pub fn report(tracked: &[Tracked], reviews: &[Watched]) -> StickReport {
	let places: HashMap<TargetId, &Tracked> = tracked.iter().map(|t| (t.target, t)).collect();
	let lives = lives(&places, reviews);
	let grouped = |key: &dyn Fn(&Life) -> String| {
		let mut groups: BTreeMap<String, Vec<&Life>> = BTreeMap::new();
		for l in &lives {
			groups.entry(key(l)).or_default().push(l);
		}
		groups
	};
	let named = |groups: BTreeMap<String, Vec<&Life>>, order: &[&str]| {
		order
			.iter()
			.filter_map(|name| {
				groups.get(*name).map(|ls| StickGroup {
					name: (*name).to_owned(),
					stick: stick(ls.iter().copied()),
				})
			})
			.collect::<Vec<_>>()
	};
	StickReport {
		overall: stick(lives.iter()),
		by_stars: named(grouped(&|l| l.stars.map_or("unrated".to_owned(), |s| s.to_string())), &["5", "4", "3", "2", "1", "unrated"]),
		by_burst: named(grouped(&|l| bucket(l.burst).to_owned()), &BUCKETS),
		by_week: grouped(&|l| week(l.seen).to_string())
			.into_iter()
			.map(|(name, ls)| StickGroup { name, stick: stick(ls.into_iter()) })
			.collect(),
		gmbs: tracked
			.iter()
			.map(|t| StickGmb {
				gmb: t.gmb,
				name: t.name.clone(),
				stick: stick(lives.iter().filter(|l| l.target == t.target)),
			})
			.collect(),
	}
}

/// A counted review: how long it was watched, and whether it was lost then.
struct Life {
	target: TargetId,
	stars: Option<i64>,
	seen: Date,
	/// How many of its place's counted reviews were posted the day it was.
	burst: usize,
	watched: SignedDuration,
	lost: bool,
}

/// The reviews posted since their place's first scan, each with how long it was watched.
fn lives(places: &HashMap<TargetId, &Tracked>, reviews: &[Watched]) -> Vec<Life> {
	let mut per_day: HashMap<(TargetId, Date), usize> = HashMap::new();
	let mut counted = Vec::new();
	for r in reviews {
		let place = places.get(&r.target).expect("the reviews are the tracked places'");
		let (Some(raw), Some(est)) = (r.published_raw.as_deref(), r.published_est) else { continue };
		if relative_date::lower_bound(raw, est).is_none_or(|earliest| earliest < place.from) {
			continue;
		}
		let day = est.to_zoned(TimeZone::UTC).date();
		*per_day.entry((r.target, day)).or_default() += 1;
		counted.push((r, place, day));
	}
	counted
		.into_iter()
		.map(|(r, place, day)| {
			let end = r.gone_at.unwrap_or(place.to);
			Life {
				target: r.target,
				stars: r.stars,
				seen: r.first_seen.to_zoned(TimeZone::UTC).date(),
				burst: per_day[&(r.target, day)],
				watched: r.first_seen.duration_until(end).max(SignedDuration::ZERO), // an ad-hoc capture can see one no scan has yet
				lost: r.gone_at.is_some(),
			}
		})
		.collect()
}

/// The Kaplan–Meier estimate at each horizon: one still listed counts as at risk for as long as it was watched.
fn stick<'a>(lives: impl Iterator<Item = &'a Life>) -> Stick {
	let lives: Vec<&Life> = lives.collect();
	let mut ages: Vec<SignedDuration> = lives.iter().map(|l| l.watched).collect();
	ages.sort_unstable();
	let watched_for = |age: SignedDuration| ages.len() - ages.partition_point(|a| *a < age);
	let lost: BTreeMap<SignedDuration, usize> = lives.iter().filter(|l| l.lost).fold(BTreeMap::new(), |mut at, l| {
		*at.entry(l.watched).or_default() += 1;
		at
	});
	let points = HORIZONS
		.iter()
		.map(|&days| {
			let horizon = SignedDuration::from_hours(24 * i64::from(days));
			let rate = lost.range(..=horizon).fold(1., |s, (age, n)| s * (1. - *n as f64 / watched_for(*age) as f64));
			let watched = watched_for(horizon);
			StickPoint {
				days,
				rate: (watched > 0 || (rate == 0. && !lives.is_empty())).then_some(rate),
				watched: u32::try_from(watched).expect("a place's reviews fit u32"),
			}
		})
		.collect();
	Stick {
		posted: u32::try_from(lives.len()).expect("a place's reviews fit u32"),
		lost: u32::try_from(lives.iter().filter(|l| l.lost).count()).expect("counted above"),
		points,
	}
}

fn bucket(posts: usize) -> &'static str {
	match posts {
		..=1 => BUCKETS[0],
		2..=4 => BUCKETS[1],
		5..=9 => BUCKETS[2],
		10.. => BUCKETS[3],
	}
}

/// The Monday of its ISO week.
fn week(day: Date) -> Date {
	day - jiff::Span::new().days(i64::from(day.weekday().to_monday_zero_offset()))
}

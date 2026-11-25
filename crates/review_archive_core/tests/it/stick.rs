//! What sticks: survival over the reviews a place's archive watched from their first listing.

use jiff::{SignedDuration, Timestamp};
use review_archive_core::{
	TargetId,
	dto::{Stick, StickPoint},
	stick::{self, Tracked, Watched},
};

const HOME: &str = "2026-09-01T00:00:00Z";

fn day(n: i64) -> Timestamp {
	HOME.parse::<Timestamp>().unwrap() + SignedDuration::from_hours(24 * n)
}

/// Posted and first seen on day `seen`, gone on day `gone`; its place was watched days 0 to 40.
fn watched(target: i64, stars: i64, seen: i64, gone: Option<i64>) -> Watched {
	Watched {
		target: TargetId(target),
		stars: Some(stars),
		published_raw: Some(day(seen).to_string()),
		published_est: Some(day(seen)),
		first_seen: day(seen),
		gone_at: gone.map(day),
	}
}

fn tracked(gmb: i64, target: i64) -> Tracked {
	Tracked {
		gmb,
		name: format!("GMB {gmb}"),
		target: TargetId(target),
		from: day(0),
		to: day(40),
	}
}

fn at(s: &Stick, days: u32) -> (Option<f64>, u32) {
	let p: &StickPoint = s.points.iter().find(|p| p.days == days).unwrap();
	(p.rate.map(|r| (r * 1000.).round() / 1000.), p.watched)
}

#[test]
fn young_reviews_count_for_the_days_they_have_had() {
	let reviews = [
		watched(1, 5, 1, None),     // 39 days
		watched(1, 5, 2, Some(4)),  // lost at 2 days
		watched(1, 5, 3, Some(13)), // lost at 10 days
		watched(1, 5, 35, None),    // 5 days so far
	];
	let s = stick::report(&[tracked(1, 1)], &reviews).overall;
	assert_eq!((s.posted, s.lost), (4, 2));
	assert_eq!(at(&s, 1), (Some(1.), 4));
	assert_eq!(at(&s, 3), (Some(0.75), 3));
	// the one 5 days old is not among those at risk at 10
	assert_eq!(at(&s, 14), (Some(0.375), 1));
	assert_eq!(at(&s, 30), (Some(0.375), 1));
	assert_eq!(at(&s, 60), (None, 0), "nobody was watched that long");
}

#[test]
fn a_review_that_may_predate_the_first_scan_is_not_counted() {
	let mut backlog = watched(1, 5, 1, Some(3));
	backlog.published_raw = Some("a month ago".into()); // a month back from day 1 or up to two
	let mut undated = watched(1, 5, 1, Some(3));
	(undated.published_raw, undated.published_est) = (None, None);
	let s = stick::report(&[tracked(1, 1)], &[backlog, undated]).overall;
	assert_eq!((s.posted, s.lost), (0, 0));
	assert!(s.points.iter().all(|p| p.rate.is_none()));
}

#[test]
fn a_place_two_gmbs_track_is_counted_once_but_shown_under_each() {
	let reviews = [watched(1, 5, 1, Some(2)), watched(1, 4, 1, None)];
	let r = stick::report(&[tracked(7, 1), tracked(8, 1)], &reviews);
	assert_eq!(r.overall.posted, 2);
	assert_eq!(r.gmbs.iter().map(|g| (g.gmb, g.stick.posted)).collect::<Vec<_>>(), [(7, 2), (8, 2)]);
}

#[test]
fn groups_split_by_stars_by_posts_a_day_and_by_week() {
	let reviews = [
		watched(1, 5, 1, Some(2)), // day 1: three on one day at place 1
		watched(1, 5, 1, None),
		watched(1, 4, 1, None),
		watched(1, 3, 9, None), // alone on day 9
	];
	let r = stick::report(&[tracked(1, 1)], &reviews);
	let of = |groups: &[review_archive_core::dto::StickGroup]| groups.iter().map(|g| (g.name.clone(), g.stick.posted, g.stick.lost)).collect::<Vec<_>>();
	assert_eq!(of(&r.by_stars), [("5".into(), 2, 1), ("4".into(), 1, 0), ("3".into(), 1, 0)]);
	assert_eq!(of(&r.by_burst), [("1".into(), 1, 0), ("2–4".into(), 3, 1)]);
	// 2026-09-02 is a Wednesday, 2026-09-10 a Thursday
	assert_eq!(of(&r.by_week), [("2026-08-31".into(), 3, 1), ("2026-09-07".into(), 1, 0)]);
}

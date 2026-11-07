//! What a member does: their GMBs' tracks, appeals, settings. The member is who
//! the caller authenticated as; everything is scoped to them in the store. GMBs and balances
//! are the conductor's: it pushes them here, and follows what walks cost and the Alerts feed.

use jiff::Timestamp;
use review_archive_core::{
	PersonId, Rejected, ReviewId, TargetId,
	dto::{Board, Charges, GbpState, GmailOverview, GmbSnapshot, JobKind, MAX_PAGE, MemberDto, MyGmbs, ReinstatementDto, Settings, SettingsPatch, TargetDto, TrackPatch},
	maps::cost,
	parse_interval,
};
use sa_alerts::NotePage;

use super::Archive;
use crate::store::people::{Claim, Seen};

impl Archive {
	/// The person the panel vouches for; see [`Store::person`](crate::store::Store::person).
	pub async fn person(&self, seen: &Seen<'_>) -> eyre::Result<Claim> {
		self.store()?.person(seen, Timestamp::now()).await
	}

	/// Everyone, with what they hold.
	pub async fn members(&self) -> eyre::Result<Vec<MemberDto>> {
		let mut out = Vec::new();
		for p in self.store()?.people().await? {
			out.push(MemberDto {
				balance: self.store()?.holdings(p.sub.as_slice()).await?,
				id: p.id,
				email: p.email,
				name: p.name,
				claimed: p.sub.is_some(),
			});
		}
		Ok(out)
	}

	/// [`Rejected::NotFound`] for an id nobody has.
	pub async fn check_person(&self, id: PersonId) -> eyre::Result<()> {
		match self.store()?.person_by_id(id).await? {
			Some(_) => Ok(()),
			None => Err(Rejected::not_found(format!("no person {id}")).into()),
		}
	}

	/// Changes the member's GMB's track of a place: on or off, favorite or not.
	pub async fn patch_track(&self, member: PersonId, gmb: i64, target: TargetId, patch: &TrackPatch) -> eyre::Result<()> {
		self.store()?.patch_track(member, gmb, target, patch).await
	}

	/// Asks Google about a tracked place again and takes its id, name and address as Google
	/// gives them now. Refused when the newer id is another target's already.
	pub async fn relink(&self, member: PersonId, gmb: i64, target: TargetId) -> eyre::Result<TargetDto> {
		let store = self.store()?;
		store.check_track(member, gmb, target).await?;
		if self.inner.secrets.google_maps_keys.is_empty() {
			return Err(Rejected::invalid("re-linking a place needs GOOGLE_MAPS_KEYS").into());
		}
		let t = store.target(target).await?;
		let place = self.stored()?.google.details(&t.place_id, sa_places::Ask::Afresh).await.map_err(crate::failure::places_error)?;
		if place.place_id != t.place_id
			&& let Some(other) = store.find_target(&place.place_id, &t.lang, t.gbp.as_ref()).await?
		{
			return Err(Rejected::invalid(format!("Google now knows {t} as {}, which is target {} already", place.place_id, other.id)).into());
		}
		store.relink(target, &place).await?;
		Ok(store.target(target).await?.into())
	}

	/// The member's gmails, their GMBs, and each one's places as tracked.
	pub async fn overview(&self, member: PersonId) -> eyre::Result<Vec<GmailOverview>> {
		self.store()?.overview(member, Timestamp::now()).await
	}

	/// The member's GMBs' tags for the conductor's page.
	pub async fn my_gmbs(&self, member: PersonId) -> eyre::Result<MyGmbs> {
		self.store()?.my_gmbs(member).await
	}

	/// Queues a scan of a place the member's GMB tracks, on top of its schedule; the member
	/// pays for it. Returns the job id.
	pub async fn scan_tracked(&self, member: PersonId, gmb: i64, target: TargetId) -> eyre::Result<i64> {
		let store = self.store()?;
		store.check_track(member, gmb, target).await?;
		match store.gmb_switch(gmb).await? {
			(false, _) => return Err(Rejected::Busy(format!("GMB {gmb} is switched off")).into()),
			(true, GbpState::Verified) => {}
			(true, state) => return Err(Rejected::Busy(format!("GMB {gmb} is {}", state.as_ref())).into()),
		}
		let now = Timestamp::now();
		#[cfg(feature = "maps")]
		if let Some(why) = self.closed(store.target(target).await?.kind, true, now).await? {
			return Err(Rejected::Busy(why).into());
		}
		let sub = store.person_by_id(member).await?.and_then(|p| p.sub).expect("check_track matched their GMB by their sub");
		if store.holdings(&[sub]).await? < cost::FIRST_SCREEN {
			return Err(Rejected::Busy("you are out of tokens".into()).into());
		}
		store.enqueue_job(JobKind::Scan, target, None, Some(member), self.inner.defaults.max_queued_jobs, now).await
	}

	/// A tracked place's reviews: snapshotted, suspended, removed, reinstating.
	pub async fn board(&self, member: PersonId, gmb: i64, target: TargetId) -> eyre::Result<Board> {
		self.store()?.board(member, gmb, target).await
	}

	/// Records that the review's reinstatement was asked of Google, now.
	pub async fn reinstate(&self, member: PersonId, gmb: i64, review: ReviewId) -> eyre::Result<ReinstatementDto> {
		self.store()?.reinstate(member, gmb, review, Timestamp::now()).await
	}

	/// Withdraws the open appeal; it stays on record.
	pub async fn withdraw_reinstatement(&self, member: PersonId, gmb: i64, review: ReviewId) -> eyre::Result<()> {
		self.store()?.withdraw_reinstatement(member, gmb, review, Timestamp::now()).await
	}

	/// A capture's WebP, if it shows a review of a place the member tracks.
	pub async fn member_capture_webp(&self, member: PersonId, sha256: &str) -> eyre::Result<Vec<u8>> {
		if !self.store()?.member_sees_capture(member, sha256).await? {
			return Err(Rejected::not_found("no such capture").into());
		}
		self.capture_webp(sha256).await
	}

	/// What the member chose, beside the defaults.
	pub async fn settings(&self, member: PersonId) -> eyre::Result<Settings> {
		self.store()?.settings(member).await
	}

	/// Chooses what the patch names.
	pub async fn patch_settings(&self, member: PersonId, patch: &SettingsPatch) -> eyre::Result<Settings> {
		let interval = patch.interval.as_deref().map(parse_interval).transpose()?;
		if let Some(i) = interval {
			self.check_interval(i)?;
		}
		let store = self.store()?;
		store.choose(member, interval).await?;
		store.settings(member).await
	}

	/// Every setting of the member's back to its default.
	pub async fn reset_settings(&self, member: PersonId) -> eyre::Result<Settings> {
		let store = self.store()?;
		store.reset_settings(member).await?;
		store.settings(member).await
	}

	/// The conductor's snapshot of GMBs and balances; see [`Store::apply_snapshot`](crate::store::Store::apply_snapshot).
	pub async fn apply_snapshot(&self, snap: &GmbSnapshot) -> eyre::Result<()> {
		self.store()?.apply_snapshot(snap, Timestamp::now()).await
	}

	/// The charge feed past `after`, `limit` at most.
	pub async fn charges(&self, after: i64, limit: u32) -> eyre::Result<Charges> {
		page(limit)?;
		self.store()?.charges(after, limit).await
	}

	/// The Alerts feed past `after`, `limit` at most; its notes kept a day.
	pub async fn notifications(&self, after: i64, limit: u32) -> eyre::Result<NotePage> {
		page(limit)?;
		self.store()?.notes(after, limit).await
	}
}

/// A feed's page holds 1 to [`MAX_PAGE`].
fn page(limit: u32) -> Result<(), Rejected> {
	match (1..=MAX_PAGE).contains(&limit) {
		true => Ok(()),
		false => Err(Rejected::invalid(format!("limit is 1 to {MAX_PAGE}, not {limit}"))),
	}
}

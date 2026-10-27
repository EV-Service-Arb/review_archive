//! What a member does: their gmails, what those track, appeals, Telegram channels. The
//! member is who the caller authenticated as; everything is scoped to them in the store.

use jiff::Timestamp;
use review_archive_core::{
	GbpLocation, PersonId, Rejected, ReviewId, TargetId, check_lang,
	dto::{
		Board, GmailDto, GmailOverview, JobKind, LedgerEntry, MemberDto, NewGmail, NewTgChannel, NewTrack, PlaceDto, ReinstatementDto, Settings, SettingsPatch, TargetDto, TargetPatch,
		TgChannelDto, TokensChange, TokensDto, TrackPatch, Usage,
	},
	maps::cost,
	parse_interval,
};
use tg_types::TelegramDestination;

use super::Archive;
use crate::store::people::{Claim, Seen};

impl Archive {
	/// The person the panel vouches for; see [`Store::person`](crate::store::Store::person).
	pub async fn person(&self, seen: &Seen<'_>) -> eyre::Result<Claim> {
		self.store()?.person(seen, Timestamp::now()).await
	}

	/// Everyone, with their balances.
	pub async fn members(&self) -> eyre::Result<Vec<MemberDto>> {
		let mut out = Vec::new();
		for p in self.store()?.people().await? {
			out.push(MemberDto {
				balance: self.store()?.balance(PersonId(p.id), Timestamp::now(), &self.inner.tokens).await?,
				id: p.id,
				email: p.email,
				name: p.name,
				claimed: p.claimed,
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

	/// Adds a managing gmail to the member.
	pub async fn add_gmail(&self, member: PersonId, req: &NewGmail) -> eyre::Result<GmailDto> {
		let gmail = req.gmail.trim().to_lowercase();
		if gmail.is_empty() || gmail.contains(char::is_whitespace) {
			return Err(Rejected::invalid(format!("{:?} is not an address or an alias for one", req.gmail)).into());
		}
		self.store()?.add_gmail(member, &gmail, Timestamp::now()).await
	}

	/// Removes a gmail with its tracks and the channels scoped to it; one with appeals on
	/// record stays.
	pub async fn delete_gmail(&self, member: PersonId, id: i64) -> eyre::Result<()> {
		self.store()?.delete_gmail(member, id, Timestamp::now()).await
	}

	/// Every place a pasted id or Maps URL may mean, to pick one to track by its id.
	pub async fn places(&self, input: &str) -> eyre::Result<Vec<PlaceDto>> {
		crate::places::candidates(&self.inner.http, self.inner.secrets.google_maps_key.as_deref(), input).await
	}

	/// Tracks a place under the member's gmail: the target on that place, language and
	/// source if there is one — enabled, if it was not — else a new one.
	pub async fn track(&self, member: PersonId, gmail: i64, req: &NewTrack) -> eyre::Result<TargetDto> {
		let store = self.store()?;
		store.check_gmail(Some(member), gmail).await?;
		let lang = req.lang.clone().unwrap_or_else(|| self.inner.defaults.lang.clone());
		check_lang(&lang)?;
		let gbp = req.gbp.as_deref().map(str::parse::<GbpLocation>).transpose()?;
		let (place_id, resolved) = crate::places::resolve(&self.inner.http, self.inner.secrets.google_maps_key.as_deref(), &req.place).await?;
		let target = match store.find_target(&place_id, &lang, gbp.as_ref()).await? {
			Some(t) if t.enabled => t,
			Some(t) =>
				self.update_target(
					t.id,
					&TargetPatch {
						enabled: Some(true),
						..Default::default()
					},
				)
				.await?,
			None => self.insert_target(place_id, resolved.as_ref(), req.label.clone(), Some(lang), None, gbp, true).await?,
		};
		store.track(Some(member), gmail, target.id, Timestamp::now()).await?;
		Ok(target.into())
	}

	/// The operator puts a target under any gmail.
	pub async fn assign(&self, gmail: i64, target: TargetId) -> eyre::Result<()> {
		self.store()?.track(None, gmail, target, Timestamp::now()).await
	}

	/// Stops the member's gmail tracking a target; the target and its archive stay until `defaults.untracked_retention` after no gmail tracks it.
	pub async fn untrack(&self, member: PersonId, gmail: i64, target: TargetId) -> eyre::Result<()> {
		self.store()?.untrack(member, gmail, target, Timestamp::now()).await
	}

	/// Switches the member's gmail on or off: off, its places are scanned only if someone else has them on.
	pub async fn set_gmail_enabled(&self, member: PersonId, gmail: i64, on: bool) -> eyre::Result<()> {
		self.store()?.set_gmail_enabled(member, gmail, on).await
	}

	/// Changes the member's gmail's track of a place: on or off, favorite or not.
	pub async fn patch_track(&self, member: PersonId, gmail: i64, target: TargetId, patch: &TrackPatch) -> eyre::Result<()> {
		self.store()?.patch_track(member, gmail, target, patch).await
	}

	/// Asks Google about a tracked place again and takes its id, name and address as Google
	/// gives them now. Refused when the newer id is another target's already.
	pub async fn relink(&self, member: PersonId, gmail: i64, target: TargetId) -> eyre::Result<TargetDto> {
		let store = self.store()?;
		store.check_track(member, gmail, target).await?;
		let key = self
			.inner
			.secrets
			.google_maps_key
			.as_deref()
			.ok_or_else(|| Rejected::invalid("re-linking a place needs GOOGLE_MAPS_KEY"))?;
		let t = store.target(target).await?;
		let place = crate::places::details(&self.inner.http, crate::places::PLACES, key, &t.place_id).await?;
		if place.place_id != t.place_id
			&& let Some(other) = store.find_target(&place.place_id, &t.lang, t.gbp.as_ref()).await?
		{
			return Err(Rejected::invalid(format!("Google now knows {t} as {}, which is target {} already", place.place_id, other.id)).into());
		}
		store.relink(target, &place).await?;
		Ok(store.target(target).await?.into())
	}

	/// The member's gmails and each one's places.
	pub async fn overview(&self, member: PersonId) -> eyre::Result<Vec<GmailOverview>> {
		self.store()?.overview(member, Timestamp::now(), &self.inner.tokens).await
	}

	/// Queues a scan of a place the member's gmail tracks, on top of its schedule; the member
	/// pays for it. Returns the job id.
	pub async fn scan_tracked(&self, member: PersonId, gmail: i64, target: TargetId) -> eyre::Result<i64> {
		let store = self.store()?;
		store.check_track(member, gmail, target).await?;
		let now = Timestamp::now();
		#[cfg(feature = "maps")]
		if let Some(why) = self.closed(store.target(target).await?.kind, now).await? {
			return Err(Rejected::Busy(why).into());
		}
		if store.held_together(&[member], now, &self.inner.tokens).await? < cost::FIRST_SCREEN {
			return Err(Rejected::Busy("you are out of tokens".into()).into());
		}
		store.enqueue_job(JobKind::Scan, target, None, Some(member), self.inner.defaults.max_queued_jobs, now).await
	}

	/// A tracked place's reviews: snapshotted, removed, reinstating.
	pub async fn board(&self, member: PersonId, gmail: i64, target: TargetId) -> eyre::Result<Board> {
		self.store()?.board(member, gmail, target).await
	}

	/// Records that the review's reinstatement was asked of Google, now.
	pub async fn reinstate(&self, member: PersonId, gmail: i64, review: ReviewId) -> eyre::Result<ReinstatementDto> {
		self.store()?.reinstate(member, gmail, review, Timestamp::now()).await
	}

	/// Withdraws the open appeal; it stays on record.
	pub async fn withdraw_reinstatement(&self, member: PersonId, gmail: i64, review: ReviewId) -> eyre::Result<()> {
		self.store()?.withdraw_reinstatement(member, gmail, review, Timestamp::now()).await
	}

	/// A capture's WebP, if it shows a review of a place the member tracks.
	pub async fn member_capture_webp(&self, member: PersonId, sha256: &str) -> eyre::Result<Vec<u8>> {
		if !self.store()?.member_sees_capture(member, sha256).await? {
			return Err(Rejected::not_found("no such capture").into());
		}
		self.capture_webp(sha256).await
	}

	/// Adds a Telegram channel: a destination the archive's bot can post to, and at least
	/// one event.
	pub async fn add_tg_channel(&self, member: PersonId, ch: &NewTgChannel) -> eyre::Result<TgChannelDto> {
		ch.destination
			.parse::<TelegramDestination>()
			.map_err(|e| Rejected::invalid(format!("destination {:?}: {e} (expected @channel, a -100… chat id, or <group>/<topic>)", ch.destination)))?;
		if ch.events.is_empty() {
			return Err(Rejected::invalid("subscribe the channel to at least one event").into());
		}
		self.store()?.add_tg_channel(member, ch, Timestamp::now()).await
	}

	/// The member's Telegram channels.
	pub async fn tg_channels(&self, member: PersonId) -> eyre::Result<Vec<TgChannelDto>> {
		self.store()?.tg_channels(member).await
	}

	/// Removes a channel and what it was still owed.
	pub async fn delete_tg_channel(&self, member: PersonId, id: i64) -> eyre::Result<()> {
		if !self.store()?.delete_tg_channel(member, id).await? {
			return Err(Rejected::not_found(format!("no Telegram channel {id}")).into());
		}
		Ok(())
	}

	/// Posts a line to the channel now; what Telegram refused comes back as [`Rejected`].
	pub async fn test_tg_channel(&self, member: PersonId, id: i64) -> eyre::Result<()> {
		let ch = self
			.tg_channels(member)
			.await?
			.into_iter()
			.find(|c| c.id == id)
			.ok_or_else(|| Rejected::not_found(format!("no Telegram channel {id}")))?;
		let events: Vec<&str> = ch.events.iter().map(AsRef::as_ref).collect();
		let text = format!("review_archive: this chat gets {}", events.join(", "));
		Ok(self.inner.webhooks.test_telegram(&ch.destination, text).await?)
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

	/// The member's tokens, renewed up to now.
	pub async fn tokens(&self, member: PersonId) -> eyre::Result<TokensDto> {
		let cfg = &self.inner.tokens;
		let store = self.store()?;
		Ok(TokensDto {
			balance: store.balance(member, Timestamp::now(), cfg).await?,
			daily: cfg.daily,
			cap: cfg.cap,
			kickbacks: store.kickbacks(member).await?,
		})
	}

	/// Adds the member's pending kick-backs to their balance.
	pub async fn claim_kickbacks(&self, member: PersonId) -> eyre::Result<TokensDto> {
		self.store()?.claim(member, Timestamp::now(), &self.inner.tokens).await?;
		self.tokens(member).await
	}

	/// The member's ledger, newest first: 200 rows.
	pub async fn ledger(&self, member: PersonId) -> eyre::Result<Vec<LedgerEntry>> {
		self.store()?.ledger(member, 200).await
	}

	/// What the member's tokens bought over the last 30 days.
	pub async fn usage(&self, member: PersonId) -> eyre::Result<Usage> {
		self.store()?.usage(member, Timestamp::now()).await
	}

	/// Sets or adds to the member's balance, as admin `by`.
	pub async fn change_tokens(&self, member: PersonId, req: &TokensChange, by: &str) -> eyre::Result<TokensDto> {
		self.store()?
			.change_balance(member, req.change, by, req.note.as_deref(), Timestamp::now(), &self.inner.tokens)
			.await?;
		self.tokens(member).await
	}
}

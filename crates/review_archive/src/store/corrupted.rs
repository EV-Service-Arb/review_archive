//! One-shot: the captures from before #36's check, checked; a review whose every capture is
//! corrupted goes back to pending. Remove once prod has checked.

use eyre::WrapErr;

use super::{Store, blobs::BlobStore};
use crate::webp;

impl Store {
	/// Resumable per capture.
	pub(crate) async fn check_unchecked_captures(&self, blobs: &BlobStore) -> eyre::Result<()> {
		let unchecked: Vec<(i64, String)> = sqlx::query_as("SELECT id, sha256 FROM captures WHERE corrupted IS NULL ORDER BY id")
			.fetch_all(&self.pool)
			.await
			.wrap_err("listing unchecked captures")?;
		if unchecked.is_empty() {
			return Ok(());
		}
		tracing::info!(count = unchecked.len(), "checking captures for #36's cut");
		let mut found = 0;
		// one at a time: a decode's buffers are the pod's memory budget, and nobody waits on this
		for (i, (id, sha)) in unchecked.iter().enumerate() {
			let path = blobs.path_of(sha).ok_or_else(|| eyre::eyre!("capture {id} names {sha:?}, not a blob"))?;
			let bytes = tokio::fs::read(&path).await.wrap_err_with(|| format!("reading {}", path.display()))?;
			let corrupted = tokio::task::spawn_blocking(move || eyre::Ok(webp::corrupted(&image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP)?.into_rgb8())))
				.await?
				.wrap_err_with(|| format!("decoding {}", path.display()))?;
			let mut tx = self.write().await?;
			sqlx::query("UPDATE captures SET corrupted = ? WHERE id = ?").bind(corrupted).bind(id).execute(&mut *tx).await?;
			found += usize::from(corrupted);
			sqlx::query(
				"UPDATE reviews SET capture_pending = NOT EXISTS (SELECT 1 FROM captures c WHERE c.review_id = reviews.id AND c.corrupted = 0)
				 WHERE id = (SELECT review_id FROM captures WHERE id = ?)",
			)
			.bind(id)
			.execute(&mut *tx)
			.await?;
			tx.commit().await?;
			if (i + 1) % 500 == 0 {
				tracing::info!(done = i + 1, of = unchecked.len(), found, "checking captures for #36's cut");
			}
		}
		tracing::info!(count = unchecked.len(), found, "captures checked for #36's cut");
		Ok(())
	}
}

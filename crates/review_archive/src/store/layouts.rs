//! Published dock layouts.

use eyre::WrapErr;
use jiff::Timestamp;
use review_archive_core::{dto::LayoutDto, fmt_ts};

use super::Store;

impl Store {
	/// Every band's published layout of `dock`.
	pub async fn layouts(&self, dock: &str) -> eyre::Result<Vec<LayoutDto>> {
		let rows: Vec<(String, String)> = sqlx::query_as("SELECT band, layout FROM layouts WHERE dock = ? ORDER BY band")
			.bind(dock)
			.fetch_all(&self.pool)
			.await
			.wrap_err("listing published layouts")?;
		Ok(rows.into_iter().map(|(band, layout)| LayoutDto { band, layout }).collect())
	}

	/// Replaces `dock`'s published layout for `band`.
	pub async fn publish_layout(&self, dock: &str, band: &str, layout: &str, now: Timestamp) -> eyre::Result<()> {
		sqlx::query(
			"INSERT INTO layouts (dock, band, layout, published_at) VALUES (?, ?, ?, ?) ON CONFLICT (dock, band) DO UPDATE SET layout = excluded.layout, published_at = excluded.published_at",
		)
		.bind(dock)
		.bind(band)
		.bind(layout)
		.bind(fmt_ts(now))
		.execute(&self.pool)
		.await
		.wrap_err("publishing a layout")?;
		Ok(())
	}
}

//! A place id from what a person pastes: the id itself, or a Maps URL. Resolving a URL
//! without an id takes a Places API search, which is the engine's job.

use eyre::WrapErr;

/// What the input says without asking anyone.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
	/// The input is, or contains, a place id.
	PlaceId(String),
	/// A URL with no id in it; searching needs the name and, when present, where the map was.
	Search {
		/// The place's name from the URL path.
		query: String,
		/// `(lat, lng)` of the map the URL was showing.
		near: Option<(f64, f64)>,
	},
}

/// Reads a place id or a Google Maps URL.
pub fn parse(input: &str) -> eyre::Result<Parsed> {
	let input = input.trim();
	if !input.contains("://") {
		eyre::ensure!(looks_like_place_id(input), "{input:?} is neither a place id nor a URL");
		return Ok(Parsed::PlaceId(input.to_owned()));
	}
	let url = url::Url::parse(input).wrap_err_with(|| format!("parsing {input:?}"))?;
	for (k, v) in url.query_pairs() {
		let candidate = match k.as_ref() {
			"query_place_id" | "place_id" => Some(v.to_string()),
			"q" | "query" => v.strip_prefix("place_id:").map(str::to_owned),
			_ => None,
		};
		if let Some(id) = candidate.filter(|c| looks_like_place_id(c)) {
			return Ok(Parsed::PlaceId(id));
		}
	}

	// /maps/place/<Name>/@<lat>,<lng>,<zoom>z/data=...!1s0x<hex>:0x<hex>!...
	let segments: Vec<String> = url.path_segments().map(|s| s.map(percent_decode).collect()).unwrap_or_default();
	let mut features: Vec<(u64, u64)> = segments.iter().filter_map(|s| s.strip_prefix("data=")).flat_map(feature_ids).collect();
	features.dedup();
	match features[..] {
		[] => {}
		[(cell, cid)] => return Ok(Parsed::PlaceId(place_id_of_feature(cell, cid))),
		_ => eyre::bail!("{input:?} names more than one place: {features:x?}"),
	}
	let name = segments
		.iter()
		.position(|s| s == "place")
		.and_then(|i| segments.get(i + 1))
		.filter(|s| !s.is_empty() && !s.starts_with('@'))
		.cloned()
		.or_else(|| {
			(url.path() == "/search")
				.then(|| url.query_pairs().find(|(k, _)| k == "q").map(|(_, v)| v.into_owned()))
				.flatten()
		}); // a share.google link lands on a Search for the place's name
	let near = segments.iter().find_map(|s| {
		let mut it = s.strip_prefix('@')?.split(',');
		Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
	});
	let query = name.ok_or_else(|| eyre::eyre!("no place id and no place name in {input:?}"))?;
	Ok(Parsed::Search { query, near })
}

/// Place ids are URL-safe base64-ish tokens, `ChIJ…` for most places.
pub fn looks_like_place_id(s: &str) -> bool {
	s.len() >= 16 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The `!1s0x<hex>:0x<hex>` entries of a Maps URL's `data=` segment.
fn feature_ids(data: &str) -> impl Iterator<Item = (u64, u64)> + '_ {
	data.split('!').filter_map(|e| {
		let (cell, cid) = e.strip_prefix("1s0x")?.split_once(":0x")?;
		Some((u64::from_str_radix(cell, 16).ok()?, u64::from_str_radix(cid, 16).ok()?))
	})
}

/// A `ChIJ…` place id is the feature id itself: `{1: {1: fixed64 cell, 2: fixed64 cid}}`, base64url.
fn place_id_of_feature(cell: u64, cid: u64) -> String {
	const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
	let mut bytes = vec![0x0a, 0x12, 0x09];
	bytes.extend(cell.to_le_bytes());
	bytes.push(0x11);
	bytes.extend(cid.to_le_bytes());
	let bits = bytes.len() * 8;
	(0..bits.div_ceil(6))
		.map(|i| {
			let at = |b: usize| bytes.get(b / 8).map_or(0, |byte| (byte >> (7 - b % 8)) & 1);
			B64[(0..6).fold(0, |acc, k| acc << 1 | at(i * 6 + k)) as usize] as char
		})
		.collect()
}

/// Form decoding: `+` is a space, `%2B` a plus.
fn percent_decode(s: &str) -> String {
	url::form_urlencoded::parse(format!("x={s}").as_bytes())
		.next()
		.map(|(_, v)| v.into_owned())
		.unwrap_or_else(|| s.to_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ids_and_urls() {
		let id = "ChIJLU7jZClu5kcR4PcOOO6p3I0";
		assert_eq!(parse(id).unwrap(), Parsed::PlaceId(id.into()));
		assert_eq!(parse(&format!("https://www.google.com/maps/place/?q=place_id:{id}&hl=fr")).unwrap(), Parsed::PlaceId(id.into()));
		assert_eq!(
			parse(&format!("https://www.google.com/maps/search/?api=1&query=Eiffel&query_place_id={id}")).unwrap(),
			Parsed::PlaceId(id.into())
		);
		assert!(parse("cafe").is_err());
	}

	#[test]
	fn url_without_id_becomes_a_search() {
		let got = parse("https://www.google.com/maps/place/Le+Procope/@48.8530,2.3389,17z/data=!3m1!4b1").unwrap();
		assert_eq!(
			got,
			Parsed::Search {
				query: "Le Procope".into(),
				near: Some((48.853, 2.3389)),
			}
		);
		let accented = parse("https://www.google.com/maps/place/Caf%C3%A9+de+Flore/@48.854,2.332,17z").unwrap();
		assert!(matches!(accented, Parsed::Search { ref query, .. } if query == "Café de Flore"));
	}
}

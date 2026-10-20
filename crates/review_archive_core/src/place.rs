//! A place id from what a person pastes: the id itself, or a Maps URL. Resolving a URL
//! without an id takes a Places API search, which is the engine's job.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use eyre::WrapErr;

/// What the input says without asking anyone.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
	/// The input is, or contains, a place id.
	PlaceId(String),
	/// An id Places answers for by the place's own: built from a cid alone, it is not the place's.
	Alias(String),
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
	let cid = url
		.query_pairs()
		.find_map(|(k, v)| match k.as_ref() {
			"cid" => Some(v.parse::<u64>().wrap_err_with(|| format!("cid {v:?} in {input:?}"))),
			"svid" => Some(svid_cid(&v).ok_or_else(|| eyre::eyre!("the svid of {input:?} names no place"))),
			_ => None,
		})
		.transpose()?;
	if let Some(cid) = cid {
		return Ok(Parsed::Alias(place_id_of_feature(0, cid)));
	}
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

/// A Search viewer's `svid` is base64url protobuf; one of its strings is, base64url again, a protobuf holding the feature id `0x0:0x<cid>`.
fn svid_cid(svid: &str) -> Option<u64> {
	fn varint(b: &mut &[u8]) -> Option<u64> {
		let mut v = 0;
		for shift in (0..64).step_by(7) {
			let (&byte, rest) = b.split_first()?;
			*b = rest;
			v |= u64::from(byte & 0x7f) << shift;
			if byte & 0x80 == 0 {
				return Some(v);
			}
		}
		None
	}
	fn strings<'a>(mut b: &'a [u8], out: &mut Vec<&'a [u8]>) -> Option<()> {
		while !b.is_empty() {
			match varint(&mut b)? & 7 {
				0 => {
					varint(&mut b)?;
				}
				1 => b = b.get(8..)?,
				5 => b = b.get(4..)?,
				2 => {
					let n = usize::try_from(varint(&mut b)?).ok()?;
					let (s, rest) = (b.get(..n)?, &b[n..]);
					out.push(s);
					let _ = strings(s, out); // a string is not always a message
					b = rest;
				}
				_ => return None,
			}
		}
		Some(())
	}
	let outer = URL_SAFE_NO_PAD.decode(svid.trim_end_matches('=')).ok()?;
	let mut found = Vec::new();
	strings(&outer, &mut found)?;
	found.into_iter().filter_map(|s| URL_SAFE_NO_PAD.decode(s).ok()).find_map(|inner| {
		let inner = String::from_utf8_lossy(&inner);
		let (_, after) = inner.split_once("0x0:0x")?;
		let hex: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
		u64::from_str_radix(&hex, 16).ok()
	})
}

/// A `ChIJ…` place id is the feature id itself: `{1: {1: fixed64 cell, 2: fixed64 cid}}`, base64url.
fn place_id_of_feature(cell: u64, cid: u64) -> String {
	let mut bytes = vec![0x0a, 0x12, 0x09];
	bytes.extend(cell.to_le_bytes());
	bytes.push(0x11);
	bytes.extend(cid.to_le_bytes());
	URL_SAFE_NO_PAD.encode(bytes)
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

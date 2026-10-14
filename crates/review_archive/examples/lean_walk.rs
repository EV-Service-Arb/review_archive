//! One headless walk on a kept profile with every `browser.lean` trade, its WebPs written out, to look at
//! what the Pi's launch does to Maps. `cargo r --example lean_walk -- <profile> <out_dir> <cards> <place> <lang>`

use review_archive::{
	Archive, CaptureRequest,
	config::{Config, Lean},
};

#[tokio::main]
async fn main() {
	tracing_subscriber::fmt().with_env_filter("review_archive=info").init();
	let mut args = std::env::args().skip(1);
	let profile: std::path::PathBuf = args.next().expect("profile dir").into();
	let out: std::path::PathBuf = args.next().expect("out dir").into();
	let cards: usize = args.next().expect("card count").parse().unwrap();
	let place = args.next().expect("place id");
	let lang = args.next().expect("lang");
	let mut config = Config::default();
	config.browser.executable = std::env::var_os("REVIEW_ARCHIVE_CHROME").map(Into::into);
	config.browser.profile_dirs = vec![profile];
	config.browser.lean = Some(vec![
		Lean::JsHeapMib(512),
		Lean::SmallYoungGeneration,
		Lean::JsLiteMode,
		Lean::OneRasterThread,
		Lean::LowEndDevice,
	]);
	let archive = Archive::open(config).await.unwrap();
	let mut req = CaptureRequest::new(&place).lang(&lang).max_reviews(cards);
	if std::env::var_os("SCROLL_ONLY").is_some() {
		req.limits.review_ids = Some(vec!["none".to_owned()]);
	}
	let got = archive.capture_place(&req).await;
	archive.close().await;
	let got = got.unwrap_or_else(|e| panic!("{}", review_archive::describe(&e)));
	std::fs::create_dir_all(&out).unwrap();
	for (id, webp) in &got.webps {
		std::fs::write(out.join(format!("{id}.webp")), &webp.bytes).unwrap();
		println!("{id}: {}x{}", webp.width, webp.height);
	}
	println!(
		"{} reviews, {} webps, coverage {:?}, warnings {:?}",
		got.scan.reviews.len(),
		got.webps.len(),
		got.scan.coverage,
		got.scan.warnings
	);
}

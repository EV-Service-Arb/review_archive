//! A browser test's failure that a loaded machine causes: Chromium that starts late, a page that answers late.

/// Panics with `failure`, naming the machine's load as its likely cause.
pub fn busy(failure: String) -> ! {
	let pressure = ["cpu", "io", "memory"]
		.map(|r| match std::fs::read_to_string(format!("/proc/pressure/{r}")) {
			Ok(p) => {
				let line = |l: &str| {
					let [kind, avg10, avg60, ..] = l.split_whitespace().collect::<Vec<_>>()[..] else {
						panic!("PSI's format: {l}")
					};
					let avg = |f: &str, key| f.strip_prefix(key).unwrap_or_else(|| panic!("PSI's format: {l}")).to_owned();
					format!("{kind} {}/{}", avg(avg10, "avg10="), avg(avg60, "avg60="))
				};
				format!("{r}: {}", p.lines().map(line).collect::<Vec<_>>().join(", "))
			}
			Err(e) => format!("{r}: unreadable ({e})"), // a kernel without PSI; the failure itself still stands
		})
		.join("\n");
	let test = std::thread::current().name().expect("libtest names a test's thread after it").to_owned();
	panic!("{:?}", miette::Report::new(Busy { failure, pressure, test }))
}

#[derive(Debug, miette::Diagnostic, thiserror::Error)]
#[error("{failure}\n\nthis machine's pressure now, % stalled over 10s/60s:\n{pressure}")]
#[diagnostic(
	code(tests::busy_host),
	help(
		"a test's browser starts in a few seconds on an idle machine: this is most often the machine, not the code. A high `full` IO or memory pressure above, or other Chromiums starting at once, says so. Rerun it alone: `cargo test {test} -- --ignored`"
	)
)]
struct Busy {
	failure: String,
	pressure: String,
	test: String,
}

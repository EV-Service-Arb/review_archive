//! What the host looked like when Chromium failed to start: kernel pressure, free memory and
//! the cgroup's ceiling. A start that takes longer than its limit is almost always the host
//! starved of IO or memory, and by the time anyone looks the starvation is over; the
//! figures are what says so.
//!
//! Linux only, best effort: a file that is missing (another OS, no PSI, cgroup v1) leaves its
//! part out, and a stalled read is given up on rather than waited for.

use std::{fmt, path::Path, time::Duration};

/// Longer than any read of `/proc` or `/sys` takes on a healthy host; a read still pending
/// by then is what a starved host looks like, and the error must not wait on it.
const READ_LIMIT: Duration = Duration::from_secs(2);

const PSI_IO: &str = "/proc/pressure/io";
const PSI_MEMORY: &str = "/proc/pressure/memory";
const MEMINFO: &str = "/proc/meminfo";
const CGROUP_MEMORY_CURRENT: &str = "/sys/fs/cgroup/memory.current";
const CGROUP_MEMORY_MAX: &str = "/sys/fs/cgroup/memory.max";

/// The share of the last 10 and 60 seconds some task (or all of them) waited, in percent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stall {
	avg10: f32,
	avg60: f32,
}

/// One resource's pressure-stall information (`/proc/pressure/*`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pressure {
	some: Option<Stall>,
	full: Option<Stall>,
}

/// `/proc/meminfo`, in KiB as the kernel reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MemInfo {
	available: Option<u64>,
	swap_total: Option<u64>,
	swap_free: Option<u64>,
}

/// A memory cgroup's ceiling: `memory.max` is the literal `max` when there is none.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ceiling {
	Unlimited,
	Bytes(u64),
}

/// The host's state at one moment. Renders as one line.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
	io: Option<Pressure>,
	memory: Option<Pressure>,
	meminfo: MemInfo,
	cgroup_current: Option<u64>,
	cgroup_max: Option<Ceiling>,
}

impl Snapshot {
	/// Reads the host as it is now. Never fails and never waits for long: what could not be
	/// read is left out.
	pub(crate) async fn capture() -> Self {
		let (io, memory, meminfo, current, max) = tokio::join!(read(PSI_IO), read(PSI_MEMORY), read(MEMINFO), read(CGROUP_MEMORY_CURRENT), read(CGROUP_MEMORY_MAX));
		Self::from_files(io.as_deref(), memory.as_deref(), meminfo.as_deref(), current.as_deref(), max.as_deref())
	}

	/// The snapshot of these file contents; `None` for a file that was not there.
	fn from_files(io: Option<&str>, memory: Option<&str>, meminfo: Option<&str>, cgroup_current: Option<&str>, cgroup_max: Option<&str>) -> Self {
		Self {
			io: io.map(parse_pressure),
			memory: memory.map(parse_pressure),
			meminfo: meminfo.map(parse_meminfo).unwrap_or_default(),
			cgroup_current: cgroup_current.and_then(|s| s.trim().parse().ok()),
			cgroup_max: cgroup_max.and_then(parse_ceiling),
		}
	}
}

impl fmt::Display for Snapshot {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut parts = Vec::new();
		for (name, pressure) in [("io", &self.io), ("memory", &self.memory)] {
			if let Some(p) = pressure {
				parts.push(format!("{name} pressure {p}"));
			}
		}
		if let Some(kib) = self.meminfo.available {
			parts.push(format!("MemAvailable {} MiB", kib / 1024));
		}
		if let (Some(total), Some(free)) = (self.meminfo.swap_total, self.meminfo.swap_free) {
			parts.push(format!("swap used {} of {} MiB", total.saturating_sub(free) / 1024, total / 1024));
		}
		match (self.cgroup_current, self.cgroup_max) {
			(Some(now), Some(Ceiling::Bytes(max))) => parts.push(format!("cgroup memory {} of {} MiB", now >> 20, max >> 20)),
			(Some(now), _) => parts.push(format!("cgroup memory {} MiB, no limit", now >> 20)),
			(None, _) => {}
		}
		match parts.is_empty() {
			true => f.write_str("host stats unavailable"),
			false => write!(f, "host: {}", parts.join("; ")),
		}
	}
}

impl fmt::Display for Pressure {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut first = true;
		for (kind, stall) in [("some", self.some), ("full", self.full)] {
			if let Some(s) = stall {
				write!(f, "{}{kind} avg10={:.2} avg60={:.2}", if first { "" } else { " " }, s.avg10, s.avg60)?;
				first = false;
			}
		}
		if first {
			f.write_str("unreadable")?;
		}
		Ok(())
	}
}

/// The file's contents, or `None` for any reason: this is a diagnostic, and the one thing it
/// must not do is turn a failed start into a second failure or a longer wait.
async fn read(path: impl AsRef<Path>) -> Option<String> {
	tokio::time::timeout(READ_LIMIT, tokio::fs::read_to_string(path)).await.ok()?.ok()
}

/// `some avg10=0.00 avg60=0.00 avg300=0.00 total=0` lines of a `/proc/pressure/*` file.
fn parse_pressure(text: &str) -> Pressure {
	let mut out = Pressure::default();
	for line in text.lines() {
		let mut words = line.split_whitespace();
		let slot = match words.next() {
			Some("some") => &mut out.some,
			Some("full") => &mut out.full,
			_ => continue,
		};
		let (mut avg10, mut avg60) = (None, None);
		for word in words {
			match word.split_once('=') {
				Some(("avg10", v)) => avg10 = v.parse().ok(),
				Some(("avg60", v)) => avg60 = v.parse().ok(),
				_ => {}
			}
		}
		if let (Some(avg10), Some(avg60)) = (avg10, avg60) {
			*slot = Some(Stall { avg10, avg60 });
		}
	}
	out
}

/// The three fields of `/proc/meminfo` the snapshot shows (`MemAvailable:   123456 kB`).
fn parse_meminfo(text: &str) -> MemInfo {
	let mut out = MemInfo::default();
	for line in text.lines() {
		let Some((key, rest)) = line.split_once(':') else { continue };
		let slot = match key {
			"MemAvailable" => &mut out.available,
			"SwapTotal" => &mut out.swap_total,
			"SwapFree" => &mut out.swap_free,
			_ => continue,
		};
		*slot = rest.split_whitespace().next().and_then(|n| n.parse().ok());
	}
	out
}

fn parse_ceiling(text: &str) -> Option<Ceiling> {
	match text.trim() {
		"max" => Some(Ceiling::Unlimited),
		n => n.parse().ok().map(Ceiling::Bytes),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const IO: &str = "some avg10=1.50 avg60=12.25 avg300=3.00 total=123456\nfull avg10=0.75 avg60=8.00 avg300=1.00 total=654321\n";
	// older kernels have no `full` line for CPU, and a cgroup may lack the file altogether
	const SOME_ONLY: &str = "some avg10=0.00 avg60=0.10 avg300=0.00 total=0\n";
	const MEMINFO_TEXT: &str = "MemTotal:        8000000 kB\nMemFree:          100000 kB\nMemAvailable:    2097152 kB\nSwapTotal:       1048576 kB\nSwapFree:         524288 kB\n";

	#[test]
	fn pressure_lines_give_both_windows() {
		let p = parse_pressure(IO);
		assert_eq!(p.some, Some(Stall { avg10: 1.5, avg60: 12.25 }));
		assert_eq!(p.full, Some(Stall { avg10: 0.75, avg60: 8.0 }));
		assert_eq!(parse_pressure(SOME_ONLY).full, None);
	}

	#[test]
	fn garbage_pressure_is_left_out() {
		assert_eq!(parse_pressure("some avg10=x avg60=1.0\nnonsense\n\n"), Pressure::default());
	}

	#[test]
	fn meminfo_gives_available_and_swap() {
		assert_eq!(
			parse_meminfo(MEMINFO_TEXT),
			MemInfo {
				available: Some(2_097_152),
				swap_total: Some(1_048_576),
				swap_free: Some(524_288)
			}
		);
		assert_eq!(parse_meminfo("MemAvailable: lots kB\n"), MemInfo::default());
	}

	#[test]
	fn a_ceiling_is_a_number_or_max() {
		assert_eq!(parse_ceiling("max\n"), Some(Ceiling::Unlimited));
		assert_eq!(parse_ceiling("1073741824\n"), Some(Ceiling::Bytes(1 << 30)));
		assert_eq!(parse_ceiling("-"), None);
	}

	#[test]
	fn the_snapshot_renders_on_one_line() {
		let s = Snapshot::from_files(Some(IO), Some(SOME_ONLY), Some(MEMINFO_TEXT), Some("314572800\n"), Some("1073741824\n"));
		assert_eq!(
			s.to_string(),
			"host: io pressure some avg10=1.50 avg60=12.25 full avg10=0.75 avg60=8.00; \
			 memory pressure some avg10=0.00 avg60=0.10; MemAvailable 2048 MiB; swap used 512 of 1024 MiB; cgroup memory 300 of 1024 MiB"
		);
	}

	#[test]
	fn what_is_missing_is_left_out() {
		let s = Snapshot::from_files(None, None, None, Some("1048576"), Some("max"));
		assert_eq!(s.to_string(), "host: cgroup memory 1 MiB, no limit");
		assert_eq!(Snapshot::from_files(None, None, None, None, None).to_string(), "host stats unavailable");
	}
}

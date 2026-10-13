//! What the host looked like when Chromium failed to start: kernel pressure, free memory and
//! the container's own cgroup. A start that takes longer than its limit is almost always the
//! host starved of IO or memory, and by the time anyone looks the starvation is over; the
//! figures are what says so.
//!
//! Which figures to trust:
//! - `/proc/pressure/*` is the whole node's. The node's IO pressure is inflated wherever tasks
//!   wait on `io_uring` (TigerBeetle does): the kernel counts that wait as iowait, so it can sit
//!   at tens of percent beside an idle disk. It says little there; the cgroup's memory pressure
//!   is the container's own.
//! - `memory.current` counts page cache, which the kernel keeps until it needs the room, so it
//!   sits near the limit on any pod that has read files. Anonymous memory (`anon`) is what
//!   the container cannot give back, and the `max` and `oom_kill` counters of `memory.events`
//!   say whether the limit was actually hit and whether anything was killed for it.
//! - `memory.peak` is the most the container has held since it started: what to size its
//!   memory lease by. `leased` is `memory.max` when the lease was granted.
//! - `pss` splits the cgroup's processes by kind (Chromium's `--type=`, else the executable's
//!   name): which of browser, renderer, GPU process and driver holds the memory.
//!
//! Linux only, best effort: a file that is missing (another OS, no PSI, cgroup v1) leaves its
//! part out, and a stalled read is given up on rather than waited for.

use std::{
	fmt,
	path::{Component, Path, PathBuf},
	time::Duration,
};

/// Longer than any read of `/proc` or `/sys` takes on a healthy host; a read still pending
/// by then is what a starved host looks like, and the error must not wait on it.
const READ_LIMIT: Duration = Duration::from_secs(2);

const PSI_IO: &str = "/proc/pressure/io";
const PSI_MEMORY: &str = "/proc/pressure/memory";
const MEMINFO: &str = "/proc/meminfo";
const PROC_SELF_CGROUP: &str = "/proc/self/cgroup";
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

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
pub(crate) enum Ceiling {
	Unlimited,
	Bytes(u64),
}

/// The contents of the container's cgroup files; `None` for one that was not there.
#[derive(Clone, Debug, Default, PartialEq)]
struct CgroupFiles {
	current: Option<String>,
	peak: Option<String>,
	max: Option<String>,
	stat: Option<String>,
	events: Option<String>,
	pressure: Option<String>,
	procs: Option<String>,
}

impl CgroupFiles {
	fn is_empty(&self) -> bool {
		*self == Self::default()
	}
}

/// The container's cgroup: its memory in bytes, and what the limit did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cgroup {
	/// `memory.current`, page cache included.
	current: Option<u64>,
	/// `memory.peak`: the most since the cgroup was made, page cache included.
	peak: Option<u64>,
	max: Option<Ceiling>,
	/// `memory.stat`'s `anon`: what the container holds that is not a file, so cannot be dropped for room.
	anon: Option<u64>,
	/// `memory.stat`'s `file`: the page cache.
	file: Option<u64>,
	/// `memory.events`' `max`: how often the usage hit the limit and reclaim had to run.
	hit_max: Option<u64>,
	/// `memory.events`' `oom_kill`: processes killed for the limit.
	oom_kill: Option<u64>,
	pressure: Option<Pressure>,
}

/// The host's state at one moment. Renders as one line.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
	/// The node's, not the container's.
	io: Option<Pressure>,
	/// The node's, not the container's.
	memory: Option<Pressure>,
	meminfo: MemInfo,
	cgroup: Cgroup,
	leased: Option<Ceiling>,
	/// By PSS, most first.
	processes: Vec<Processes>,
}

/// The cgroup's processes of one kind.
#[derive(Clone, Debug, PartialEq)]
struct Processes {
	kind: String,
	count: usize,
	/// Bytes.
	pss: u64,
}

impl Snapshot {
	/// Reads the host as it is now, beside what the lease was granted ([`limit`] then). Never
	/// fails and never waits for long: what could not be read is left out.
	pub(crate) async fn capture(leased: Option<Ceiling>) -> Self {
		let (io, memory, meminfo, files) = tokio::join!(read(PSI_IO), read(PSI_MEMORY), read(MEMINFO), own_cgroup());
		let processes = processes(files.procs.as_deref().unwrap_or_default()).await;
		Self {
			leased,
			processes,
			..Self::from_files(io.as_deref(), memory.as_deref(), meminfo.as_deref(), &files)
		}
	}

	/// The snapshot of these file contents; `None` for a file that was not there.
	fn from_files(io: Option<&str>, memory: Option<&str>, meminfo: Option<&str>, cgroup: &CgroupFiles) -> Self {
		let stat = cgroup.stat.as_deref();
		let events = cgroup.events.as_deref();
		Self {
			io: io.map(parse_pressure),
			memory: memory.map(parse_pressure),
			meminfo: meminfo.map(parse_meminfo).unwrap_or_default(),
			cgroup: Cgroup {
				current: cgroup.current.as_deref().and_then(|s| s.trim().parse().ok()),
				peak: cgroup.peak.as_deref().and_then(|s| s.trim().parse().ok()),
				max: cgroup.max.as_deref().and_then(parse_ceiling),
				anon: stat.and_then(|s| counter(s, "anon")),
				file: stat.and_then(|s| counter(s, "file")),
				hit_max: events.and_then(|s| counter(s, "max")),
				oom_kill: events.and_then(|s| counter(s, "oom_kill")),
				pressure: cgroup.pressure.as_deref().map(parse_pressure),
			},
			leased: None,
			processes: vec![],
		}
	}
}

impl fmt::Display for Snapshot {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut parts = Vec::new();
		for (name, pressure) in [("node io", &self.io), ("node memory", &self.memory)] {
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
		let c = &self.cgroup;
		match (c.current, c.max) {
			(Some(now), Some(Ceiling::Bytes(max))) => parts.push(format!("cgroup memory {} of {} MiB incl. page cache", now >> 20, max >> 20)),
			(Some(now), _) => parts.push(format!("cgroup memory {} MiB incl. page cache, no limit", now >> 20)),
			(None, _) => {}
		}
		if let Some(peak) = c.peak {
			parts.push(format!("cgroup peak {} MiB", peak >> 20));
		}
		match self.leased {
			Some(Ceiling::Bytes(max)) => parts.push(format!("leased {} MiB", max >> 20)),
			Some(Ceiling::Unlimited) => parts.push("leased without a limit".to_owned()),
			None => {}
		}
		match (c.anon, c.file) {
			(Some(anon), Some(file)) => parts.push(format!("cgroup anon {} MiB, file {} MiB", anon >> 20, file >> 20)),
			(Some(anon), None) => parts.push(format!("cgroup anon {} MiB", anon >> 20)),
			(None, Some(file)) => parts.push(format!("cgroup file {} MiB", file >> 20)),
			(None, None) => {}
		}
		let events: Vec<String> = [("max", c.hit_max), ("oom_kill", c.oom_kill)]
			.into_iter()
			.filter_map(|(name, n)| Some(format!("{name}={}", n?)))
			.collect();
		if !events.is_empty() {
			parts.push(format!("cgroup memory events {}", events.join(" ")));
		}
		if let Some(p) = &c.pressure {
			parts.push(format!("cgroup memory pressure {p}"));
		}
		if !self.processes.is_empty() {
			let each: Vec<String> = self
				.processes
				.iter()
				.map(|p| match p.count {
					1 => format!("{} {} MiB", p.kind, p.pss >> 20),
					n => format!("{}×{n} {} MiB", p.kind, p.pss >> 20),
				})
				.collect();
			parts.push(format!("pss {}", each.join(", ")));
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

/// The container's `memory.max` now; `None` when it cannot be read.
pub(crate) async fn limit() -> Option<Ceiling> {
	own_cgroup().await.max.as_deref().and_then(parse_ceiling)
}

async fn own_cgroup() -> CgroupFiles {
	let own = read(PROC_SELF_CGROUP).await;
	let root = Path::new(CGROUP_ROOT);
	let dir = cgroup_dir(own.as_deref().unwrap_or_default(), root);
	let files = read_cgroup(&dir).await;
	// The path is the one the host sees; a container with its own cgroup namespace has its cgroup mounted as the root.
	match files.is_empty() && dir != root {
		true => read_cgroup(root).await,
		false => files,
	}
}

async fn read_cgroup(dir: &Path) -> CgroupFiles {
	let (current, peak, max, stat, events, pressure, procs) = tokio::join!(
		read(dir.join("memory.current")),
		read(dir.join("memory.peak")),
		read(dir.join("memory.max")),
		read(dir.join("memory.stat")),
		read(dir.join("memory.events")),
		read(dir.join("memory.pressure")),
		read(dir.join("cgroup.procs"))
	);
	CgroupFiles {
		current,
		peak,
		max,
		stat,
		events,
		pressure,
		procs,
	}
}

/// `cgroup.procs`' processes by kind; one that exits or hides its files meanwhile is left out.
async fn processes(procs: &str) -> Vec<Processes> {
	let each = procs.split_whitespace().map(|pid| async move {
		let (cmdline, rollup) = tokio::join!(read(format!("/proc/{pid}/cmdline")), read(format!("/proc/{pid}/smaps_rollup")));
		Some((process_kind(&cmdline?), counter(&rollup?, "Pss:")? << 10))
	});
	let mut out: Vec<Processes> = Vec::new();
	for (kind, pss) in futures::future::join_all(each).await.into_iter().flatten() {
		match out.iter_mut().find(|p| p.kind == kind) {
			Some(p) => {
				p.count += 1;
				p.pss += pss;
			}
			None => out.push(Processes { kind, count: 1, pss }),
		}
	}
	out.sort_by_key(|p| std::cmp::Reverse(p.pss));
	out
}

/// Chromium's `--type=` (`renderer`, `gpu-process`, …), else the executable's name: the browser
/// process, the node driver, this server.
fn process_kind(cmdline: &str) -> String {
	let mut args = cmdline.split('\0');
	let exe = args.next().unwrap_or_default();
	match args.find_map(|a| a.strip_prefix("--type=")) {
		Some(t) => t.to_owned(),
		None => Path::new(exe).file_name().map_or_else(|| exe.to_owned(), |n| n.to_string_lossy().into_owned()),
	}
}

/// Where the process's own cgroup is under `root` (cgroup v2: the `0::<path>` line of
/// `/proc/self/cgroup`). `root` itself when there is no such line or the path leaves the
/// namespace (`/../..`), which is also what a container with its own cgroup namespace sees
/// for its own cgroup.
fn cgroup_dir(proc_self_cgroup: &str, root: &Path) -> PathBuf {
	let Some(path) = proc_self_cgroup.lines().find_map(|l| l.strip_prefix("0::")) else {
		return root.to_owned();
	};
	let relative = Path::new(path.trim()).strip_prefix("/").unwrap_or(Path::new(""));
	match relative.components().all(|c| matches!(c, Component::Normal(_))) {
		true => root.join(relative),
		false => root.to_owned(),
	}
}

/// The value of `key` in a flat-keyed file (`memory.stat`, `memory.events`: `anon 123456`).
fn counter(text: &str, key: &str) -> Option<u64> {
	text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')?.trim().parse().ok())
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

	const STAT: &str = "anon 20971520\nfile 41943040\nkernel 1048576\nfile_mapped 4096\nanon_thp 0\n";
	const EVENTS: &str = "low 0\nhigh 0\nmax 12\noom 3\noom_kill 2\noom_group_kill 0\n";

	fn cgroup() -> CgroupFiles {
		CgroupFiles {
			current: Some("66060288\n".to_owned()), // 63 MiB
			peak: Some("547356672\n".to_owned()),   // 522 MiB
			max: Some("67108864\n".to_owned()),     // 64 MiB
			stat: Some(STAT.to_owned()),
			events: Some(EVENTS.to_owned()),
			pressure: Some("some avg10=2.50 avg60=1.00 avg300=0.00 total=9\nfull avg10=0.50 avg60=0.25 avg300=0.00 total=3\n".to_owned()),
			procs: None,
		}
	}

	#[test]
	fn a_counter_is_matched_by_its_whole_name() {
		assert_eq!(counter(STAT, "anon"), Some(20 << 20));
		assert_eq!(counter(STAT, "file"), Some(40 << 20), "not file_mapped");
		assert_eq!(counter(EVENTS, "max"), Some(12));
		assert_eq!(counter(EVENTS, "oom"), Some(3), "not oom_kill");
		assert_eq!(counter(EVENTS, "oom_kill"), Some(2));
		assert_eq!(counter(EVENTS, "swap"), None);
		assert_eq!(counter("anon lots\n", "anon"), None);
	}

	#[test]
	fn the_cgroup_is_the_one_proc_names_under_the_root() {
		let root = Path::new("/sys/fs/cgroup");
		let pod = "0::/kubepods.slice/kubepods-burstable.slice/cri-containerd-abc.scope\n";
		assert_eq!(cgroup_dir(pod, root), root.join("kubepods.slice/kubepods-burstable.slice/cri-containerd-abc.scope"));
		// hybrid hosts list v1 controllers too; only the unified line counts
		assert_eq!(cgroup_dir("12:memory:/old\n0::/user.slice\n", root), root.join("user.slice"));
	}

	#[test]
	fn a_cgroup_that_is_the_namespace_root_is_the_mount_itself() {
		let root = Path::new("/sys/fs/cgroup");
		assert_eq!(cgroup_dir("0::/\n", root), root);
		assert_eq!(cgroup_dir("0::/../../kubepods.slice\n", root), root, "outside the namespace is not under the mount");
		assert_eq!(cgroup_dir("", root), root);
		assert_eq!(cgroup_dir("4:memory:/x\n", root), root, "cgroup v1");
	}

	#[test]
	fn the_snapshot_renders_on_one_line() {
		let s = Snapshot {
			leased: Some(Ceiling::Bytes(1 << 30)),
			..Snapshot::from_files(Some(IO), Some(SOME_ONLY), Some(MEMINFO_TEXT), &cgroup())
		};
		assert_eq!(
			s.to_string(),
			"host: node io pressure some avg10=1.50 avg60=12.25 full avg10=0.75 avg60=8.00; \
			 node memory pressure some avg10=0.00 avg60=0.10; MemAvailable 2048 MiB; swap used 512 of 1024 MiB; \
			 cgroup memory 63 of 64 MiB incl. page cache; cgroup peak 522 MiB; leased 1024 MiB; cgroup anon 20 MiB, file 40 MiB; cgroup memory events max=12 oom_kill=2; \
			 cgroup memory pressure some avg10=2.50 avg60=1.00 full avg10=0.50 avg60=0.25"
		);
	}

	#[test]
	fn a_process_is_its_chromium_type_or_its_executable() {
		assert_eq!(process_kind("/nix/store/x-chromium/libexec/chromium/chromium\0--type=renderer\0--lang=en\0"), "renderer");
		assert_eq!(process_kind("/nix/store/x-chromium/libexec/chromium/chromium\0--headless\0"), "chromium");
		assert_eq!(process_kind("/nix/store/y-nodejs/bin/node\0/z/cli.js\0run-driver\0"), "node");
		let s = Snapshot {
			processes: vec![
				Processes {
					kind: "renderer".to_owned(),
					count: 2,
					pss: 300 << 20,
				},
				Processes {
					kind: "node".to_owned(),
					count: 1,
					pss: 60 << 20,
				},
			],
			..Snapshot::default()
		};
		assert_eq!(s.to_string(), "host: pss renderer×2 300 MiB, node 60 MiB");
	}

	#[test]
	fn what_is_missing_is_left_out() {
		let only_current = CgroupFiles {
			current: Some("1048576".to_owned()),
			max: Some("max".to_owned()),
			..CgroupFiles::default()
		};
		let s = Snapshot::from_files(None, None, None, &only_current);
		assert_eq!(s.to_string(), "host: cgroup memory 1 MiB incl. page cache, no limit");
		assert_eq!(Snapshot::from_files(None, None, None, &CgroupFiles::default()).to_string(), "host stats unavailable");
	}

	#[test]
	fn a_cgroup_without_some_files_shows_the_rest() {
		let files = CgroupFiles {
			stat: Some("anon 1048576\n".to_owned()),
			events: Some("oom_kill 0\n".to_owned()),
			..CgroupFiles::default()
		};
		assert_eq!(
			Snapshot::from_files(None, None, None, &files).to_string(),
			"host: cgroup anon 1 MiB; cgroup memory events oom_kill=0"
		);
	}
}

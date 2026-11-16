//! Composition root: settings, error monitoring and telemetry, then the CLI over the
//! library's `Archive`.

mod config;
mod settings;

use std::{
	fs::{OpenOptions, TryLockError},
	path::PathBuf,
	time::Duration,
};

use clap::{Args, Parser, Subcommand};
use ev_lib::{alerts, error_monitoring};
use eyre::WrapErr;
use review_archive::{Archive, SCANNER_VERSION, browser::Browser};
use review_archive_core::PersonId;
use review_archive_server::{auth::Dev, http, report, worker};
use tokio::sync::watch;

use crate::{config::AppConfig, settings::Settings};

#[derive(Parser)]
#[command(name = "review_archive", version = SCANNER_VERSION, about = "Archive of public place reviews: a WebP screenshot of every review as it first appears, plus data for statistics")]
struct Cli {
	#[clap(flatten)]
	settings_flags: config::SettingsFlags,
	#[command(subcommand)]
	cmd: Cmd,
	/// Buy the Places answers the queries cache lacks, on real quota; without it only cached ones are served.
	#[arg(long, global = true)]
	spend_real_quota_i_am_certain: bool,
}

#[derive(Subcommand)]
enum Cmd {
	/// Write the defaults, diff against them, or export the JSON Schema / Nix module of the config.
	Config {
		#[command(subcommand)]
		cmd: config::SettingsCommand,
	},
	/// Scheduler + HTTP API: what the archive does, every write through its operations.
	Serve(ServeArgs),
	/// Starts Chromium as a walk does, on a throwaway profile, and shoots an element as a walk
	/// shoots a card: whether this host runs the configured browser.
	CheckBrowser,
}

#[derive(Args)]
struct ServeArgs {
	/// Take every request as one made-up person holding these permissions, without the panel:
	/// an alias (`sa:admin`), `sa:review_archive:*` permissions comma-separated, or `none`. A
	/// local dashboard; loopback binds outside production only.
	#[arg(long)]
	dev_member: Option<String>,
	/// Who `--dev-member` is (their person id, `GET /members`); needed only when several members have GMBs.
	#[arg(long, requires = "dev_member")]
	dev_as: Option<i64>,
	/// Development only, never in production: no check at boot that a Google account is signed
	/// in, and every account found signed out fails walks instead of ending `serve` with exit 78.
	#[arg(long)]
	no_login_check: bool,
	/// Save the review cards' HTML of every walk's every step here, to refresh the parser fixtures.
	#[arg(long)]
	dump_html: Option<PathBuf>,
}

// Sentry must be initialised before the async runtime starts, hence no #[tokio::main].
fn main() -> eyre::Result<()> {
	color_eyre::install()?;

	// The deploy contract, straight out of the image: the gitops preflight diffs it with the
	// cluster Secret's keys, so a missing variable is caught before the rollout.
	if let Some(profile) = settings::print_required_vars_for() {
		for var in Settings::required_var_names(&profile) {
			println!("{var}");
		}
		return Ok(());
	}
	let cli = Cli::parse();
	if let Cmd::Config { cmd } = cli.cmd {
		AppConfig::handle_settings_command(cmd, cli.settings_flags);
	}
	// Exits 78 (EX_CONFIG) on a bad environment, before anything else is built.
	let settings = ev_lib::settings::or_exit(Settings::from_env());

	// Held for the life of main: dropping it flushes. A no-op without SENTRY_DSN.
	let _sentry = error_monitoring::init(&error_monitoring::Config {
		dsn: settings.sentry_dsn.clone(),
		environment: settings.app_env.clone(),
		release: error_monitoring::release_name!().map(|r| r.into_owned()),
		// the same name OTEL uses, so an issue and its trace agree on the service
		service: std::env::var("OTEL_SERVICE_NAME").ok().filter(|s| !s.trim().is_empty()),
		traces_sample_rate: error_monitoring::Config::traces_sample_rate_for(&settings.app_env),
	});
	let mut config = AppConfig::load(cli.settings_flags.clone())?;
	if let Cmd::Serve(args) = &cli.cmd {
		config.browser.dump_html = args.dump_html.clone();
	}
	let alerts = match settings.alert_webhooks()? {
		Some(webhooks) => Some(alerts::alerts(alerts::Config {
			artifacts: alerts::Artifacts::open(config.archive(settings.secrets()).artifacts_dir().expect("the server always has a data dir"))?,
			webhooks,
			service: "review_archive".to_owned(),
		})),
		None => None,
	};
	let (alert_layer, deliverer) = alerts.unzip();
	let _otel = init_tracing(&settings.app_env, alert_layer)?;

	let ended = tokio::runtime::Builder::new_multi_thread()
		.enable_all()
		.build()
		.wrap_err("building the tokio runtime")?
		.block_on(with_alerts(deliverer, run(cli, config, settings)));
	// devops holds a deployment whose container exits 78, instead of restarting it into the same failure
	if let Err(e) = &ended
		&& review_archive::remedy(e) == review_archive::Remedy::Fatal
	{
		eprintln!("Error: {}", review_archive::describe(e));
		drop((_otel, _sentry));
		std::process::exit(78);
	}
	ended
}

/// Runs `work`, delivering alerts beside it and, once it ends, the ones still queued.
async fn with_alerts<T>(deliverer: Option<alerts::Deliverer>, work: impl Future<Output = T>) -> T {
	let Some(deliverer) = deliverer else { return work.await };
	let done = tokio::sync::Notify::new();
	let work = async {
		let out = work.await;
		done.notify_one();
		out
	};
	tokio::join!(work, deliverer.run(done.notified())).0
}

/// Logs go to stderr, so a CLI command's stdout stays its output. OTLP export only when
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is set; alerts only when their webhooks are.
fn init_tracing(environment: &str, alerts: Option<alerts::AlertLayer>) -> eyre::Result<Option<ev_lib::otel::Telemetry>> {
	use tracing_subscriber::{EnvFilter, fmt, prelude::*};

	let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(option_env!("LOG_DIRECTIVES").unwrap_or("info")))?;
	let (otel_guard, otel_layers) = ev_lib::otel::telemetry(&ev_lib::otel::Config {
		environment: environment.to_owned(),
		traces_sample_rate: ev_lib::otel::Config::traces_sample_rate_for(environment),
	})
	.unzip();
	tracing_subscriber::registry()
		.with(filter)
		.with(fmt::layer().with_writer(std::io::stderr))
		.with(error_monitoring::tracing_layer())
		.with(otel_layers)
		.with(alerts)
		.init();
	Ok(otel_guard)
}

async fn run(cli: Cli, config: AppConfig, settings: Settings) -> eyre::Result<()> {
	// The image points TMPDIR into the data volume, which starts out empty; Chromium puts its
	// shared memory there (`--disable-dev-shm-usage`) and dies if the directory is missing.
	let tmp = std::env::temp_dir();
	std::fs::create_dir_all(&tmp).wrap_err_with(|| format!("creating the temp dir {}", tmp.display()))?;
	if let Cmd::CheckBrowser = cli.cmd {
		let profile = tempfile::tempdir().wrap_err("creating a throwaway profile")?;
		return Browser::new(config.browser.clone(), vec![profile.path().to_owned()]).check().await;
	}
	if let Cmd::Serve(_) = cli.cmd {
		eyre::ensure!(config.bind.is_some(), "serve needs a `bind`: the config's, or REVIEW_ARCHIVE_SERVER_BIND (the dev shell sets it)");
	}
	let archive = Archive::open(review_archive::config::Config {
		spend: cli.spend_real_quota_i_am_certain,
		..config.archive(settings.secrets())
	})
	.await?;

	match cli.cmd {
		Cmd::Config { .. } | Cmd::CheckBrowser => unreachable!("handled before the archive opens"),
		Cmd::Serve(args) => serve(archive, &config, &settings, args).await,
	}
}

async fn serve(archive: Archive, config: &AppConfig, settings: &Settings, args: ServeArgs) -> eyre::Result<()> {
	let bind = config.bind.expect("checked before the archive opened");
	if args.dev_member.is_some() {
		eyre::ensure!(bind.ip().is_loopback(), "--dev-member signs everyone in: it serves on a loopback address only, not {bind}");
		eyre::ensure!(settings.app_env != "production", "--dev-member is for development, and APP_ENV is production");
	}
	eyre::ensure!(
		!(args.no_login_check && settings.app_env == "production"),
		"--no-login-check is for development, and APP_ENV is production"
	);
	let signals = std::sync::Arc::new(worker::Signals::default());
	// the page at `/` is the dev member's only: behind the panel, the panel's page mounts the bundle
	let dev = match &args.dev_member {
		Some(held) => Some(Dev::new(dev_user(&archive, args.dev_as, dev_permissions(held)?).await?)),
		None => None,
	};
	let verifier = match &dev {
		Some(dev) => dev.verifier(),
		None => sa_auth::Verifier::new(settings.panel_keys()?, sa_auth::Service::ReviewArchive),
	};
	let page = dev.is_some();
	let app = http::router(
		http::AppState::new(archive.clone(), verifier, signals.clone(), config.http.clone()),
		config.mfe_dir.as_deref(),
		dev,
	);
	let listener = tokio::net::TcpListener::bind(bind).await.wrap_err_with(|| format!("binding {bind}"))?;
	tracing::info!(%bind, "serving");
	match (&config.mfe_dir, page) {
		(Some(_), true) => {
			println!(
				"▶ dashboard: http://{bind}/{}",
				args.dev_member.as_ref().map(|m| format!("  (signed in holding {m})")).unwrap_or_default()
			);
		}
		_ => println!("▶ API: http://{bind}/"),
	}

	let (tx, rx) = watch::channel(false);
	let mut http_rx = rx.clone();
	let http = async move {
		axum::serve(listener, app)
			.with_graceful_shutdown(async move {
				// a dropped sender means shutdown too
				let _ = http_rx.wait_for(|stop| *stop).await;
			})
			.await
			.wrap_err("HTTP server")
	};
	let signal = async move {
		shutdown_signal().await;
		tracing::info!("shutting down");
		// every receiver is alive until both futures below end, so this cannot fail meaningfully
		let _ = tx.send(true);
		Ok::<_, eyre::Report>(())
	};
	let mut leader_stop = rx.clone();
	let deliver = worker::deliver(&archive, &config.worker, rx.clone());
	let mut stop = rx.clone();
	let work = async {
		let r = worker::run(&archive, &signals, &config.worker, &config.schedule, !args.no_login_check, rx).await;
		archive.close().await;
		r
	};
	// resumable per blob: a shutdown midway leaves the rest for the next boot
	let backfill = async {
		tokio::select! {
			r = archive.backfill_blobs() => r,
			_ = stop.wait_for(|stopped| *stopped) => Ok(()),
		}
	};
	// A rolling update runs two of us on one data dir: the newcomer serves HTTP at once, the
	// background side only once the outgoing process is gone, which drops its flock.
	let lock_path = config.data_dir.join("serve.lock");
	let background = async {
		let lock = OpenOptions::new()
			.create(true)
			.write(true)
			.truncate(false)
			.open(&lock_path)
			.wrap_err_with(|| format!("opening {}", lock_path.display()))?;
		let mut logged = false;
		loop {
			match lock.try_lock() {
				Ok(()) => break,
				Err(TryLockError::WouldBlock) if !logged => {
					tracing::info!(lock = %lock_path.display(), "another serve holds the data dir: HTTP only until it exits");
					logged = true;
				}
				Err(TryLockError::WouldBlock) => {}
				Err(TryLockError::Error(e)) => return Err(eyre::Report::new(e).wrap_err(format!("locking {}", lock_path.display()))),
			}
			tokio::select! {
				() = tokio::time::sleep(Duration::from_secs(1)) => {}
				_ = leader_stop.wait_for(|stopped| *stopped) => return Ok(()),
			}
		}
		if logged {
			tracing::info!("took over the data dir");
		}
		if !args.no_login_check {
			match archive.check_signed_in(jiff::Timestamp::now()).await {
				Err(e) if review_archive::remedy(&e) != review_archive::Remedy::Fatal => report(&e, "the boot check of the Google accounts failed"), // the walks find out again; a restart would only open Maps again
				r => r?,
			}
		}
		tokio::try_join!(work, deliver, backfill)?;
		drop(lock);
		Ok(())
	};
	tokio::try_join!(http, background, signal)?;
	Ok(())
}

async fn shutdown_signal() {
	let ctrl_c = async {
		if let Err(e) = tokio::signal::ctrl_c().await {
			tracing::error!(error = %e, "listening for Ctrl-C");
			std::future::pending::<()>().await;
		}
	};
	#[cfg(unix)]
	let term = async {
		match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
			Ok(mut s) => {
				s.recv().await;
			}
			Err(e) => {
				tracing::error!(error = %e, "listening for SIGTERM");
				std::future::pending::<()>().await;
			}
		}
	};
	#[cfg(not(unix))]
	let term = std::future::pending::<()>();
	tokio::select! {
		() = ctrl_c => {}
		() = term => {}
	}
}

/// `--dev-member`'s person: the one member with GMBs, so the dashboard shows what the worker
/// scans; before anyone has one, a made-up `dev`.
async fn dev_user(archive: &Archive, named: Option<i64>, permissions: sa_auth::PermissionSet) -> eyre::Result<sa_auth::User> {
	let all = archive.members().await?;
	let chosen = match named {
		Some(id) => Some(all.into_iter().find(|m| m.id == id).ok_or_else(|| eyre::eyre!("--dev-as: nobody has person id {id}"))?),
		None => {
			let mut managing = Vec::new();
			for m in all {
				if !archive.overview(PersonId(m.id)).await?.is_empty() {
					managing.push(m);
				}
			}
			match managing.len() {
				0 => None,
				1 => managing.pop(),
				_ => eyre::bail!(
					"--dev-member: {} have GMBs; name one with --dev-as <person id>",
					managing.iter().map(|m| format!("{} (#{})", m.email, m.id)).collect::<Vec<_>>().join(", ")
				),
			}
		}
	};
	let (sub, email, name) = match chosen {
		Some(m) => {
			let person = archive.store()?.person_by_id(PersonId(m.id)).await?.expect("listed among the members");
			let sub = person.sub.ok_or_else(|| eyre::eyre!("--dev-member: {} never signed in, so has no sub to sign for", m.email))?;
			(sub, person.email, person.name)
		}
		None => ("dev".to_owned(), "dev@localhost".to_owned(), "dev".to_owned()),
	};
	Ok(sa_auth::User {
		sub,
		email,
		email_verified: true,
		name,
		permissions,
	})
}

/// `--dev-member`: an alias's `sa:review_archive:*` part, such permissions listed, or `none`.
fn dev_permissions(held: &str) -> eyre::Result<sa_auth::PermissionSet> {
	let catalog = concierge_iam::Catalog::collect("sa", 0);
	let ours = |p: &String| p.starts_with(sa_auth::Service::ReviewArchive.prefix());
	if held == "none" {
		return Ok(std::iter::empty::<String>().collect());
	}
	if let Some(members) = catalog.aliases.get(held) {
		return Ok(members.iter().filter(|p| ours(p)).cloned().collect());
	}
	let listed: Vec<String> = held.split(',').map(|p| p.trim().to_owned()).collect();
	for p in &listed {
		eyre::ensure!(
			catalog.permissions.contains(p) && ours(p),
			"--dev-member: {p} is not a sa:review_archive permission, an alias, or `none`"
		);
	}
	Ok(listed.into_iter().collect())
}

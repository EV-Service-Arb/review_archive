# Prod config, evaluated to TOML at build time and baked into the image
# (`flake.nix: prodConfig`). Secret-free — PANEL_ASSERTION_KEYS, GOOGLE_MAPS_KEY,
# the GBP_* OAuth triple, TELEGRAM_BOT_TOKEN,
# SENTRY_DSN, ALERT_WEBHOOK_{ERROR,WARN} and APP_ENV arrive
# from the container environment (`review_archive --print-required-vars` lists the required ones).
#
# Passed as `--config` explicitly: without it the binary boots on dev defaults,
# a 127.0.0.1 bind that fails the readiness probe and a data dir that is not the
# mounted volume.
{ port, chromium, mfe }:
{
  data_dir = "/data";
  # the dashboard bundle, served under /mfe/, its page at /
  mfe_dir = mfe;
  bind = "0.0.0.0:${toString port}";
  browser.executable = chromium;
  # primary first; the second takes walks only while the first is signed out of Google
  browser.profile_dirs = [ "/data/chromium-profile" "/data/chromium-profile-2" ];
  # the Pi's 1Gi burst: every memory-for-speed trade (ARCHITECTURE.md § Memory); and a longer
  # pause between scrolls, so V8 collects before the next screen loads
  browser.lean = [{ js_heap_mib = 512; } "small_young_generation" "js_lite_mode" "one_raster_thread" "low_end_device"];
  browser.step_wait = "3s";
  # members start at 0 and nothing renews: an admin assigns tokens by hand
  tokens.daily = 0;
}

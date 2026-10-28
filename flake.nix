{
  nixConfig = {
    extra-substituters = [ "https://valeratrades.cachix.org" ];
    extra-trusted-public-keys = [ "valeratrades.cachix.org-1:gXVwhzO5YB+BaiEJYT48qZgzdaErGQew6xtZcz4Fo1Q=" ];
  };

  inputs = {
    v_flakes.url = "github:valeratrades/v_flakes?ref=v1.6";
    browser_manipulation = {
      url = "github:valeratrades/browser_manipulation?ref=v0.4.6";
      inputs.v_flakes.follows = "v_flakes";
    };
  };

  outputs = { self, v_flakes, browser_manipulation }:
    let
      inherit (v_flakes) flake-utils pre-commit-hooks;
      manifest = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package;
      # The binary (crates/review_archive_server, `[[bin]] name`), the image and the
      # release all go by this name; the workspace root has no package of its own.
      pname = "review_archive";
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import v_flakes.default_nixpkgs { inherit system; };
        lib = pkgs.lib;
        rust = v_flakes.rs.default_nightly system;
        # mold is Linux-only — the adapter throws at *eval* time on Darwin, which
        # would break every target on a mac, not just the ones that link.
        stdenv = if pkgs.stdenv.isDarwin then pkgs.stdenv else pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv;

        # Single source for the container's exposed port and the prod config's
        # bind. The binary's own default (`config::DEFAULT_BIND`) is the same
        # port on 127.0.0.1.
        port = 59110;

        # nixpkgs builds Chromium for Linux only. On a mac the scanner is pointed
        # at a local Chrome through `browser.executable` in the config instead.
        chromium = lib.optional pkgs.stdenv.isLinux pkgs.chromium;

        # the Playwright driver browser_manipulation speaks to
        playwrightEnv = {
          PLAYWRIGHT_CLI_JS = "${browser_manipulation.packages.${system}.patchright}/package/cli.js";
          PLAYWRIGHT_NODE_EXE = "${pkgs.nodejs}/bin/node";
          PLAYWRIGHT_SKIP_DRIVER_DOWNLOAD = "1";
        };

        pre-commit-check = pre-commit-hooks.lib.${system}.run (v_flakes.files.preCommit { inherit pkgs; stripClaudeSignature = true; });
        rs = v_flakes.rs {
          inherit pkgs rust;
          build.workspace = {
            "./crates/review_archive" = [ "git_version" ];
            "./crates/review_archive_server" = [ "git_version" "log_directives" ];
          };
        };
        github = v_flakes.github {
          inherit pkgs pname rs;
          enable = true;
          lastSupportedVersion = "nightly-2026-09-03";
          containerRelease = { registry = "ghcr.io/ev-service-arb"; };
          jobs.default = true;
          lfs = false;
          gitignore.extra = ''
            # the data dir of a local `serve` / `scan`: the database, the PNGs and the browser profile
            /data/
          '';
        };
        readme = v_flakes.readme-fw {
          inherit pkgs pname;
          defaults = true;
          lastSupportedVersion = "nightly-1.100";
          rootDir = ./.;
          badges = [ "msrv" "loc" "ci" ];
        };
        combined = v_flakes.utils.combine { inherit rust; modules = [ rs github readme ]; };

        build_rust = v_flakes.rs.build_nightly system;
        rustPlatform = pkgs.makeRustPlatform { rustc = build_rust; cargo = build_rust; inherit stdenv; };
        # `.cargo` holds dev-only accelerators (sccache rustc-wrapper, cranelift,
        # mold) the hermetic sandbox lacks — drop it so the pure build uses nix's
        # own toolchain instead of failing on a missing `sccache` on PATH.
        pureSrc = lib.cleanSourceWith {
          src = lib.cleanSource ./.;
          filter = path: _type: baseNameOf path != ".cargo";
        };

        cargoLock = {
          lockFile = ./Cargo.lock;
          allowBuiltinFetchGit = true;
        };
        bin = rustPlatform.buildRustPackage {
          inherit pname;
          version = manifest.version;
          src = pureSrc;
          inherit cargoLock;
          cargoBuildFlags = [ "-p" "review_archive_server" ];
          nativeBuildInputs = with pkgs; [ pkg-config ];
          # ev_lib's `sentry` turns on reqwest's native-tls, which is OpenSSL on Linux
          # (Security.framework on Darwin, which needs nothing here).
          buildInputs = lib.optionals pkgs.stdenv.isLinux [ pkgs.openssl ];
          # the parser tests are `cargo test`'s job in the devShell and CI; the
          # live one needs a browser and the network, which the sandbox has neither of
          doCheck = false;
          auditable = false; # cargo-auditable doesn't support edition 2024
        };

        # Pinned to the `wasm-bindgen` crate (`=0.2.129`, review_archive_web): a skew
        # between the two is a hard error at bindgen time.
        wasm-bindgen-cli =
          let
            src = pkgs.fetchCrate {
              pname = "wasm-bindgen-cli";
              version = "0.2.129";
              hash = "sha256-pcecKQd7E8Opw6bkFoE569epUi7gh5qpQF1e5PJY6V8=";
            };
          in
          pkgs.buildWasmBindgenCli {
            inherit src;
            cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
              inherit src;
              inherit (src) pname version;
              hash = "sha256-vmUrWVU7kPJJxO5qIVeAkwQyWDELO1Z4Z5gitz2kco8=";
            };
          };
        wasmFlags = ''--cfg=web_sys_unstable_apis --cfg=getrandom_backend="wasm_js"'';

        # The dashboard bundle the binary serves under /mfe/ (`mfe_dir`). Built with the
        # dev toolchain, which carries the wasm32 target.
        mfe = (pkgs.makeRustPlatform { rustc = rust; cargo = rust; inherit stdenv; }).buildRustPackage {
          pname = "${pname}-mfe";
          version = manifest.version;
          src = pureSrc;
          inherit cargoLock;
          nativeBuildInputs = [ wasm-bindgen-cli pkgs.tailwindcss_4 ];
          buildPhase = ''
            runHook preBuild
            RUSTFLAGS='${wasmFlags}' cargo build -p review_archive_web --target wasm32-unknown-unknown --release --offline
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            bash crates/review_archive_web/package.sh target/wasm32-unknown-unknown/release/review_archive_web.wasm "$out"
            runHook postInstall
          '';
          doCheck = false;
          auditable = false;
        };

        # lays out `ra-location` in the Stats tile like the board's own tiles
        tilersPlugin = pkgs.fetchzip {
          url = "https://github.com/EV-Main/tilers/releases/download/grafana-v0.6.0/evmain-tilers-app-0.6.0.zip";
          hash = "sha256-NDxPRBj29ZmAb5ROp9SxPCSvrG0C+CVDWYPljcBeW4k=";
        };

        # Infinity's package for this platform alone: the catalog's default zip carries every platform's backend
        infinityPlugin = pkgs.grafanaPlugins.grafanaPlugin {
          pname = "yesoreyeram-infinity-datasource";
          version = "4.1.0";
          zipHash = {
            x86_64-linux = "sha256-pzgHZ2abzFzyKvBIQOTzlirus+AUTE0/VfP2zYnL06I=";
            aarch64-linux = "sha256-1fZ7v7mkleROOessTSbLB6wQH2HLMbhJdVGiqaaC/pg=";
          };
        };

        # `serve`'s `grafana.server`: `serve` gives it its socket, data dir, root URL, `RA_API` and its
        # sign-in settings; the rest is fixed here, cut to what the dashboards use (ARCHITECTURE.md § Memory).
        # `logFile`: dev's, appended to; otherwise warnings and up on serve's stderr.
        grafanaServer = { logFile }: pkgs.writeShellScript "grafana-server" ''
          export RA_GRAFANA=${./grafana}
          export GF_PATHS_LOGS="$GF_PATHS_DATA/log"
          export GF_PATHS_PLUGINS="${pkgs.linkFarm "grafana-plugins" [
            { name = "yesoreyeram-infinity-datasource"; path = infinityPlugin; }
            { name = "marcusolsson-dynamictext-panel"; path = pkgs.grafanaPlugins.marcusolsson-dynamictext-panel; }
            { name = "evmain-tilers-app"; path = tilersPlugin; }
          ]}"
          export GF_PLUGINS_ALLOW_LOADING_UNSIGNED_PLUGINS=evmain-tilers-app
          # with the sections it reads and nothing is provisioned in: missing, each is logged as an error
          export GF_PATHS_PROVISIONING="${pkgs.runCommand "grafana-provisioning" { } "cp -r ${./grafana/provisioning} $out; chmod u+w $out; mkdir $out/alerting $out/plugins"}"
          export GF_SECURITY_ALLOW_EMBEDDING=true
          export GF_PLUGINS_PREINSTALL_DISABLED=true # its plugin dir is the store's
          export GF_LIVE_MAX_CONNECTIONS=0
          export GF_ANALYTICS_REPORTING_ENABLED=false GF_ANALYTICS_CHECK_FOR_UPDATES=false GF_ANALYTICS_CHECK_FOR_PLUGIN_UPDATES=false
          export GF_UNIFIED_ALERTING_ENABLED=false GF_NEWS_NEWS_FEED_ENABLED=false GF_PUBLIC_DASHBOARDS_ENABLED=false GF_SNAPSHOTS_ENABLED=false
          export GF_QUERY_HISTORY_ENABLED=false GF_EXPLORE_ENABLED=false GF_CLOUD_MIGRATION_ENABLED=false GF_RECORDED_QUERIES_ENABLED=false
          export GF_PROFILE_ENABLED=false GF_HELP_ENABLED=false
          # its own, not the pod's, which is serve's (ARCHITECTURE.md § Memory); Infinity inherits it
          export GOMEMLIMIT=100MiB
          ${if logFile then ''
            log="''${XDG_STATE_HOME:-$HOME/.local/state}/review_archive/grafana.log"
            mkdir -p "$(dirname "$log")"
            exec ${pkgs.grafana}/bin/grafana server --homepath ${pkgs.grafana}/share/grafana >>"$log" 2>&1
          '' else ''
            export GF_LOG_MODE=console GF_LOG_LEVEL=warn
            exec ${pkgs.grafana}/bin/grafana server --homepath ${pkgs.grafana}/share/grafana
          ''}
        '';

        # `nix run .#<name> [-- <held>]`: the dashboard built (debug) into tmp/<name> and served by a
        # local `serve` at `/`, on the repo's `data/`, signed in holding `<held>` (`--dev-member`: an
        # alias, permissions comma-separated, or `none`; no panel needed). Scans run in a Chromium
        # window: a headless one gets Maps' limited view (#9). With `?grafana=true`, Grafana draws the overview and boards.
        devMfe = { name }: pkgs.writeShellApplication {
          inherit name;
          runtimeInputs = [ pkgs.git ];
          text = ''
            member="''${1:-sa:admin}"
            repo="$(git rev-parse --show-toplevel)"
            cd "$repo"
            out="$repo/tmp/${name}"
            RUSTFLAGS='${wasmFlags}' nix develop "$repo" --command bash -euc "
              cargo build -p review_archive_web --target wasm32-unknown-unknown
              bash crates/review_archive_web/package.sh target/wasm32-unknown-unknown/debug/review_archive_web.wasm '$out'
            "
            cat >"$out/config.toml" <<EOF
            data_dir = "$repo/data"
            mfe_dir = "$out"
            [grafana]
            server = "${grafanaServer { logFile = true; }}"
            root_url = "http://127.0.0.1:59110/grafana/"
            [browser]
            executable = "${pkgs.chromium}/bin/chromium"
            headful = true
            lean = []
            [tokens]
            start_cap_multiple = 2.0
            EOF
            exec nix develop "$repo" --command cargo r -p review_archive_server -- --config "$out/config.toml" serve --dev-member "$member" --no-login-check
          '';
        };

        # The image's driver: a capped heap, no optimizing JIT, collected sooner (ARCHITECTURE.md § Memory).
        # Flags, not NODE_OPTIONS, which refuses `--lite-mode` and `--optimize-for-size`; no `--jitless`, the
        # driver runs `webp_codec.wasm`.
        leanNode = pkgs.writeShellScript "lean-node" ''
          exec ${pkgs.nodejs}/bin/node --max-old-space-size=128 --max-semi-space-size=1 --lite-mode --optimize-for-size "$@"
        '';

        # A headless Chromium renders review text with whatever fonts fontconfig
        # finds; an image without any turns every screenshot into tofu boxes.
        fontsConf = pkgs.makeFontsConf {
          fontDirectories = with pkgs; [ dejavu_fonts liberation_ttf noto-fonts-color-emoji ];
        };

        # The image's Chromium, the one the driver drives: the shell has no browser UI layer to hold in memory (ARCHITECTURE.md § Memory).
        headlessShell = "${browser_manipulation.packages.${system}.headless-shell}/bin/chrome-headless-shell";

        # Secret-free prod config baked into the image; the container has no `nix`,
        # so it is evaluated to TOML here. Secrets arrive from the k8s Secret.
        prodConfig = (pkgs.formats.toml { }).generate "config.toml" (import ./deploy/config.nix {
          inherit port;
          chromium = headlessShell;
          mfe = "${mfe}";
          grafana = "${grafanaServer { logFile = false; }}";
        });

        imageEnv = playwrightEnv // {
          PLAYWRIGHT_NODE_EXE = "${leanNode}";
          FONTCONFIG_FILE = "${fontsConf}";
          # fixed, so freed image buffers return to the OS and the pod can shrink back to its base after a memory lease (examples/memory_footprint.rs)
          MALLOC_MMAP_THRESHOLD_ = "131072";
          # an error no code knows is reported with its trace, which errors only capture with this set
          RUST_LIB_BACKTRACE = "1";
        };

        # The image's binary, once it started the image's Chromium with the image's driver and config on a host
        # whose GPU fails to start, as the Pi's does: no Vulkan driver, no /dev/dri, no display.
        launchChecked = pkgs.runCommand "${pname}-launch-checked" { } ''
          export ${lib.concatStringsSep " " (lib.mapAttrsToList (k: v: "${k}=${lib.escapeShellArg v}") imageEnv)}
          export HOME=$TMPDIR VK_DRIVER_FILES=/nonexistent
          ${bin}/bin/${pname} --config ${prodConfig} check-browser
          mkdir -p $out/bin
          ln -s ${bin}/bin/${pname} $out/bin/${pname}
        '';

        containerStd = v_flakes.container.implement {
          inherit pkgs pname;
          containers."" = {
            inherit port;
            mounts = [ "/data" ];
            sqlite = [ "/data/review_archive.db" ];
            # Chromium's own, a lost sign-in being the `signed_out` halt; Grafana's, provisioned on every start
            unreplicated = [ "/data/chromium-profile" "/data/chromium-profile-2" "/data/.local/share/pki" "/data/grafana" ];
            healthPath = "/health";
            # an archive that misses a scan catches up on the next one
            criticality = "normal";
            entrypoint = [ "${launchChecked}/bin/${pname}" "--config" "${prodConfig}" "serve" ];
            workingDir = "/data";
            contents = [ pkgs.nodejs ];
            imageEnv = lib.mapAttrsToList (k: v: "${k}=${v}") (imageEnv // {
              HOME = "/data";
              # the image has no /tmp, and Chromium's shared memory goes to TMPDIR
              # (--disable-dev-shm-usage); the binary creates it on start, as the
              # volume is empty on first boot
              TMPDIR = "/data/tmp";
            });
          };
        };

        help = pkgs.writeShellApplication {
          name = "help";
          text = ''
            cat <<'EOF'
            nix develop                       toolchain, sqlite, cargo-insta (and chromium on Linux)
            nix build                         the review_archive binary
            nix build .#${pname}-container    OCI image with chromium-headless-shell (Linux only), its binary checked to start it
            nix build .#mfe                   the dashboard bundle (served under /mfe/)
            nix run .#dev-mfe [-- <held>]     the dashboard, built and served locally at /, holding those permissions
            nix run .#publish -- major|minor|patch [note]
                                              live tests, then tag the next vX.Y.Z and push it: CI ships the image
            nix run .#help                    this
            cargo test                        parser snapshots, repository, scheduler, gbp stub
            cargo test -- --ignored live      the live scans; need a browser and the network
            the CLI itself: review_archive --help
            EOF
          '';
        };
        # The version lives in the tag, not in Cargo.toml: `release-container.yml` ships the image on it.
        publish = pkgs.writeShellApplication {
          name = "publish";
          runtimeInputs = with pkgs; [ git ];
          text = ''
            part="''${1:-}"
            case "$part" in
              major | minor | patch) ;;
              *) echo "usage: nix run .#publish -- major|minor|patch [release note]" >&2; exit 1 ;;
            esac
            [ -z "$(git status --porcelain)" ] || { echo "uncommitted changes — commit or stash first" >&2; exit 1; }
            shift
            note="$*"
            nix develop -c cargo test --workspace -- --ignored

            git fetch --tags --force origin >/dev/null # not silenced: a stale tag list reuses a version
            last="$(git tag -l 'v*' --sort=-v:refname | head -n1)"
            ver="''${last#v}"; [ -n "$ver" ] || ver="0.0.0"
            ma="''${ver%%.*}"; rest="''${ver#*.}"; mi="''${rest%%.*}"; pa="''${rest##*.}"
            case "$part" in
              major) ma=$((ma + 1)); mi=0; pa=0 ;;
              minor) mi=$((mi + 1)); pa=0 ;;
              patch) pa=$((pa + 1)) ;;
            esac
            next="v$ma.$mi.$pa"
            echo "''${last:-<no tag>} → $next"

            # annotated: a lightweight tag has nowhere to put a note, and `--follow-tags` skips it
            if [ -n "$note" ]; then
              printf '%s\n\n%s\n' "$next" "$note" | git tag -a "$next" -F -
            else
              git tag -a "$next" -m "$next"
            fi
            git push origin "$next"
          '';
        };
      in
      {
        apps = {
          help = { type = "app"; program = lib.getExe help; };
          dev-mfe = { type = "app"; program = lib.getExe (devMfe { name = "dev-mfe"; }); };
          publish = { type = "app"; program = lib.getExe publish; };
        };

        packages = {
          default = bin;
          inherit bin mfe;
        } // lib.optionalAttrs pkgs.stdenv.isLinux containerStd.packages;

        # Linux-only: the image carries Chromium, which nixpkgs does not build for Darwin.
        containers = lib.optionalAttrs pkgs.stdenv.isLinux containerStd.containers;

        devShells.default =
          with pkgs;
          mkShell {
            inherit stdenv;
            shellHook =
              pre-commit-check.shellHook
              + combined.shellHook
              + ''
                cp -f ${(v_flakes.files.treefmt) { inherit pkgs; }} ./.treefmt.toml
                cp -f ${(v_flakes.files.gitattributes) { inherit pkgs; lfs = false; }} ./.gitattributes
              '';

            packages = [
              mold
              pkg-config
              openssl # sentry's native-tls
              rust
              sqlite # inspecting the archive
              cargo-insta
              wasm-bindgen-cli # the dashboard; pinned with the crate
              tailwindcss_4
            ] ++ chromium ++ pre-commit-check.enabledPackages ++ combined.enabledPackages;

            env = {
              RUST_BACKTRACE = 1;
              RUST_LIB_BACKTRACE = 0;
            } // playwrightEnv // lib.optionalAttrs pkgs.stdenv.isLinux {
              REVIEW_ARCHIVE_CHROME = "${pkgs.chromium}/bin/chromium";
            };
          };
      }
    );
}

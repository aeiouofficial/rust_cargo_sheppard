# Changelog

All notable cargo-shepherd changes are recorded here.

## 2026-05-05 - Shim coordination smoke verification

### Verified

- Redeployed `target_verify_ext_0505d\debug\shepherd.exe` (length 10 423 808
  bytes, LastWriteTime 2026-05-05 12:18:42) to `shepherd.exe`,
  `bin\shepherd.exe`, and `bin\shim\cargo.exe`.
- Live-verified the full shim coordination matrix: `shepherd run` queue,
  shim-attached drain (`cargo --version` streamed, exit 0), shim-attached
  queue under low-resource gates, CLI cancel while shim is queued (shim
  exits 1 with the daemon's cancellation message), client-disconnect
  auto-cancellation (killing the queued shim removes the job), source
  labels (`sheppard` vs `external`), and shim path is never double-counted
  as an external Cargo process.
- Documented the verified matrix in `docs/SMOKE_TEST_RESULTS.md` with the
  exact command shape used to reproduce it.

### Documentation

- Created backups in `bin/doc_backups_2026-05-05_shim/` of the previous
  `CHANGELOG.md` and `docs/SMOKE_TEST_RESULTS.md` before this update.

## 2026-05-05 - Cargo shim coordination repair

### Fixed

- Replaced default runtime killing of unmanaged `cargo.exe` roots with an attached Cargo shim path. When `shepherd.exe` is copied or linked as `cargo.exe`, raw `cargo ...` commands now queue through the daemon, stream stdout/stderr back to the caller, and return Cargo's real exit code.
- Added attached IPC messages for queued, started, output, finished, and error events so machine-readable commands such as `cargo metadata` keep working while Sheppard owns scheduling.
- Made the daemon scheduler wake periodically while jobs are queued, so CPU/RAM-gated jobs automatically start after resource pressure clears.
- Made startup process termination opt-in via `SHEPHERD_TAKEOVER_EXISTING_CARGO=1`; Sheppard no longer kills existing or future Cargo starts by default.
- Updated Windows launchers to create `bin\shim\cargo.exe`, prepend it for the launcher session, and optionally install it into the user PATH with `SHEPHERD_INSTALL_USER_SHIM=1`.

### Documentation

- Created documentation backups in `bin/doc_backups_2026-05-05_shim` before editing README, changelog, feature, reproducibility, and release changelog documentation.
- Updated README, feature, and reproducibility guidance to describe shim-based Cargo coordination and the opt-in emergency takeover flag.

## 2026-05-05 - Audit cycle and takeover hardening

### Fixed

- Added continuous daemon enforcement for unmanaged external `cargo.exe` roots after startup, while preserving Sheppard-owned builds and skipping enforcement while a managed spawn has not recorded its PID yet.
- Made startup takeover wait briefly for killed Rust build processes to exit so stale Cargo locks are less likely to survive daemon launch.
- Added the documented `Shell_NotifyIconW(NIM_SETVERSION)` step with `NOTIFYICON_VERSION_4` after adding the Windows tray icon.
- Avoided destroying the shared fallback Windows application icon; only file-loaded tray icons are destroyed.
- Made launcher packaged-executable lookup root-based so the same PowerShell launcher works from root or `bin`.
- Synchronized root source, manifests, lockfile, assets, launchers, `build_island`, release bundle, and `bin` launcher copies to v1.0.0 behavior.

### Documentation

- Created documentation backups in `bin/doc_backups_2026-05-05` before editing project docs.
- Replaced stale v0.2.0 release-bundle references with `cargo_sheppard_v1.0.0_release`.
- Added the missing UI design system document referenced by the README.
- Updated audit, feature, reproducibility, and smoke-test docs for takeover, external Cargo enforcement, tray behavior, and source-tree synchronization.

## 2026-05-05 - External Cargo adoption repair

### Fixed

- Made the daemon adopt already-running external `cargo.exe` processes into status snapshots as `external-<pid>` running jobs.
- Made daemon startup take coordinator position by terminating existing raw `cargo`/`rustc`/`rust-lld` processes before listening; set `SHEPHERD_PRESERVE_EXISTING_CARGO=1` only for emergency opt-out.
- Counted adopted external Cargo processes against active slots so Sheppard does not schedule as if the machine is idle.
- Allowed kill controls to terminate adopted external Cargo processes by their `external-<pid>` job IDs, including project-level kill matching by working directory.
- Restored the TUI ASCII wordmark while spelling `Sheppard` correctly, and sourced the displayed version from Cargo package metadata instead of a stale hardcoded value.
- Added a Windows notification-area tray icon while the daemon is active.
- Made launchers prefer the freshly built root `shepherd.exe`, rebuild automatically when source/assets are newer than the selected executable, and restart any existing daemon so takeover code always runs.

## 2026-05-04 - v1.0.0 release

### Changed

- Bumped version to 1.0.0. All smoke tests pass, 20/20 unit tests pass.

## 2026-05-04 - Working app repair pass

### Fixed

- Added a bounded Windows named-pipe client retry so CLI and TUI commands no longer hang indefinitely when the daemon is missing.
- Refreshed resource monitor data when producing status snapshots so `shepherd status` and the TUI show live CPU/RAM instead of stale zero values.
- Recomputed running job elapsed time at status snapshot time.
- Corrected kill accounting so running-job kill requests signal the runner without decrementing active slots early.
- Made persistent config commands usable when the daemon is not running by falling back to disk-backed config updates.
- Canonicalized project directories for queue, config, and kill workflows.
- Made per-project config match canonical path prefixes, with the most specific project entry winning.
- Rejected `child-jobs 0` and normalized loaded `child_jobs` values to at least one Cargo job.
- Made TUI truncation Unicode-safe so aliases or paths with non-ASCII characters cannot panic rendering.
- Let `kill`, `cancel`, and `reprioritize` accept unique job ID prefixes, matching the eight-character IDs printed by status/run output; ambiguous prefixes return a clear error.
- Made daemon-side CLI errors exit with status code 1 for status, kill, cancel, reprioritize, and config mutation commands.
- Restored Windows icon embedding in the release manifest/build script through `winres`.
- Reworked Windows launch/build scripts to use isolated target directories, retry lock failures, validate `SHEPHERD_SLOTS`, work from root or `bin`, and tolerate a locked root `shepherd.exe`.
- Synchronized the stale `build_island` and release-bundle source trees with the verified root implementation.

### Added

- Automatic `sccache` enablement for child Cargo builds when `sccache` is on `PATH` and `RUSTC_WRAPPER` is not already set.
- Root `build_shepherd.ps1` helper.
- MIT `LICENSE` file matching the Cargo manifest and README badge.
- Audit, feature, UI/design, reproducibility, and smoke-test documentation under `docs/`.
- Regression tests for unlimited slots, prefix project config, child job normalization, running elapsed time, kill-slot accounting, missing command lookup, and Unicode-safe TUI truncation.
- Regression tests for unique and ambiguous short job ID operations.

### Verified

- Root: `cargo test --bin shepherd` passed 20/20.
- Root: `cargo build --bin shepherd` passed.
- `build_island`: `cargo test --bin shepherd --target-dir target_verify_final_0504` passed 20/20.
- Release bundle: `cargo test --bin shepherd --target-dir target_verify_final_0504` passed 20/20.

## 2026-05-03 - v0.2.0

- Introduced priority queue scheduling.
- Introduced Ratatui dashboard.
- Added persistent config with project aliases and per-project settings.
- Added launcher scripts and `SHEPHERD_SLOTS` support.
- Documented `0` as unlimited job-level slot mode with CPU/RAM resource gates still active.

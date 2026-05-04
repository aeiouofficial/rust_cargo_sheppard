# Changelog

All notable cargo-shepherd changes are recorded here.

## 2026-05-04 - Working app repair pass v1.0

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
- Synchronized the stale `build_island` and `cargo_sheppard_v0.2.0_release` source trees with the verified root implementation.

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

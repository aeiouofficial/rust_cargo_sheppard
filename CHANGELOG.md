# Changelog

All notable cargo-shepherd changes are recorded here.

## 2026-05-05 - v1.2.0 — Tray UX, Console Icon & Startup Takeover Fixes

Finally fixed the tray icon on Windows. The double-click was silently failing because the message handler was truncating the event word on 64-bit Windows, and the right-click menu wasn't dispatching either. Now both work reliably—double-click restores the dashboard, right-click opens the menu.

We also fixed the console icon so Sheppard shows up properly in the taskbar and title bar instead of showing the generic Windows icon. And the TUI wordmark is back to the clean 3-line ASCII you wanted (compacted header to save space).

The big one: startup takeover now actually works by default. Previously it was opt-in (`SHEPHERD_TAKEOVER_EXISTING_CARGO=1`), so other tools like OpenLLM's launcher could still find and kill `cargo`/`rustc` processes that Sheppard wasn't monitoring. Now startup takeover runs automatically when `herd_unmanaged = true` and uses the same Rust activity matcher as live herding (`cargo`, `rustc`, `rust-lld`, `rustdoc`, `clippy-driver`, `rustfmt`, and `cargo-*` tools). You can opt out with `SHEPHERD_TAKEOVER_EXISTING_CARGO=0` if needed.

### Changed

- The release folder is now `cargo_sheppard_v1.2.0_release/` (was `v1.1.0_release/`). The source code, manifests, and headers stay synced with root.
- Tray tooltips are clearer now: `"Sheppard dashboard (double-click to open, right-click for menu)"` in TUI mode, and `"Sheppard daemon (right-click for menu)"` in daemon-only mode.
- Version bump: `1.1.0` → `1.2.0` across all three `Cargo.toml` files (root, `build_island/`, and the release folder). The TUI header and `--version` output now show `1.2.0`.

### Notes

- All 20 unit tests pass, including new coverage for the shared Rust-activity matcher that startup takeover uses.
- Release binary SHA-256 (identical across all four deployment targets): `892AC2F08D0032945838396A28222CE5E8F6501327DC6FA5E53E007951A2DEB6`.
- Doc snapshots from before this release are in `bin/doc_backups_2026-05-05_v1.2.0_pre/` and `bin/doc_backups_2026-05-05_v1.2.0_takeover_pre/`.

## 2026-05-05 - v1.1.0 — Passive Rust Herding Supervisor

The big addition is passive Rust herding. Sheppard now watches any external Rust build process (`cargo`, `rustc`, `rust-lld`, `rustdoc`, `clippy-driver`, `rustfmt`, `cargo-*`) that's running outside of its queue—so if you kick off a build from another tool, Sheppard sees it and can manage it without killing it.

Instead of just terminating processes, Sheppard uses Windows suspend/resume to pause external builds when RAM exceeds your limit (75% by default) and automatically wakes them up once usage drops back to 70%. You can configure all of this: scan interval, pause/resume thresholds, and how many external builds can run at once (`herd_max_active`, default 1). The TUI shows held jobs with a `HELD` badge, RAM reason, and process info.

We also added proper tray support: minimize the TUI to the system tray, double-click to restore it, right-click for a menu with Open and Exit options. The TUI can now auto-focus the active panel, and you can use `h`/`l` as shortcuts to switch panels alongside the arrow keys. The header now pulls the actual version number from the Cargo manifest instead of a hardcoded string, so it'll always show the right version.

There's a bunch of new config options too: `herd_unmanaged`, `herd_ram_pause_pct`, `herd_ram_resume_pct`, `herd_scan_ms`, `herd_max_active`. You can tweak all of them live with `shepherd config herd` commands, and they're saved to your config file.

On the launcher side, both `build_shepherd.bat` and `start_shepherd.bat` now delegate to PowerShell scripts instead of carrying old batch code. The IPC types got expanded to support the new external herding status info.

Bumped version to 1.1.0, and the release folder is now `cargo_sheppard_v1.1.0_release/`. All doc changes are backed up in `bin/doc_backups_2026-05-05_v1.1.0_pre/`.

## 2026-05-05 - Shim coordination smoke verification

Deployed and tested the full shim coordination flow: queue, drain, streaming, exit codes, cancellation, and source labeling all work correctly. See `docs/SMOKE_TEST_RESULTS.md` for the exact test matrix and commands.

Doc backups are in `bin/doc_backups_2026-05-05_shim/`.

## 2026-05-05 - Cargo shim coordination repair

Stopped killing external Cargo processes on startup and replaced it with an attached shim: if you copy or symlink `shepherd.exe` as `cargo.exe`, then raw `cargo ...` commands will route through Sheppard's queue, stream output back to you, and return the real Cargo exit code. So you still get Sheppard's scheduling and resource management, but tools that rely on `cargo metadata` or other machine-readable output still work.

The daemon listens for IPC events (queued, started, output, finished, error) so the machine-readable part of Cargo still works properly. The scheduler also wakes up while jobs are queued so CPU/RAM gates work—jobs start automatically once resources are available.

Startup process termination was made opt-in with `SHEPHERD_TAKEOVER_EXISTING_CARGO=1`, but v1.2.0 later flipped it back to default-on. Windows launchers now create `bin\shim\cargo.exe` and can optionally install it to your user PATH with `SHEPHERD_INSTALL_USER_SHIM=1`.

Updated the README and docs to explain the shim approach and the takeover flag.

## 2026-05-05 - Audit cycle and takeover hardening

Added continuous daemon enforcement for unmanaged external Cargo processes after startup (while being careful not to interfere with Sheppard-owned builds or still-launching managed jobs). Startup takeover now waits briefly for killed processes to exit so stale Cargo locks don't cause issues later.

Fixed the Windows tray icon registration (`Shell_NotifyIconW(NIM_SETVERSION)` with `NOTIFYICON_VERSION_4`), and made sure we don't accidentally destroy the fallback system icon—only file-loaded ones.

Made the packaged executable lookup root-based so the same PowerShell launcher works from root or `bin`. Synchronized everything: root source, manifests, lockfile, assets, launchers, `build_island`, release bundle, and `bin` launcher copies to v1.0.0 behavior.

Doc backups in `bin/doc_backups_2026-05-05`. Updated the audit, feature, reproducibility, and smoke-test docs with all the details on takeover, external Cargo enforcement, tray behavior, and source-tree sync. Cleaned up stale v0.2.0 release-bundle references and added the missing UI design system doc.

## 2026-05-05 - External Cargo adoption repair

Made the daemon adopt already-running external `cargo.exe` processes into status snapshots as `external-<pid>` running jobs instead of just killing them. The daemon now takes coordinator position on startup by terminating existing raw `cargo`/`rustc`/`rust-lld` processes before listening (set `SHEPHERD_PRESERVE_EXISTING_CARGO=1` only if you absolutely need to keep them).

Adopted external processes count against active slots so Sheppard doesn't schedule as if the machine is idle. You can kill them by their `external-<pid>` job ID or by project-level directory matching.

Fixed the TUI ASCII wordmark and made the version display come from the Cargo manifest instead of a hardcoded value. Added the Windows notification-area tray icon while the daemon is active.

Improved the launchers to prefer the freshly built root `shepherd.exe`, rebuild automatically when source/assets are newer than the selected executable, and restart any existing daemon so takeover code always runs.

## 2026-05-04 - v1.0.0 release

Version 1.0.0 is here. All smoke tests pass, 20/20 unit tests pass.

## 2026-05-04 - Working app repair pass

Fixed CLI and TUI reliability: they no longer hang indefinitely when the daemon is missing (added a bounded named-pipe client retry), and status snapshots now show live CPU/RAM instead of zeros. Kill accounting was corrected so running-job termination works properly without prematurely freeing up slots.

Config commands work even when the daemon isn't running (fall back to disk-backed config), and project paths are canonicalized so prefix matching works correctly. The TUI is now Unicode-safe and won't panic on non-ASCII characters. Short job ID prefixes work for kill/cancel/reprioritize commands, and the error messages are clear when a prefix is ambiguous.

Added automatic `sccache` support for child Cargo builds (when it's on PATH and `RUSTC_WRAPPER` isn't already set). Added a Root `build_shepherd.ps1` helper, MIT license file, and comprehensive docs (audit, features, UI design, reproducibility, smoke tests). Windows icon embedding works through `winres`. The launchers are now much more robust—they use isolated target directories, retry on lock failures, validate `SHEPHERD_SLOTS`, work from root or `bin`, and tolerate a locked `shepherd.exe`.

Synced `build_island` and the release bundle source trees with the verified root. Added regression test coverage for unlimited slots, per-project config, child job normalization, running job elapsed time, kill accounting, missing command lookup, Unicode truncation, and short job ID operations.

## 2026-05-03 - v0.2.0 handover baseline

The foundation: priority queue scheduling, Ratatui dashboard, persistent config with project aliases and per-project settings, launcher scripts, and `SHEPHERD_SLOTS` support. Also documented the `0` unlimited job-level slot mode with CPU/RAM resource gates still active.

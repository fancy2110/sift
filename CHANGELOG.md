# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-11-12

### Added

- Explorer two-pane work landed on a single full-width list: the top bar carries a Finder-style chevron path (root first), the header names the folder with its item count, recursive size and, after an explanation, what the folder is for.
- One press of AI 解读 rescans the folder and asks the model for a per-child verdict (Deletable / Careful / Keep) shown at the right edge of each row, with a purpose tooltip; old directories now carry how long they have been unused, and folders untouched for 180+ days are treated as candidates for attention.
- Directory totals include every subdirectory; listings and recursive sizes share a persistent global cache (30 min) so one scan serves every view and survives restarts. Cleanup and manual rescans invalidate it.
- Routines have an enable switch; mined suggestions are inserted into the list once, disabled, and deleted suggestions never return.
- Storage health moved into the dashboard tile: health state, free percentage, free/used bytes and a usage bar, refreshed at launch and every minute.
- Custom analysis prompt ("skill") in settings, appended to directory analysis and routine drafts, plus provider presets for Ollama, OpenRouter and SiliconFlow behind a dropdown.
- Protected folders: a list saved in settings; the explorer refuses to add them and routine drafts reject them.
- Default weekly routines are seeded once for Downloads and Desktop screenshots (approve mode, editable, deletable).

### Fixed

- Breadcrumb no longer repeats the volume name shown by the disk picker, and its segments interlock instead of leaving gaps.
- Selected AI profile card uses a softer border and no doubled focus outline.

## [0.4.0] - 2026-11-11

### Added

- Full app uninstall flow in the UI: build a plan, choose artifacts, move them to Sift's quarantine and restore them later.
- Shared-path detection: a path used by another installed app is downgraded from auto-safe to review and labeled in the plan.
- Homebrew Cask `zap` recipes: the planner probes the installed cask on demand, parses its trash/rmdir/delete entries and merges them into the plan.
- Package-manager action runner: a closed command catalog (`brew cleanup`, `pnpm store prune`, `npm cache verify`, `pip cache purge`, ...), no shell, static arguments and a timeout. Every run requires confirmation and updates the hint lifecycle.
- Model maintenance suggestions: the prompt can request a `maintenance` array; answers are parsed, restricted to the closed action vocabulary and stored as `Proposed` hints.
- Linux app inventory from `.desktop` entries (including flatpak exports) and Windows app inventory from the registry uninstall keys.
- Event-driven environment invalidation: a watcher on the application directories marks the five-minute cache dirty when apps are installed or removed.

### Changed

- Maintenance hints now carry model reason/impact text, success/failure counts and a last-validated timestamp.
- Successful package-manager actions move the matching hint to `Validated`; failures move it to `Stale`.
- The uninstall planner merges recipe evidence with conventional paths and keeps the stronger source.

### Known limitations

- Windows registry and Linux `.desktop` inventory are best-effort and not exercised on those platforms by CI; macOS remains the primary target.
- The package-manager runner covers a conservative subset of actions; project-scoped commands and system package managers are not runnable from the UI.
- Homebrew zap lookup guesses the cask slug from the app name and only covers installed casks.
- System-scope and admin-only uninstall artifacts are shown but not executed.
- The environment watcher is non-recursive on the application directories and uses a dirty flag, not a full event pipeline.

## [0.3.0] - 2026-11-10

### Added

- New `sift-apps` environment inventory: macOS application identity, detected package managers and their versions, and project manifests/lockfiles.
- A redacted `ScanContext` is now part of every analysis; the AI prompt receives a bounded environment summary (apps, package managers, projects) without real home paths or file contents.
- Environment snapshots are cached for five minutes so repeated analyses do not re-probe every app and toolchain.
- Maintenance hints derived from package-manager capabilities (`cleanCache`, `pruneStore`, `removeOldVersion`, ...), persisted with a `Proposed → Validated → Rejected/Stale` lifecycle.
- Read-only app uninstall planning: conventional support/cache/preferences/container/log paths classified as auto-safe, review or keep.
- Reversible uninstall execution: selected artifacts move to a Sift quarantine with a manifest, and can be restored; paths outside the plan are rejected.
- New Tauri commands: `environment_summary`, `refresh_environment`, `plan_app_uninstall`, `execute_app_uninstall`, `restore_app_uninstall`.
- Dashboard "Environment" panel showing detected apps, package managers and maintenance hints.
- Tests for context redaction/fingerprints, project detection, maintenance hint selection, uninstall planning and quarantine round-trips.

### Changed

- AI adjudication can now use the environment context; the set of candidates allowed to leave the machine is unchanged.
- The local store gains `environment.json` alongside verdicts, cleanables, decisions, settings, history and routines.

### Known limitations

- Application inventory is macOS-only in this release; Windows and Linux return an empty app list while the context shape stays stable.
- Package-manager capabilities are catalogued and displayed as hints, but no command runner UI ships yet; nothing is executed automatically.
- Model-produced maintenance hints are not parsed yet; hints in this release come from the local capability catalog.
- Uninstall execution has backend commands and a quarantine/undo path, but no dedicated UI flow; system-scope and admin-only artifacts are not executed.
- Environment refresh is time-based (five minutes), not yet event-driven on app install/uninstall.

## [0.2.0] - 2026-11-06

### Added

- Finder-style column browser in Explorer, with a linked two-pane layout and a chevron path bar.
- Per-child AI verdicts inline on every Explorer row, plus an AI explanation for the selected folder.
- Folder-total cache with an explicit rescan, and auto-scanning of the selected folder's total.
- Size sorting and visible analysis progress in list views.
- Custom AI analysis prompt, a provider dropdown and more provider presets.
- Scheduled tasks can run analyse-only or auto-execute, and scheduled findings stay pending until you act.

### Changed

- Explorer keeps the disk selection and breadcrumb trail; the left pane shows the parent of the right directory.
- Settings: AI options are consolidated, and auto-clean moved to the bottom of the group.
- Explorer refreshes whenever its tab becomes active.
- Release builds now use the macOS 15 runner (macOS 14 images were retired on 2026-11-02).

### Fixed

- Tray menu-bar glyph now fills its canvas and is easier to see.
- Root is the first breadcrumb; the duplicated volume label is gone.
- AI progress is shown on the button, and answers follow the UI language.
- Explorer rows stay consistent when the left pane or breadcrumb changes.

### Known limitations

- macOS is the primary tested platform; Windows and Linux builds are available but still being validated.

## [0.1.0] - 2026-10-26

### Added

- Disk and directory scanning with live progress and cancellation.
- Tree browser with size shares and a smart list with a "selected only" filter.
- Trash-only cleanup with per-item results, cancellation and history.
- Local rule nomination plus optional AI adjudication that can only confirm or soften a verdict.
- Multiple AI provider profiles with per-profile keys: Volcengine ARK (platform and Agent Plan), DeepSeek, OpenAI, Anthropic Claude.
- Scheduled tasks: path routines with daily/weekly/monthly cadence, and directory + file-name regex rules with an optional age filter.
- Three ways to create a scheduled task: manual form, rule matching, and an AI-generated draft.
- Menu-bar free-space monitoring, low-space warnings and scheduled cleanup.
- Eight interface languages: zh-CN, zh-TW, en, ja, ko, hi, id, ru.
- Lucide icon set with light and dark themes.

### Changed

- Saved routines and cleanup rules share one "Scheduled tasks" list and one creation dialog.
- AI provider keys are stored in `~/Library/Application Support/Sift/` (mode 0600) instead of the macOS Keychain.
- AI requests run off the main thread with a 120 s timeout and readable transport errors.

### Fixed

- Deleting files updates ancestor directory sizes and volume capacity.
- Cancelling a rescan no longer leaves stale state.
- macOS trash moves no longer trigger per-item sounds.
- Fingerprint checks compare logical sizes, so files are deletable.
- Cadence labels and AI provider endpoint normalization are corrected.

### Known limitations

- macOS only; Windows and Linux are still in progress.
- AI features require your own API key and are billed by the provider.

[Unreleased]: https://github.com/fancy2110/sift/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/fancy2110/sift/releases/tag/v0.2.0
[0.1.0]: https://github.com/fancy2110/sift/releases/tag/v0.1.0

# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

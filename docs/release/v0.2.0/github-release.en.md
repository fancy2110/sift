## Highlights

0.2.0 turns Explorer into a real Finder-style file browser and puts the AI verdict directly on every row.

- Column browser, linked two-pane layout and a breadcrumb path bar, with automatic folder totals and caching.
- Per-child AI verdicts inline, plus an AI explanation for the selected folder.
- Sort lists by size and watch analysis progress in place.
- Scheduled tasks can run analyse-only or auto-execute; findings wait for your confirmation first.
- Custom AI analysis prompt and more provider presets.

## What's Changed

### Added
- Finder-style column browser, linked two-pane layout and breadcrumb path bar in Explorer.
- Per-child AI verdicts inline, and an AI explanation for the selected folder.
- Folder-total cache with explicit rescan; the selected folder is scanned automatically.
- Size sorting and visible analysis progress in lists.
- Custom AI analysis prompt, provider dropdown and more provider presets.
- Scheduled tasks switch between analyse-only and auto-execute; results stay pending until you act.

### Changed
- Explorer keeps the disk selection and breadcrumb trail; the left pane shows the parent of the right directory.
- AI options are consolidated in Settings, with auto-clean moved to the bottom of the group.
- Explorer refreshes whenever its tab becomes active.
- Release builds moved to the macOS 15 runner (macOS 14 images were retired on 2026-11-02).

### Fixed
- Tray menu-bar glyph fills its canvas and is easier to see.
- Root is the first breadcrumb; the duplicated volume label is gone.
- AI progress appears on the button, and answers follow the UI language.
- Explorer rows stay consistent when the left pane or breadcrumb changes.

## Full Changelog

https://github.com/fancy2110/sift/compare/v0.1.0...v0.2.0

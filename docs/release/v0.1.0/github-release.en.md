## Highlights

First public release of Sift: a macOS disk-space manager where every deletion goes to the Trash and the model can only corroborate a verdict, never authorize one.

- ~2.3 s to scan 610k files / 57 GB with batched `getattrlistbulk(2)`.
- Trash-only cleanup with per-item results and history.
- Scheduled tasks created manually, by regex rule, or from an AI draft.
- Multiple AI providers with independent keys, switchable at any time.

## What's Changed

### Added
- Disk and directory scanning with progress and cancel.
- Tree browser, size shares and a "selected only" smart filter.
- Trash-only cleanup, per-item results and history.
- AI adjudication across Volcengine ARK (platform + Agent Plan), DeepSeek, OpenAI and Anthropic Claude.
- Scheduled tasks: cadence-based routines and directory + file-name regex rules.
- Manual, rule and AI creation modes in one dialog.
- Menu-bar monitoring, low-space warnings and scheduled cleanup.
- Eight interface languages and Lucide icons.

### Fixed
- Tree sizes and volume capacity update after deletions.
- Cancelling a rescan no longer leaves stale state.
- macOS trash moves are silent.
- Fingerprint checks use logical sizes.

## Full Changelog

https://github.com/fancy2110/sift/compare/v0.0.0...v0.1.0

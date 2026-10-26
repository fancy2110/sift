# Sift — Product Description

Sift is a disk-space manager for macOS: it scans the disk, finds what can be cleaned, removes it safely, and turns those conclusions into reusable scheduled tasks. It is not a one-click "empty everything" tool — every deletion has a reason, a record and a way back.

## Who it is for

- Developers and creators who keep running out of space but do not dare delete blindly;
- People who want cache, build-output and old-installer cleanup to become a routine;
- Anyone who wants AI to assist a decision without handing deletion over to a model.

## What it does

**Scan and locate.** Scan a whole disk or a directory with live progress and cancel; browse the tree with size shares to find large folders quickly.

**Smart cleanup.** Local rules nominate candidates; the model returns cleanability, a safety level and a reason, and can only confirm or soften a verdict — it never marks unknown data as safe on its own. Confirmed items move to the Trash together, with per-item results, cancellation and history.

**Scheduled tasks.** One list manages all recurring cleanup, with run, edit and delete. Create them by hand, by rule (directory + file-name regex + age), or by describing the intent to the AI. Routines run daily, weekly or monthly and default to approve-before-run.

**Background watch.** Lives in the menu bar, monitors free space and warns at thresholds; optional daily scheduled cleanup.

## Safety and privacy

- Trash only — no permanent delete inside the app;
- Fingerprints are re-checked before removal, so a changed path is skipped;
- Rules and routines always require confirmation before deleting;
- AI is opt-in and receives only the necessary text and redacted metadata, never file contents;
- API keys stay on this machine (`~/Library/Application Support/Sift/`, mode `0600`), never in settings or the repository.

## AI providers

Multiple profiles can coexist, each with its own key, and you switch by selecting one:

| Provider | Endpoint |
| --- | --- |
| Volcengine ARK | `/api/v3` (platform key) or `/api/plan/v3` (Agent Plan key) |
| DeepSeek | `https://api.deepseek.com/chat/completions` |
| OpenAI | `https://api.openai.com/v1/chat/completions` |
| Anthropic Claude | `https://api.anthropic.com/v1/messages` |

Add a profile under Settings → AI, fill in the endpoint, model and that provider's key, and select the profile you want to use. Model usage is billed by the provider.

## Typical flow

1. Pick a starting directory and scan on first launch;
2. Review suggestions in Smart, select items (optionally filter to selected only);
3. Confirm cleanup — files go to the Trash;
4. For recurring cleanup, add a scheduled task (manual, rule or AI);
5. Enable daily scheduled cleanup and low-space warnings in Settings.

## Requirements

- macOS 13 or later (Apple Silicon / Intel);
- Full Disk Access for the first scan;
- Network and a valid API key for AI features.

## Languages

Simplified Chinese, Traditional Chinese, English, Japanese, Korean, Hindi, Indonesian, Russian.

## Version

Current version: v0.1.0. See `docs/release/v0.1.0/` for release notes.

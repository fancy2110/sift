# Sift v0.1.0 Release Notes

Released 2026-10-26 · Platform: macOS (Apple Silicon / Intel)

The first usable release of Sift: a macOS disk-space manager that turns scanning, judging, cleaning and reviewing into one loop. Deletions always go to the Trash, and the model can only corroborate a verdict — it can never authorize one on its own.

## Highlights

- **Fast**: batched `getattrlistbulk(2)` enumeration — ~2.3 s for 610k files / 57 GB; a full volume of 13.9M files in about three minutes.
- **Lean**: fixed 72-byte directory nodes; the tree for 610k files stays around 7 MB.
- **Safe by default**: Trash only, per-item results, fingerprint checked before removing, cancellable at any time.
- **AI only tightens**: local rules nominate candidates; the model can confirm or soften a verdict but never mark unknown data as safe.
- **Durable conclusions**: the cleanable list is persisted, so it survives restarts and feeds habits, routines and scheduled cleanup.

## Features

### Scan and explore
- Whole-disk or single-directory scans with live progress and cancel.
- Tree browser with size shares to find large directories quickly.
- Smart list sorted by size and cleanability, with a "selected only" filter.

### Cleanup
- Confirmed items move to the Trash together, without per-item sounds.
- Per-item success / failure / cancellation; one failure does not block the rest.
- History of finished sessions; deletions update the tree and volume sizes immediately.

### AI assistance
- Local rules nominate candidates; the model returns cleanability, safety and a reason, and is only used to confirm or soften.
- Multiple provider profiles, each with its own key, switchable at any time:
  - Volcengine ARK (platform API and **Agent Plan** endpoints)
  - DeepSeek
  - OpenAI
  - Anthropic Claude (`/v1/messages`)
- The standalone assistant was folded into routines: describe an intent and get a routine draft.

### Routines and rules
- One "Scheduled tasks" list with run, edit and delete.
- Three ways to create: manual, rule matching, AI draft.
- Routines run on a daily / weekly / monthly cadence over chosen directories and default to approve-before-run.
- Rules match directory + file-name regex with an optional age filter, run with the daily schedule, and are capped at 2,000 files / 20 GB per run; symlinks are never followed.
- Suggestions mined from past deletions can be accepted or ignored.

### System and UI
- Menu-bar presence: free-space monitoring, low-space warnings, scheduled cleanup.
- Eight interface languages: Simplified and Traditional Chinese, English, Japanese, Korean, Hindi, Indonesian, Russian.
- Consistent Lucide icons; light and dark themes.

## Safety and privacy

- Deletions go to the system Trash; the app has no "delete permanently".
- AI requests send only the necessary text and redacted metadata — never file contents — after explicit opt-in.
- API keys stay on this machine (`~/Library/Application Support/Sift/`, mode `0600`); they are not written to settings or shipped with the repository.
- Items created by routines and rules always run in approve-before-delete mode.

## Known limitations

- macOS only for now; Windows and Linux are still in progress.
- AI features require your own API key and are subject to the provider's billing and terms.
- Claude model names must match what your account can access.
- The last-run time is not yet shown on every scheduled task entry.

## Upgrade and verification

- Settings, history and scheduled tasks remain compatible across versions; a single old AI configuration migrates into the first profile.
- Self-check: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `pnpm check`.

## Feedback

Please include the steps you took, your macOS version and relevant logs in a repository issue.

## Highlights

0.4.0 closes the loop opened in 0.3.0: uninstall plans can be executed and restored, package-manager maintenance can be run item by item, and Homebrew recipes plus model suggestions feed the same knowledge base.

- App uninstall: plan → select → move to Sift quarantine → restore later.
- Shared-path detection: a path used by another installed app is downgraded to review.
- Homebrew Cask `zap` recipes: read installed casks on demand and merge trash/rmdir/delete paths into the plan.
- Package-manager maintenance: `brew cleanup`, `pnpm store prune`, `npm cache verify`, `pip cache purge` and more, each confirmed, no shell, with a timeout.
- Model maintenance suggestions: the AI may return a closed action enum, stored as `Proposed`, never executed directly.
- Windows/Linux app inventory: Linux `.desktop`/flatpak and Windows registry uninstall keys.
- Application-directory changes invalidate the environment cache immediately instead of waiting five minutes.

## What changed

### Uninstall flow

- Every app in the dashboard Environment panel has a "Manage" entry.
- The plan groups cache, saved state, support, containers, preferences, logs and launch agents; cache/saved state are selected by default, everything else needs confirmation.
- The plan is rebuilt before execution and every requested path must belong to it; selected artifacts move to Sift quarantine with a manifest.
- Restore is available immediately; existing originals are never overwritten.

### Package-manager maintenance

- New catalog: Homebrew, npm, pnpm, yarn, pip, conda, go, gem, composer, scoop, flatpak.
- Only `(pm_id, action)` is accepted; program, arguments and scope come from the local catalog; `Danger` actions are refused.
- Five-minute timeout; success marks the hint `Validated`, failure marks it `Stale`.
- Each hint has a "Run" button with a second confirmation.

### Recipes and sharing

- `brew info --cask --json=v2 <slug>` is read on demand; `zap` trash/rmdir/delete paths are parsed and system-level paths are filtered out.
- Recipe paths merge and dedupe with conventional paths; recipe provenance is shown.
- Another app's name or bundle id in a path marks `sharedWith` and downgrades it to review.

### Model maintenance

- The prompt may request a `maintenance` array limited to cleanCache / pruneStore / dedupe / removeOrphan / removeOldVersion / verifyIntegrity.
- Unknown actions, invalid subjects and empty reasons are dropped; hints always enter as `Proposed`.

### Cross-platform and refresh

- Linux: `/usr/share/applications`, `~/.local/share/applications`, flatpak export directories.
- Windows: HKCU/HKLM registry uninstall keys (best-effort).
- Watches `/Applications`, `~/Applications`, Linux app dirs and Windows Programs/Program Files; a change invalidates the cache.

## Installation

- **macOS**: open the dmg and drag Sift into Applications. The app is signed and notarized; the dmg itself is not separately notarized, which is expected.
- **Windows**: the installer is unsigned for now; SmartScreen may warn — choose "More info → Run anyway".
- **Linux**: make the AppImage executable, or install the deb.

## Known limitations

- Windows registry and Linux `.desktop` inventory are best-effort; CI only verifies compilation/tests, not a real inventory on those platforms.
- The package-manager runner covers a conservative subset; project-scoped commands and system package managers are not runnable from the UI.
- Homebrew zap covers installed casks only and guesses the cask slug from the app name.
- System/admin uninstall artifacts are shown but not executed.
- The environment watcher is a non-recursive dirty-flag watcher, not a full event pipeline.

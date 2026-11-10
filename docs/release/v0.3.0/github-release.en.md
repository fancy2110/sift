## Highlights

0.3.0 starts teaching Sift what is installed on the machine: applications, package managers and projects become part of every analysis instead of isolated directories.

- New environment inventory: macOS app identity, package-manager versions, project manifests/lockfiles.
- Every analysis receives a redacted environment context, so the AI can explain which app or toolchain a path belongs to.
- Maintenance hints begin to sediment: `cleanCache`, `pruneStore`, `removeOldVersion`, with a state lifecycle.
- App uninstall planning: support, cache, preferences, containers and logs are classified as auto-safe, review or keep.
- Reversible uninstall: selected artifacts move to a Sift quarantine with a manifest and can be restored; paths outside the plan are rejected.
- A new Environment panel on the dashboard lists apps, package managers and maintenance hints.

## What changed

### Environment context

- `ScanContext` carries apps, package managers and projects; paths are redacted to `~/...` and file contents never leave the machine.
- The inventory is cached for five minutes, so repeated analyses do not re-probe every app and toolchain.
- The context enters the AI system prompt, capped at 40 apps / 20 package managers / 20 projects.

### Maintenance knowledge

- Hints follow `Proposed → Validated → Rejected/Stale`.
- A hint that disappears becomes `Stale`; validated/rejected states survive rescans.
- A hint is data: actions are a closed enum with no shell command.

### App uninstall

- Read-only plan: conventional `Application Support`, `Caches`, `Preferences`, `Containers`, `Saved Application State`, `Logs` and `LaunchAgents`.
- Dispositions: cache/saved state are auto-safe; support/containers/preferences need review; missing paths are omitted.
- Execution: selected artifacts move to Sift's quarantine (same-volume rename, copy fallback), recorded in a manifest; restore moves them back.
- Safety: the plan is rebuilt from the app key, and every requested path must belong to that plan.

### UI

- Environment panel on the dashboard with app/manager/project counts and hint states.
- Manual refresh of the environment inventory.

## Installation

- **macOS**: open the dmg and drag Sift into Applications. The app is signed and notarized; the dmg itself is not separately notarized, which is expected.
- **Windows**: the installer is unsigned for now; SmartScreen may warn — choose "More info → Run anyway".
- **Linux**: make the AppImage executable, or install the deb with your package manager.

## Known limitations

- Application inventory is macOS-only in this release; Windows/Linux return an empty app list while the context shape stays stable.
- Package-manager capabilities are catalogued and shown as hints, but no command-runner UI ships yet; nothing executes automatically.
- Model-produced maintenance hints are not parsed yet; hints come from the local capability catalog.
- Uninstall execution has backend commands and a quarantine/undo path, but no dedicated UI flow; system/admin artifacts are not executed.
- Environment refresh is a five-minute time cache, not yet event-driven on app install/uninstall.

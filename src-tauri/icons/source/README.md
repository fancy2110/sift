# Sift app icon — vector sources

Brand mark **Reclaim Wedge**: six bold disk-usage wedges, with the top wedge
pulled out and glowing cyan→violet (the space the AI found to reclaim) and a
small spark in the hub. One focal point, thick filled shapes, readable at 16px.

## Files

| File | Use |
|---|---|
| `app-icon.svg` | Default master, 1024, **full-bleed square** (iOS/Android mask it) |
| `app-icon-desktop.svg` | Same art clipped to the **squircle** (transparent corners) → macOS `.icns`, Windows `.ico`, Linux PNG |
| `app-icon-dark.svg` | Dark appearance (shared composition) |
| `app-icon-tinted.svg` | iOS 18+ tinted: transparent bg, single luminance channel, spark knocked out |
| `tray-template.svg` | macOS menu-bar template, black alpha mask (OS inverts for light/dark bars) |
| `android-bg.svg` / `android-fg.svg` / `android-monochrome.svg` | Android adaptive layers (1080) |
| `icon-manifest.json` | Tauri manifest (default + android bg/fg/monochrome) |

## Regenerate the raster set

Render the SVGs to 1024 PNGs first (any SVG rasterizer), then:

```bash
# iOS/Android (full-bleed default + adaptive layers)
npx tauri icon src-tauri/icons/source/icon-manifest.json -o /tmp/icons-mobile

# macOS / Windows / Linux (squircle)
npx tauri icon src-tauri/icons/source/app-icon-desktop.png -o /tmp/icons-desktop

# tray: render tray-template.svg at 128px as icons/tray-template.png
```

`app-icon.svg` replaces the old `app-icon-source.html`. Platform assets live in
`src-tauri/icons/` (root PNGs/ico/icns, `ios/`, `android/`).

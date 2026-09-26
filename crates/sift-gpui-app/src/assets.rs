//! The design稿's own icon set.
//!
//! The design does not use an icon library: `src/lib/components/Icon.svelte`
//! carries hand-written 24×24 paths stroked at 1.7 with round caps. GPUI Kit's
//! components draw from the Lucide catalog instead (stroke 2, different
//! geometry), which is why the first version's folders and disks looked like a
//! different product. The paths are transcribed into
//! `assets/sift-icons/*.svg` verbatim and served through this asset source, so
//! `svg("sift-icons/folder.svg")` draws the design's glyph, not a lookalike.
//!
//! Anything that is not one of these icons falls through to GPUI Kit's assets,
//! so the components keep their own icons.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// Where the design's icons live inside the asset namespace.
pub const PREFIX: &str = "sift-icons";

/// The icons, embedded so the application has no runtime asset directory.
const ICONS: &[(&str, &str)] = &[
    ("alert", include_str!("../assets/sift-icons/alert.svg")),
    ("bolt", include_str!("../assets/sift-icons/bolt.svg")),
    ("check", include_str!("../assets/sift-icons/check.svg")),
    ("chevron-down", include_str!("../assets/sift-icons/chevron-down.svg")),
    ("chevron-right", include_str!("../assets/sift-icons/chevron-right.svg")),
    ("chevron-up", include_str!("../assets/sift-icons/chevron-up.svg")),
    ("clock", include_str!("../assets/sift-icons/clock.svg")),
    ("external-drive", include_str!("../assets/sift-icons/external-drive.svg")),
    ("file", include_str!("../assets/sift-icons/file.svg")),
    ("folder", include_str!("../assets/sift-icons/folder.svg")),
    ("hard-drive", include_str!("../assets/sift-icons/hard-drive.svg")),
    ("home", include_str!("../assets/sift-icons/home.svg")),
    ("info", include_str!("../assets/sift-icons/info.svg")),
    ("layers", include_str!("../assets/sift-icons/layers.svg")),
    ("lock", include_str!("../assets/sift-icons/lock.svg")),
    ("refresh", include_str!("../assets/sift-icons/refresh.svg")),
    ("search", include_str!("../assets/sift-icons/search.svg")),
    ("settings", include_str!("../assets/sift-icons/settings.svg")),
    ("spark", include_str!("../assets/sift-icons/spark.svg")),
    ("trash", include_str!("../assets/sift-icons/trash.svg")),
    ("x", include_str!("../assets/sift-icons/x.svg")),
];

/// The asset path for one of the design's icons.
pub fn path(name: &str) -> SharedString {
    SharedString::from(format!("{PREFIX}/{name}.svg"))
}

/// The application's asset source: the design's icons first, GPUI Kit's after.
pub struct SiftAssets;

impl AssetSource for SiftAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix(PREFIX) {
            // The `sift-icons/` namespace is this application's, so a name that is
            // not in the table is "no such icon" (a missing glyph) rather than an
            // error forwarded from another source.
            let name = name.trim_start_matches('/').trim_end_matches(".svg");
            return Ok(ICONS
                .iter()
                .find(|(icon, _)| *icon == name)
                .map(|(_, body)| Cow::Borrowed(body.as_bytes())));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.starts_with(PREFIX) {
            return Ok(ICONS
                .iter()
                .map(|(icon, _)| SharedString::from(format!("{PREFIX}/{icon}.svg")))
                .collect());
        }
        gpui_kit::assets::Assets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_design_icon_is_served() {
        let assets = SiftAssets;
        for (name, body) in ICONS {
            let path = format!("{PREFIX}/{name}.svg");
            let loaded = assets
                .load(&path)
                .expect("no error")
                .unwrap_or_else(|| panic!("{path} must load"));
            let text = std::str::from_utf8(&loaded).expect("utf-8");
            assert!(text.contains("<path d=\""), "{name} has a path");
            // The design's own stroke, not Lucide's 2.
            assert!(text.contains("stroke-width=\"1.7\""), "{name} keeps the design's stroke");
            assert!(text.contains("stroke=\"currentColor\""), "{name} takes the text colour");
            assert_eq!(text, *body);
        }
    }

    #[test]
    fn an_unknown_sift_icon_falls_through_rather_than_panicking() {
        // A typo must surface as a missing glyph, not as a crash on startup.
        let loaded = SiftAssets.load("sift-icons/nope.svg");
        assert!(loaded.is_ok());
    }

    #[test]
    fn the_kit_s_own_icons_still_resolve() {
        // The components need their catalog: falling through is what keeps a
        // button's spinner working.
        let loaded = SiftAssets
            .load("icons/check.svg")
            .expect("no error")
            .expect("the kit catalog must still answer");
        assert!(!loaded.is_empty());
    }

    #[test]
    fn listing_the_design_icons_reports_every_file() {
        let listed = SiftAssets.list(PREFIX).expect("list");
        assert_eq!(listed.len(), ICONS.len());
        assert!(listed.iter().any(|path| path.ends_with("folder.svg")));
    }
}

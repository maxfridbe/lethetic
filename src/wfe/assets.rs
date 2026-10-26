//! Compile-time embedded, exact-allowlist SPA assets.

use include_dir::{Dir, include_dir};

// build.rs tracks the TypeScript, static resources, and vendored dependencies.
// Cargo regenerates both the distribution and allowlist before compiling us.
static WEB_DIST: Dir<'static> = include_dir!("$OUT_DIR/web-dist");
include!(concat!(env!("OUT_DIR"), "/web-assets.rs"));

pub struct EmbeddedAsset {
    pub bytes: &'static [u8],
    pub content_type: &'static str,
}

pub fn get(path: &str) -> Option<EmbeddedAsset> {
    if !ALLOWED_ASSETS.contains(&path) {
        return None;
    }
    let file = WEB_DIST.get_file(path)?;
    let content_type = if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".txt") {
        "text/plain; charset=utf-8"
    } else {
        return None;
    };
    Some(EmbeddedAsset {
        bytes: file.contents(),
        content_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn embedded_paths(directory: &'static Dir<'static>, paths: &mut BTreeSet<String>) {
        paths.extend(
            directory
                .files()
                .map(|file| file.path().to_string_lossy().into_owned()),
        );
        for child in directory.dirs() {
            embedded_paths(child, paths);
        }
    }

    #[test]
    fn embedded_distribution_matches_the_runtime_allowlist() {
        let mut embedded = BTreeSet::new();
        embedded_paths(&WEB_DIST, &mut embedded);
        let allowed = ALLOWED_ASSETS
            .iter()
            .map(|path| (*path).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(embedded, allowed);
        for path in ALLOWED_ASSETS {
            let asset = get(path).unwrap_or_else(|| panic!("missing embedded asset {path}"));
            assert!(!asset.bytes.is_empty(), "empty embedded asset {path}");
        }
    }

    #[test]
    fn unknown_and_traversal_assets_are_not_served() {
        for path in [
            "Cargo.toml",
            "../Cargo.toml",
            "src/app.tsx",
            "lib/vendor-manifest.json",
        ] {
            assert!(get(path).is_none(), "served unexpected asset {path}");
        }
    }
}

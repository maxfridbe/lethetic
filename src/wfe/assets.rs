//! Compile-time embedded, exact-allowlist SPA assets.

use include_dir::{Dir, include_dir};

static WEB_DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

// `include_dir!` does not reliably invalidate an incremental Cargo build when
// only a file inside the directory changes. The anonymous `include_bytes!`
// constants make every allowlisted asset an explicit compiler dependency while
// `WEB_DIST` remains the single runtime copy. `build-web.sh` also touches this
// module after installing normalized-mtime assets so Cargo reevaluates them.
macro_rules! tracked_assets {
    ($($path:literal),+ $(,)?) => {
        const ALLOWED_ASSETS: &[&str] = &[$($path),+];
        $(const _: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/web/dist/",
            $path
        ));)+
    };
}

include!("assets_manifest.rs");

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

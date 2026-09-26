# Vendored web dependencies

These browser assets are committed so the SPA has no runtime CDN fetch, project `package.json`, lockfile, `node_modules`, or npm install step. `vendor-manifest.json` records exact sources, archive hashes, selected files, licenses, sizes, and per-file SHA-256 values.

Verify from this directory:

```bash
sha256sum --check SHA256SUMS
```

## Manual refresh

Download the exact upstream archives without installing packages:

```bash
curl -fL https://registry.npmjs.org/snabbdom/-/snabbdom-3.6.4.tgz -o /tmp/snabbdom-3.6.4.tgz
curl -fL https://registry.npmjs.org/marked/-/marked-18.0.11.tgz -o /tmp/marked-18.0.11.tgz
curl -fL https://github.com/ryanoasis/nerd-fonts/releases/download/v3.5.0/JetBrainsMono.tar.xz -o /tmp/JetBrainsMono-v3.5.0.tar.xz
sha256sum /tmp/snabbdom-3.6.4.tgz /tmp/marked-18.0.11.tgz /tmp/JetBrainsMono-v3.5.0.tar.xz
```

The expected archive hashes are in `vendor-manifest.json`. Extract only `package/build/**/*.js`, `package/build/**/*.d.ts`, and `package/LICENSE` from Snabbdom; `package/lib/marked.esm.js`, `package/lib/marked.d.ts`, and `package/LICENSE` from Marked (install the declaration as `marked.esm.d.ts` beside the ESM bundle); and `JetBrainsMonoNerdFontMono-Regular.ttf` plus `OFL.txt` from Nerd Fonts. Preserve the layout used here, remove source maps and package-manager metadata, regenerate the manifest/checksums, and verify that all redistributed entries are regular mode-0644 files.

## Monaco (direct, no Node acquisition or bundling)

Monaco Editor 0.56.0 is vendored as browser-native ESM, not an AMD loader or a Node-generated bundle. Its exact HTTPS archive SHA-256 and upstream SHA-512 integrity are pinned in `scripts/vendor_monaco.py` and this manifest. The Python-standard-library preparation follows the editor/basic-language/find/worker graph, moves side-effect CSS imports into `monaco/monaco.css`, extracts inline image assets, disables optional external diff-module loading, and requires an explicit module-worker hook instead of a worker fallback. `monaco/provenance.json` records changed upstream source hashes and the preparation rules. The MIT license is included.

Explicit acquisition, from the repository root:

```bash
python3 scripts/vendor_monaco.py --fetch
```

Reproduce or check using an already downloaded, checksum-verified archive:

```bash
python3 scripts/vendor_monaco.py --archive /path/to/monaco-editor-0.56.0.tgz
python3 scripts/vendor_monaco.py --archive /path/to/monaco-editor-0.56.0.tgz --check
```

The script refuses to overwrite changed existing vendored files. A version/selection update requires reviewing the old files, pinned archive, preparation and manifests first. After that review, `--refresh-verified` permits an explicit refresh only after every existing vendor hash passes verification; it does not discard unexpected files. The selected styles include Monaco's local Codicon font and modifiers. Normal `build-web.sh` runs only verify/copy these assets and never acquire Monaco. The existing TypeScript compiler and dependency-free browser test runner are unchanged.

The opt-in file pane permits Monaco's inline CSS and reviewed escaped HTML renderer. Inline JavaScript, `eval`, runtime CDN/package loading, and blob workers remain forbidden; the application passes file text through Monaco's model API rather than constructing HTML itself.


#!/usr/bin/env python3
"""Stage the existing browser distribution for Tauri; no npm frontend toolchain."""
from pathlib import Path
import shutil
root = Path(__file__).resolve().parent.parent
source = root / 'target/browser'
if not (source / 'browser/wasm/lethetic_wasm_bg.wasm').exists():
    raise SystemExit('Run ./build-browser.sh first')
target = root / 'native/dist'
shutil.rmtree(target, ignore_errors=True)
shutil.copytree(source, target)
page = target / 'index.html'
html = page.read_text()
html = html.replace('connect-src https:', "connect-src 'self' ipc: http://ipc.localhost https:")
page.write_text(html)
print(f'Staged native UI: {target}')

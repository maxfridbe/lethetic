#!/usr/bin/env python3
"""Generate native icons from the current themeable emblem using Tauri's icon tool."""
import argparse
import copy
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tauri", default="cargo-tauri", help="Path to the Tauri CLI binary")
    args = parser.parse_args()
    mark = ET.parse(ROOT / "res/lethetic-mark.svg").getroot()
    icons = ROOT / "native/src-tauri/icons"
    with tempfile.TemporaryDirectory(prefix="lethetic-icons-") as tmp:
        directory = Path(tmp)
        app = ET.Element(f"{{{SVG}}}svg", {"viewBox": "0 0 160 160", "width": "160", "height": "160"})
        ET.SubElement(app, f"{{{SVG}}}rect", {"width": "160", "height": "160", "rx": "24", "fill": "#0f172a"})
        app.append(copy.deepcopy(mark))
        ET.ElementTree(app).write(directory / "app.svg", encoding="unicode")
        foreground = ET.Element(f"{{{SVG}}}svg", {"viewBox": "0 0 108 108", "width": "108", "height": "108"})
        # The shield fits within the central 66dp safe circle under every launcher mask.
        group = ET.SubElement(foreground, f"{{{SVG}}}g", {"transform": "translate(22 22) scale(.4)"})
        group.append(copy.deepcopy(mark))
        ET.ElementTree(foreground).write(directory / "foreground.svg", encoding="unicode")
        manifest = directory / "icons.json"
        manifest.write_text(json.dumps({"default": "app.svg", "bg_color": "#0f172a", "android_fg": "foreground.svg", "android_monochrome": "foreground.svg"}))
        generated = directory / "generated"
        subprocess.run([args.tauri, "icon", str(manifest), "--output", str(generated)], check=True)
        # iOS is not a build target. Copy only assets used by supported platforms.
        for asset in generated.iterdir():
            if asset.name == "ios":
                continue
            if asset.is_dir():
                shutil.copytree(asset, icons / asset.name, dirs_exist_ok=True)
            else:
                shutil.copy2(asset, icons / asset.name)
    # Keep round-icon references adaptive too, rather than falling back to a bitmap.
    adaptive = icons / "android/mipmap-anydpi-v26"
    (adaptive / "ic_launcher_round.xml").write_bytes((adaptive / "ic_launcher.xml").read_bytes())
    favicon = copy.deepcopy(mark)
    favicon.set("color", "#0f172a")
    style = ET.Element(f"{{{SVG}}}style")
    style.text = "@media (prefers-color-scheme: dark) { :root { color: #f8fafc; } }"
    favicon.insert(0, style)
    ET.ElementTree(favicon).write(ROOT / "browser/static/assets/lethetic-icon.svg", encoding="unicode")


if __name__ == "__main__":
    main()

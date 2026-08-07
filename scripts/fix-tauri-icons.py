#!/usr/bin/env python3
"""Convert src-tauri/icons/*.png to RGBA (required by Tauri)."""
from pathlib import Path

from PIL import Image

icons = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"
source = icons / "app-icon.png"
img = Image.open(source).convert("RGBA")
img.save(source)
for path in icons.glob("*.png"):
    im = Image.open(path).convert("RGBA")
    im.save(path)
    print(f"{path.name}: {im.mode} {im.size}")

#!/usr/bin/env python3
"""Render the source SVG into deterministic desktop icon assets.

Requires CairoSVG and Pillow: python3 -m pip install cairosvg pillow
"""
from io import BytesIO
from pathlib import Path
import cairosvg
from PIL import Image

root = Path(__file__).resolve().parent.parent / 'assets' / 'icons'
source = root / 'wayx.svg'
sizes = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
for size in sizes:
    rendered = cairosvg.svg2png(url=str(source), output_width=size, output_height=size)
    image = Image.open(BytesIO(rendered)).convert('RGBA')
    image.save(root / f'wayx-{size}.png')
    if size == 128:
        (root / 'wayx-128.rgba').write_bytes(image.tobytes())
# Build each ICO frame from its own vector render rather than downscaling a bitmap.
frames = [Image.open(root / f'wayx-{size}.png') for size in sizes if size <= 256]
frames[-1].save(root / 'wayx.ico', format='ICO', sizes=[im.size for im in frames], append_images=frames[:-1])
with Image.open(root / 'wayx-1024.png') as image:
    image.save(root / 'wayx.icns', format='ICNS')
print(f'Generated PNG, ICO, ICNS and window RGBA assets in {root}')

#!/usr/bin/env python3
"""Builds the app icon files from assets/icon.svg.

    python3 assets/make_icon.py

Needs `rsvg-convert` (librsvg) and Pillow. Writes, next to this script:

- icon.png  256x256, the window icon (embedded with include_bytes!);
- icon.ico  every Windows size (16 to 256), for other uses;
- icon.res  the same icons as a compiled Windows resource, linked into the
            .exe files by build.rs (Explorer, shortcuts, Start menu, task bar).

Each size is rendered from the SVG (sharper than scaling one picture down).
The .res is written here instead of calling rc.exe at build time, so the build
needs no resource compiler and no extra crate.
"""

import io
import struct
import subprocess
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SVG = HERE / "icon.svg"
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]

RT_ICON = 3
RT_GROUP_ICON = 14


def render(size: int) -> Image.Image:
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), str(SVG)],
        check=True,
        capture_output=True,
    ).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def dib(img: Image.Image) -> bytes:
    """32-bit BMP as stored in icons: header with double height, BGRA rows
    bottom-up, then the 1-bit AND mask (all zero: the alpha channel is used)."""
    w, h = img.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    px = img.tobytes("raw", "BGRA")
    rows = [px[y * w * 4 : (y + 1) * w * 4] for y in range(h)]
    mask_row = b"\x00" * (((w + 31) // 32) * 4)
    return header + b"".join(reversed(rows)) + mask_row * h


def image_bytes(img: Image.Image) -> bytes:
    if img.size[0] >= 256:
        out = io.BytesIO()
        img.save(out, "PNG", optimize=True)
        return out.getvalue()
    return dib(img)


def entry(size: int, data_len: int) -> bytes:
    """Directory entry fields shared by .ico and the icon group (without the last field)."""
    side = 0 if size >= 256 else size
    return struct.pack("<BBBBHHI", side, side, 0, 0, 1, 32, data_len)


def write_ico(images: list[bytes]) -> bytes:
    head = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    dirs, data = b"", b""
    for size, blob in zip(SIZES, images):
        dirs += entry(size, len(blob)) + struct.pack("<I", offset + len(data))
        data += blob
    return head + dirs + data


def res_entry(kind: int, name: int, data: bytes, flags: int) -> bytes:
    header = struct.pack(
        "<IIHHHHIHHII", len(data), 32, 0xFFFF, kind, 0xFFFF, name, 0, flags, 0x0409, 0, 0
    )
    pad = b"\x00" * ((4 - len(data) % 4) % 4)
    return header + data + pad


def write_res(images: list[bytes]) -> bytes:
    out = res_entry(0, 0, b"", 0)  # every .res file starts with an empty entry
    group = struct.pack("<HHH", 0, 1, len(images))
    for i, (size, blob) in enumerate(zip(SIZES, images), start=1):
        out += res_entry(RT_ICON, i, blob, 0x1010)
        group += entry(size, len(blob)) + struct.pack("<H", i)
    # Group 1: the first icon group is the one Windows shows for the .exe.
    out += res_entry(RT_GROUP_ICON, 1, group, 0x1030)
    return out


def main() -> None:
    pictures = [render(s) for s in SIZES]
    images = [image_bytes(p) for p in pictures]
    pictures[-1].save(HERE / "icon.png", "PNG", optimize=True)
    (HERE / "icon.ico").write_bytes(write_ico(images))
    (HERE / "icon.res").write_bytes(write_res(images))
    print("icon.png, icon.ico, icon.res written")


if __name__ == "__main__":
    main()

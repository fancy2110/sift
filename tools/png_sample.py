#!/usr/bin/env python3
"""Read pixels out of a rendered PNG so colour alignment can be checked as
numbers instead of impressions.

Decodes a non-interlaced 8-bit RGB/RGBA PNG with the standard library only (the
project has no Python image dependency, and adding one for a check script would
be worse than the forty lines below).

    python3 tools/png_sample.py shot.png 20 400 700 100        # points
    python3 tools/png_sample.py shot.png --avg 700 300 40 40   # a region
"""

import struct
import sys
import zlib


def decode(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos = 8
    header = None
    raw = bytearray()
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, colour, compression, filter_, interlace = struct.unpack(
                ">IIBBBBB", body
            )
            if depth != 8 or interlace != 0 or colour not in (2, 6):
                raise SystemExit(
                    f"unsupported PNG: depth={depth} colour={colour} interlace={interlace}"
                )
            header = (width, height, 3 if colour == 2 else 4)
        elif kind == b"IDAT":
            raw += body
        elif kind == b"IEND":
            break
    if header is None:
        raise SystemExit("no IHDR")
    width, height, channels = header
    pixels = bytearray(width * height * channels)
    stride = width * channels
    out = zlib.decompress(bytes(raw))
    previous = bytearray(stride)
    offset = 0
    for y in range(height):
        filter_type = out[offset]
        offset += 1
        line = bytearray(out[offset : offset + stride])
        offset += stride
        for x in range(stride):
            left = line[x - channels] if x >= channels else 0
            up = previous[x]
            up_left = previous[x - channels] if x >= channels else 0
            if filter_type == 1:
                line[x] = (line[x] + left) & 0xFF
            elif filter_type == 2:
                line[x] = (line[x] + up) & 0xFF
            elif filter_type == 3:
                line[x] = (line[x] + ((left + up) >> 1)) & 0xFF
            elif filter_type == 4:
                p = left + up - up_left
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - up_left)
                predictor = left if (pa <= pb and pa <= pc) else (up if pb <= pc else up_left)
                line[x] = (line[x] + predictor) & 0xFF
        pixels[y * stride : (y + 1) * stride] = line
        previous = line
    return width, height, channels, pixels


def hex_of(pixels, channels, width, x, y):
    index = (y * width + x) * channels
    r, g, b = pixels[index], pixels[index + 1], pixels[index + 2]
    return r, g, b


def main(argv):
    path = argv[0]
    width, height, channels, pixels = decode(path)
    print(f"{path}: {width}x{height}, {channels} channels")

    if argv[1] == "--avg":
        x, y, w, h = (int(value) for value in argv[2:6])
        total = [0, 0, 0]
        count = 0
        for row in range(y, min(y + h, height)):
            for column in range(x, min(x + w, width)):
                r, g, b = hex_of(pixels, channels, width, column, row)
                total[0] += r
                total[1] += g
                total[2] += b
                count += 1
        mean = [value // count for value in total]
        print(f"  average over ({x},{y}) {w}x{h}: rgb{tuple(mean)}  #{mean[0]:02X}{mean[1]:02X}{mean[2]:02X}")
        return

    coords = [int(value) for value in argv[1:]]
    for index in range(0, len(coords), 2):
        x, y = coords[index], coords[index + 1]
        if not (0 <= x < width and 0 <= y < height):
            print(f"  ({x},{y}): out of bounds")
            continue
        r, g, b = hex_of(pixels, channels, width, x, y)
        print(f"  ({x:4d},{y:4d}): rgb({r:3d},{g:3d},{b:3d})  #{r:02X}{g:02X}{b:02X}")


if __name__ == "__main__":
    main(sys.argv[1:])

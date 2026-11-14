#!/usr/bin/env python3
"""Generate the shared rounded-star IME icons using Python's standard library."""

import argparse
import math
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
ICO_SIZES = (16, 20, 24, 32, 40, 48)
INNER_RATIO = 0.5
TIP_RADIUS = 0.10
INNER_RADIUS = 0.035
MENU_RECT = [
    ("M", 5.2, 0), ("L", 22.8, 0), ("C", 25.8, 0, 28, 2.2, 28, 5.2),
    ("L", 28, 14.8), ("C", 28, 17.8, 25.8, 20, 22.8, 20),
    ("L", 5.2, 20), ("C", 2.2, 20, 0, 17.8, 0, 14.8),
    ("L", 0, 5.2), ("C", 0, 2.2, 2.2, 0, 5.2, 0), ("Z",),
]

# One-off extraction from packaging/icons/icon.png (512x512), in repository root:
# magick packaging/icons/icon.png -crop 272x210+116+140 +repage -alpha extract \
#   -threshold 60% -define connected-components:keep=5,11 \
#   -define connected-components:mean-color=true -connected-components 8 \
#   -threshold 50% -morphology Close Disk:1 -negate /tmp/tao-mask.pbm
# nix shell nixpkgs#potrace --command potrace /tmp/tao-mask.pbm --svg \
#   --turdsize 12 --alphamax 1.0 --opttolerance 0.5 --output /tmp/tao-trace.svg
# Components 5 and 11 retain only Tao and its swoosh, before closing nearby strokes.
# Expand relative trace commands, apply translate(0,210) scale(0.1,-0.1), then
# normalize the exact Bezier bounds (9,4.156178)-(266.310002,204.625704) to [0,1].
# Preserve the original aspect ratio and upward slant when placing the unit path.
TAO_ASPECT_RATIO = 0.779097
TAO_PATH = [
    ("M", 0.800591, 0.007202),
    ("C", 0.777273, 0.015183, 0.776495, 0.015682, 0.736854, 0.038628),
    ("C", 0.703820, 0.058083, 0.638529, 0.106469, 0.625704, 0.121434),
    ("C", 0.620263, 0.127420, 0.606273, 0.140888, 0.594613, 0.151364),
    ("C", 0.582954, 0.161340, 0.571684, 0.173312, 0.569741, 0.177802),
    ("C", 0.566243, 0.185284, 0.567020, 0.185284, 0.575959, 0.177802),
    ("C", 0.624927, 0.136898, 0.715479, 0.082525, 0.745404, 0.076041),
    ("C", 0.751234, 0.075043, 0.760173, 0.072549, 0.765613, 0.070553),
    ("C", 0.804088, 0.057584, 0.866659, 0.059579, 0.880261, 0.074045),
    ("C", 0.883370, 0.077537, 0.886091, 0.078535, 0.886091, 0.076539),
    ("C", 0.886091, 0.074544, 0.893475, 0.080530, 0.902025, 0.089509),
    ("C", 0.952936, 0.141387, 0.942832, 0.271582, 0.878318, 0.392797),
    ("C", 0.862384, 0.422228, 0.808752, 0.492563, 0.793595, 0.502539),
    ("C", 0.784268, 0.509024, 0.777661, 0.499546, 0.783102, 0.488073),
    ("C", 0.795538, 0.462633, 0.800979, 0.355884, 0.791652, 0.327450),
    ("C", 0.780770, 0.294528, 0.767945, 0.289539, 0.737243, 0.304504),
    ("C", 0.714702, 0.315478, 0.708873, 0.358877, 0.726750, 0.384816),
    ("C", 0.748125, 0.416242, 0.751234, 0.447668, 0.739963, 0.517005),
    ("C", 0.716257, 0.666654, 0.620652, 0.676132, 0.631534, 0.527980),
    ("C", 0.638141, 0.439188, 0.689052, 0.309991, 0.722863, 0.296024),
    ("C", 0.731413, 0.292532, 0.733357, 0.283553, 0.725584, 0.283553),
    ("C", 0.707707, 0.283553, 0.663013, 0.329446, 0.634254, 0.377333),
    ("C", 0.592670, 0.446172, 0.572461, 0.506530, 0.567409, 0.575867),
    ("C", 0.565077, 0.610286, 0.562357, 0.621261, 0.549920, 0.647699),
    ("C", 0.508336, 0.735991, 0.477245, 0.729506, 0.493568, 0.635727),
    ("C", 0.501729, 0.589834, 0.513000, 0.521495, 0.513000, 0.519001),
    ("C", 0.513000, 0.514012, 0.468695, 0.522991, 0.464032, 0.528977),
    ("C", 0.461311, 0.532469, 0.457036, 0.550427, 0.454704, 0.569382),
    ("C", 0.444211, 0.656179, 0.378532, 0.797347, 0.348995, 0.797347),
    ("C", 0.329175, 0.797347, 0.323345, 0.750457, 0.336559, 0.695087),
    ("C", 0.361432, 0.590832, 0.460534, 0.458144, 0.513389, 0.458144),
    ("C", 0.526991, 0.458144, 0.533986, 0.447668, 0.527379, 0.437692),
    ("C", 0.520384, 0.426717, 0.502118, 0.417738, 0.499398, 0.423226),
    ("C", 0.497843, 0.426219, 0.493568, 0.428214, 0.488904, 0.428214),
    ("C", 0.462089, 0.428214, 0.374645, 0.522991, 0.325677, 0.605298),
    ("C", 0.249116, 0.732998, 0.232793, 0.887136, 0.295364, 0.887136),
    ("C", 0.326454, 0.887136, 0.352882, 0.859202, 0.392911, 0.783380),
    ("C", 0.422059, 0.729008, 0.425168, 0.726514, 0.428666, 0.755944),
    ("C", 0.436050, 0.813310, 0.465198, 0.813310, 0.507948, 0.755944),
    ("C", 0.523104, 0.735492, 0.539816, 0.709055, 0.544868, 0.697581),
    ("C", 0.549920, 0.685610, 0.556139, 0.671144, 0.558859, 0.664160),
    ("L", 0.564300, 0.652188),
    ("L", 0.576348, 0.668151),
    ("C", 0.595002, 0.691596, 0.602775, 0.696584, 0.621818, 0.698080),
    ("C", 0.668454, 0.701572, 0.706541, 0.666155, 0.746182, 0.581853),
    ("C", 0.766002, 0.539952, 0.769111, 0.535462, 0.786988, 0.526982),
    ("C", 0.850725, 0.495057, 0.950604, 0.362368, 0.973922, 0.277567),
    ("C", 0.977032, 0.266094, 0.980918, 0.254621, 0.982861, 0.251628),
    ("C", 0.984416, 0.248635, 0.987525, 0.238160, 0.989079, 0.228682),
    ("C", 0.990634, 0.219204, 0.993354, 0.202244, 0.995297, 0.191270),
    ("C", 1.010066, 0.108464, 0.989857, 0.046610, 0.937391, 0.013687),
    ("C", 0.914073, -0.001278, 0.835179, -0.004770, 0.800591, 0.007202),
    ("Z",),
    ("M", 0.607050, 0.187778),
    ("C", 0.604718, 0.194762, 0.602386, 0.203242, 0.602386, 0.206235),
    ("C", 0.602386, 0.227186, 0.553418, 0.262104, 0.499786, 0.279064),
    ("C", 0.447320, 0.295525, 0.436827, 0.298019, 0.396409, 0.301012),
    ("C", 0.304691, 0.307497, 0.191598, 0.327949, 0.159341, 0.343413),
    ("C", 0.158175, 0.344411, 0.153900, 0.346406, 0.149625, 0.348401),
    ("C", 0.071509, 0.386811, 0.000777, 0.485080, 0.000000, 0.556413),
    ("C", 0.000000, 0.570879, 0.007773, 0.561401, 0.010105, 0.543942),
    ("C", 0.015545, 0.504535, 0.038475, 0.477598, 0.097159, 0.441682),
    ("C", 0.127473, 0.422727, 0.130582, 0.421729, 0.180327, 0.409757),
    ("C", 0.216859, 0.401277, 0.222689, 0.402774, 0.219191, 0.421729),
    ("C", 0.216470, 0.439687, 0.204423, 0.494558, 0.192375, 0.545439),
    ("C", 0.187323, 0.567387, 0.178384, 0.605298, 0.172943, 0.630240),
    ("C", 0.156620, 0.705064, 0.151180, 0.728010, 0.137966, 0.779888),
    ("C", 0.130582, 0.807324, 0.123975, 0.838750, 0.122420, 0.849724),
    ("C", 0.120866, 0.860699, 0.118923, 0.873169, 0.117757, 0.877160),
    ("C", 0.107652, 0.915570, 0.122420, 0.974432, 0.146127, 0.989895),
    ("C", 0.173720, 1.007354, 0.229295, 1.001867, 0.284870, 0.975928),
    ("C", 0.314018, 0.962460, 0.312852, 0.959467, 0.278264, 0.960464),
    ("C", 0.240954, 0.961462, 0.236679, 0.959965, 0.216082, 0.935024),
    ("C", 0.184602, 0.897612, 0.184602, 0.844237, 0.214916, 0.722523),
    ("C", 0.222300, 0.693591, 0.230850, 0.657675, 0.234348, 0.642710),
    ("C", 0.237845, 0.627745, 0.246784, 0.589336, 0.254168, 0.557411),
    ("C", 0.261552, 0.525485, 0.271268, 0.476101, 0.275932, 0.447668),
    ("C", 0.280595, 0.419235, 0.285259, 0.394293, 0.286036, 0.392797),
    ("C", 0.286814, 0.390802, 0.309743, 0.387809, 0.336948, 0.385813),
    ("C", 0.478800, 0.376834, 0.528545, 0.358378, 0.578291, 0.297022),
    ("C", 0.601609, 0.268090, 0.612879, 0.239158, 0.615988, 0.197755),
    ("C", 0.617932, 0.174809, 0.613657, 0.170319, 0.607050, 0.187778),
    ("Z",),
]


def rounded_star():
    vertices = []
    for i in range(10):
        angle = -math.pi / 2 + i * math.pi / 5
        radius = 1 if i % 2 == 0 else INNER_RATIO
        vertices.append((radius * math.cos(angle), radius * math.sin(angle)))
    path = []
    for i, vertex in enumerate(vertices):
        previous, following = vertices[i - 1], vertices[(i + 1) % 10]
        incoming = tuple(vertex[j] - previous[j] for j in (0, 1))
        outgoing = tuple(following[j] - vertex[j] for j in (0, 1))
        incoming = tuple(v / math.hypot(*incoming) for v in incoming)
        outgoing = tuple(v / math.hypot(*outgoing) for v in outgoing)
        turn = math.acos(max(-1, min(1, sum(a * b for a, b in zip(incoming, outgoing)))))
        radius = TIP_RADIUS if i % 2 == 0 else INNER_RADIUS
        distance = radius * math.tan(turn / 2)
        handle = 4 * radius * math.tan(turn / 4) / 3
        entry = tuple(vertex[j] - distance * incoming[j] for j in (0, 1))
        exit_point = tuple(vertex[j] + distance * outgoing[j] for j in (0, 1))
        control1 = tuple(entry[j] + handle * incoming[j] for j in (0, 1))
        control2 = tuple(exit_point[j] - handle * outgoing[j] for j in (0, 1))
        path.append(("M" if i == 0 else "L", *entry))
        # Short segments avoid MSVG flattening small-radius curves into visible facets.
        points = (entry, control1, control2, exit_point)
        for segment in range(4):
            start, end = segment / 4, (segment + 1) / 4
            first, tangent1 = cubic_point(points, start)
            last, tangent2 = cubic_point(points, end)
            path.append(("C",
                         *(first[j] + tangent1[j] / 12 for j in (0, 1)),
                         *(last[j] - tangent2[j] / 12 for j in (0, 1)), *last))
    return path + [("Z",)]


def cubic_point(points, t):
    p0, p1, p2, p3 = points
    point = tuple((1 - t) ** 3 * p0[j] + 3 * (1 - t) ** 2 * t * p1[j]
                  + 3 * (1 - t) * t * t * p2[j] + t ** 3 * p3[j] for j in (0, 1))
    tangent = tuple(3 * (1 - t) ** 2 * (p1[j] - p0[j])
                    + 6 * (1 - t) * t * (p2[j] - p1[j])
                    + 3 * t * t * (p3[j] - p2[j]) for j in (0, 1))
    return point, tangent


def path_bounds(path):
    coordinates = [[], []]
    current = None
    for command, *values in path:
        if command in ("M", "L"):
            current = tuple(values)
            for axis in (0, 1):
                coordinates[axis].append(current[axis])
        elif command == "C":
            for axis in (0, 1):
                p0, p1, p2, p3 = current[axis], *values[axis::2]
                a, b, c = -p0 + 3 * p1 - 3 * p2 + p3, 2 * (p0 - 2 * p1 + p2), p1 - p0
                roots = []
                if abs(a) < 1e-12:
                    if abs(b) > 1e-12:
                        roots = [-c / b]
                elif b * b - 4 * a * c >= 0:
                    discriminant = math.sqrt(b * b - 4 * a * c)
                    roots = [(-b + discriminant) / (2 * a), (-b - discriminant) / (2 * a)]
                coordinates[axis].append(p3)
                for t in roots:
                    if 0 < t < 1:
                        coordinates[axis].append(
                            (1 - t) ** 3 * p0 + 3 * (1 - t) ** 2 * t * p1
                            + 3 * (1 - t) * t * t * p2 + t ** 3 * p3
                        )
            current = tuple(values[-2:])
    return tuple(min(axis) for axis in coordinates) + tuple(max(axis) for axis in coordinates)


def fit_star(bounds):
    path = rounded_star()
    original = path_bounds(path)
    scale = tuple((bounds[i + 2] - bounds[i]) / (original[i + 2] - original[i]) for i in (0, 1))
    # Preserve the previous visible footprint, including its optical vertical placement.
    return [
        (command, *(bounds[i % 2] + (v - original[i % 2]) * scale[i % 2]
                    for i, v in enumerate(values)))
        for command, *values in path
    ]


def number(value):
    return f"{value:.6f}".rstrip("0").rstrip(".") or "0"


def place_tao(x, y, width):
    scale = (width, width * TAO_ASPECT_RATIO)
    return [(command, *((x, y)[i % 2] + v * scale[i % 2] for i, v in enumerate(values)))
            for command, *values in TAO_PATH]


def svg_path(path):
    return " ".join(command + " ".join(number(v) for v in values) for command, *values in path)


def write_svg(path, width, height, body):
    path.write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}">\n'
        '  <!-- Generated by scripts/generate-ime-brand-icons.py. -->\n'
        f'{body}\n</svg>\n', encoding="utf-8",
    )


def write_pdf(path, shape):
    operators = {"M": "m", "L": "l", "C": "c", "Z": "h"}
    # macOS caps input-source icons; keep the 21x15 pt page the previous icon used.
    stream = ("0 g\n0.75 0 0 -0.75 0 15 cm\n" + "\n".join(
        " ".join([*(number(v) for v in values), operators[command]])
        for command, *values in shape
    ) + "\nf*\n").encode("ascii")
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 21 15] /Resources << >> /Contents 4 0 R >>",
        f"<< /Length {len(stream)} >>\nstream\n".encode("ascii") + stream + b"endstream",
    ]
    document = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for index, obj in enumerate(objects, 1):
        offsets.append(len(document))
        document.extend(f"{index} 0 obj\n".encode("ascii") + obj + b"\nendobj\n")
    xref = len(document)
    document.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode("ascii"))
    for offset in offsets[1:]:
        document.extend(f"{offset:010d} 00000 n \n".encode("ascii"))
    document.extend(
        f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode("ascii")
    )
    path.write_bytes(document)


def render_svg(source, destination, width, height, source_width):
    # MSVG uses ImageMagick's internal renderer; each output gets its own 8x render.
    subprocess.run([
        "magick", "-background", "none", "-density", number(96 * 8 * width / source_width),
        f"MSVG:{source}", "-filter", "Lanczos", "-resize", f"{width}x{height}!",
        "-strip", f"PNG32:{destination}",
    ], check=True)


def generate_macos():
    directory = ROOT / "crates/keytao-macos-ime/Resources"
    # The star's visual center is below its bounding-box center.
    shape = MENU_RECT + fit_star((6.3, 2.7, 21.7, 16.85)) + place_tao(9.65, 7.2, 8.7)
    svg = directory / "keytao-menu-icon.svg"
    write_svg(svg, 28, 20, f'  <path fill="#000000" fill-rule="evenodd" d="{svg_path(shape)}"/>')
    write_pdf(directory / "keytao-menu-icon.pdf", shape)
    render_svg(svg, directory / "keytao-menu-icon.png", 56, 40, 28)


def generate_windows():
    directory = ROOT / "crates/keytao-windows-ime"
    svg = directory / "ime-brand.svg"
    star = svg_path(fit_star((11.2, 12.8, 52.8, 51.1)))
    tao = svg_path(place_tao(20.25, 24.98, 23.5))
    body = ('  <rect x="3" y="3" width="58" height="58" rx="14" fill="#202124"/>\n'
            f'  <path id="keytao-star" fill="#ffffff" d="{star}"/>')
    write_svg(svg, 64, 64, body + f'\n  <path id="keytao-tao" fill="#202124" d="{tao}"/>')
    with tempfile.TemporaryDirectory(prefix="keytao-ime-icons-") as temporary:
        plain = Path(temporary) / "plain.svg"
        write_svg(plain, 64, 64, body)
        frames = []
        for size in ICO_SIZES:
            frame = Path(temporary) / f"{size}.png"
            # Tao collapses into gray specks at 16/20px; retain the plain star there.
            render_svg(plain if size <= 20 else svg, frame, size, size, 64)
            frames.append(str(frame))
        subprocess.run(["magick", *frames, "-strip", str(directory / "ime-brand.ico")], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("all", "macos", "windows"), default="all")
    args = parser.parse_args()
    if args.platform in ("all", "macos"):
        generate_macos()
    if args.platform in ("all", "windows"):
        generate_windows()


if __name__ == "__main__":
    main()

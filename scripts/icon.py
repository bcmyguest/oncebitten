# Generates docs/media/icon.svg: the tile with a bite out of one corner, and a check mark.
# Run: uv run --with shapely python scripts/icon.py docs/media/icon.svg
import sys
from shapely.geometry import box, Point
from shapely.ops import unary_union
out = sys.argv[1]
tile = box(8, 8, 22, 22).buffer(8, quad_segs=32)
bite = unary_union([Point(30, 0).buffer(8, quad_segs=32)] + [Point(x, y).buffer(3.6, quad_segs=32) for x, y in [(30, 8), (24.3, 5.7), (22, 0)]])
shape = tile.difference(bite).simplify(0.02)
d = "M" + " L".join(f"{x:.2f} {y:.2f}" for x, y in shape.exterior.coords[:-1]) + "Z"
svg = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30">'
       f'<path d="{d}" fill="#8b93ff"/>'
       '<path d="M7.5 16 12 20.5 20 12.5" fill="none" stroke="#0f1226" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round"/></svg>\n')
open(out, "w").write(svg)

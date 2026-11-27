"""Extract an exact overpass from the animated cubic centerline that draws it."""
import math


def split(points, t):
    """de Casteljau subdivision of a single Bézier cubic."""
    p, a, b, q = points
    def mix(u, v):
        return [x + (y - x) * t for x, y in zip(u, v)]
    pa, ab, bq = mix(p, a), mix(a, b), mix(b, q)
    left, right = mix(pa, ab), mix(ab, bq)
    mid = mix(left, right)
    return [p, pa, left, mid], [mid, right, bq, q]


def cubic_window(d, start, end):
    """Crop a fixed interval in authored cubic coordinates across every keyed pose.

    E.g. [1.5, 3] starts halfway through the second cubic and ends at the end
    of the third. Bézier subdivision is linear in its control points, so the
    subsection stays exactly on its parent curve throughout interpolated poses.
    """
    keys = d["keys"]
    if not keys or not all(math.isfinite(value) for value in (start, end)):
        raise ValueError("a crossing window needs keyed poses and finite endpoints")
    count = len(keys[0]["points"])
    if (count - 1) % 3 or not 0 <= start < end <= (count - 1) // 3:
        raise ValueError("crossing window must lie on the keyed cubic strand")
    result = []
    for key in keys:
        controls = key["points"]
        if len(controls) != count:
            raise ValueError("all crossing poses must have corresponding cubics")
        section = []
        for n in range(math.floor(start), math.ceil(end)):
            curve = controls[3*n : 3*n+4]
            lo, hi = max(0., start - n), min(1., end - n)
            if hi < 1:
                curve, _ = split(curve, hi)
            if lo > 0:
                _, curve = split(curve, lo / hi)
            section.extend(curve[1:] if section else curve)
        result.append({**key, "points": section})
    return {**d, "closed": False, "keys": result}

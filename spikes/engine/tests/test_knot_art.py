"""Check the authoring geometry independently of rasterization and composition."""
import cmath
import json
import math
from pathlib import Path
import sys
import unittest

FIXTURES = Path(__file__).resolve().parents[1] / "fixtures"
sys.path.insert(0, str(FIXTURES))
from knot_motion import layout


def ink(art):
    return [group["content"]["children"][0]["content"]["d"]["keys"] for group in layout(art)[1:]]


def arc_length(points):
    """Sample the authored cubics, independently of the angular subdivision builder."""
    controls = [complex(x, y) for x, y in points]
    last = controls[0]
    length = 0.0
    for i in range(0, len(controls)-1, 3):
        a, b, c, d = controls[i:i+4]
        for j in range(1, 129):
            u = j/128
            v = 1-u
            point = a*v**3 + 3*b*u*v*v + 3*c*u*u*v + d*u**3
            length += abs(point-last)
            last = point
    return length


class KnotArtTest(unittest.TestCase):
    def setUp(self):
        self.art = json.loads((FIXTURES / "knot-art.json").read_text())

    def test_seam_revision_partitions_one_circumference(self):
        radius = self.art["ring_radius"]
        for shift in (0, .125):
            with self.subTest(shift=shift):
                strands = ink({**self.art, "seam_shift_turns":shift})
                arcs = [s[0]["points"] for s in strands]
                seams = [0, 1/3+shift, 2/3, 1]
                for i, points in enumerate(arcs):
                    self.assertEqual(points[-1], arcs[(i+1)%3][0])
                    angle = -math.tau*seams[i]
                    radial = cmath.exp(1j*angle)*radius
                    self.assertAlmostEqual(points[0][0], 960+radial.imag, places=3)
                    self.assertAlmostEqual(points[0][1], 540-radial.real, places=3)
                    self.assertAlmostEqual(arc_length(points),
                        math.tau * radius * (seams[i+1]-seams[i]), delta=.025)
                self.assertEqual([s[-1] for s in strands], [s[-1] for s in ink(self.art)])

    def test_tip_and_crossing_edits_preserve_other_strands(self):
        baseline = ink(self.art)
        pulled = ink({**self.art, "tip_pull_px":60})
        delayed = ink({**self.art, "weave_delay_frames":6})
        for variant in (pulled, delayed):
            self.assertEqual(variant[1:], baseline[1:])
            self.assertEqual(variant[0][0], baseline[0][0])
            self.assertEqual(variant[0][-1], baseline[0][-1])
        self.assertEqual([p["frame"] for p in pulled[0]], [p["frame"] for p in baseline[0]])
        self.assertNotEqual(pulled[0][2]["points"][-1], baseline[0][2]["points"][-1])
        self.assertEqual([p["points"] for p in delayed[0]], [p["points"] for p in baseline[0]])
        self.assertEqual([v["frame"]-b["frame"] for v,b in zip(delayed[0],baseline[0])],
                         [0,0,0,6,6,0])


if __name__ == "__main__":
    unittest.main()

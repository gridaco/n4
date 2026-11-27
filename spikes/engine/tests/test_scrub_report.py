"""A frozen picture must remain a failure even when the UI repaints every input."""
import runpy
import unittest
from pathlib import Path


summarize = runpy.run_path(
    str(Path(__file__).resolve().parents[1] / "fixtures" / "scrub_report.py")
)["summarize"]


class ScrubReportTest(unittest.TestCase):
    def test_unsettled_release_keeps_gesture_measurements(self):
        trace = {"inputs_us_frame": [[0, 0], [1000, 1], [2000, 0], [3000, 3], [4000, 1]],
                 "presents_us_cursor_picture": [[0, 0, 0], [4100, 1, 0]], "ticks_per_leg": 1}
        with self.assertRaises(ValueError):
            summarize({"scrub_trace": trace})
        failed = summarize({"scrub_trace": trace, "scrub_failure": "release timed out"})
        self.assertIsNone(failed["release_to_matching_present_ms"])
        self.assertEqual([leg["updated_pictures_per_second"] for leg in failed["legs"]], [0] * 4)

    def test_frozen_movie_and_exact_release(self):
        inputs = [[i * 100_000, target] for i, target in enumerate([0, 1, 2, 1, 0, 3, 6, 4, 2])]
        presents = [[t, cursor, 0] for t, cursor in inputs]
        presents.append([850_000, 2, 2])
        result = summarize({"scrub_trace": {
            "inputs_us_frame": inputs,
            "presents_us_cursor_picture": presents,
            "ticks_per_leg": 2,
        }})
        self.assertEqual(result["release_to_matching_present_ms"], 50)
        self.assertEqual(result["legs"][2]["longest_continuous_target_mismatch_ms"], 100)
        self.assertEqual(result["legs"][3]["longest_continuous_target_mismatch_ms"], 200)
        for leg in result["legs"]:
            self.assertEqual(leg["updated_pictures_per_second"], 0)
            self.assertEqual(leg["picture_hold_ms_p50_p95_max"], [200, 200, 200])
            self.assertEqual(leg["pointer_to_playhead_frames_p95"], 0)
            self.assertIsNone(leg["first_picture_moving_in_leg_direction_ms"])
        self.assertEqual(result["legs"][3]["painted_playhead_to_picture_frames_p95"], 6)

    def test_target_must_be_visible_before_the_following_input(self):
        inputs = [[i * 10_000, target] for i, target in enumerate([0, 1, 2, 2, 0, 3, 6, 4, 2])]
        # The movie reaches 3 at the exact instant the pointer requests 6.
        # During the stationary tick at frame 2 there is no new target to score.
        pictures = [[0, 0, 0], [19_999, 1, 1], [22_000, 2, 2], [40_001, 0, 0],
                    [60_000, 6, 3], [60_500, 6, 6], [72_000, 4, 4], [82_000, 2, 2]]
        result = summarize({"scrub_trace": {"inputs_us_frame": inputs,
                           "presents_us_cursor_picture": pictures, "ticks_per_leg": 2}})
        scored = [(leg["correct_before_next_pointer_tick"], leg["changed_targets"]) for leg in result["legs"]]
        self.assertEqual(scored, [(1, 1), (1, 1), (1, 2), (2, 2)])


if __name__ == "__main__":
    unittest.main()

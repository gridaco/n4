import json
import os
from pathlib import Path
import shutil
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch

from n4 import ConcurrentEditError, NativeError, Project, label


ENGINE = Path(__file__).resolve().parents[3] / "engine"
BINARY = os.getenv("N4_BIN", str(ENGINE / "target/debug/n4"))


class NativeAuthoringTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / "sequences").mkdir()
        shutil.copy(ENGINE / "fixtures/fonts/N4GridSans.ttf", self.root / "font.ttf")
        (self.root / "project.proj").write_text(
            "project test fps 30/1 size 640x360\n[0,60) V1 first\n[60,120) V1 second\n"
        )
        self.file = self.root / "sequences/first.json"
        group = {"name": "Scroll", "content": {"type": "transform_group", "children": []}}
        self.file.write_text(json.dumps({"background": {"color": [255, 255, 255]}, "graphics": [group]}))
        self.other = self.root / "sequences/second.json"
        self.other.write_text('{"background":{"color":[255,255,255]}}')
        self.project = Project.open(self.root, n4=BINARY)

    def test_build_query_patch_and_publish_one_detail(self):
        original = self.file.read_bytes()
        master = self.project.entry.read_bytes()
        second = self.other.read_bytes()
        self.assertEqual(set(self.project.details), {"first", "second"})
        s = self.project.sequence("first")
        self.assertFalse(s.write())  # The original file has deliberately compact whitespace.
        name = label("Name", "Village", font="font.ttf", size=24, width=135)
        s.add(name, "Scroll")
        self.assertEqual(len(s.metrics("Scroll", "Name")["lines"]), 1)
        s.layer("Scroll", "Name")["content"]["text"] = "North Station Visitor Parking"
        self.assertGreater(len(s.metrics("Scroll", "Name")["lines"]), 1)
        s.validate()
        self.assertEqual(self.file.read_bytes(), original)
        self.assertTrue(s.write())
        self.assertFalse(s.write())
        self.assertEqual(Project.open(self.root, n4=BINARY).sequence("first").layer("Scroll", "Name"), name)
        self.assertEqual(self.project.entry.read_bytes(), master)
        self.assertEqual(self.other.read_bytes(), second)

    def test_rejected_candidate_keeps_the_previous_revision(self):
        original = self.file.read_bytes()
        s = self.project.sequence("first")
        s.add(label("Name", "Village", font="missing.ttf", size=24, width=135))
        with self.assertRaisesRegex(NativeError, "missing.ttf"):
            s.write()
        self.assertEqual(self.file.read_bytes(), original)
        s.layer("Name")["content"]["font"] = "font.ttf"
        self.assertTrue(s.write())
        self.assertEqual(set(self.file.parent.iterdir()), {self.file, self.other})

    def test_orphans_and_ambiguous_layer_names(self):
        (self.root / "sequences/orphan.json").write_text(self.other.read_text())
        with self.assertRaisesRegex(KeyError, "no placed detail"):
            self.project.sequence("orphan")
        s = self.project.sequence("first")
        for _ in range(2):
            s.add(label("Name", "Village", font="font.ttf", size=24, width=135), "Scroll")
        with self.assertRaisesRegex(KeyError, "Scroll / Name.+matched 2"):
            s.layer("Scroll", "Name")

    def test_concurrent_sdk_saves_preserve_the_winning_revision(self):
        ready = threading.Barrier(2)

        def save(value):
            s = self.project.sequence("first")
            s.document["name"] = value
            ready.wait(timeout=5)
            try:
                s.write()
                return value
            except ConcurrentEditError:
                return None

        with ThreadPoolExecutor(max_workers=2) as pool:
            wins = list(pool.map(save, ("North", "South")))
        self.assertEqual(wins.count(None), 1)
        self.assertIn(json.loads(self.file.read_bytes())["name"], wins)

    def test_detect_an_external_save_during_validation(self):
        s = self.project.sequence("first")
        s.document["name"] = "My change"
        external = self.file.read_bytes() + b"\n"
        native = self.project._run

        def edit_after_validation(*args, **kwargs):
            result = native(*args, **kwargs)
            self.file.write_bytes(external)
            return result

        with patch.object(self.project, "_run", side_effect=edit_after_validation):
            with self.assertRaisesRegex(ConcurrentEditError, "first.json changed on disk"):
                s.write()
        self.assertEqual(self.file.read_bytes(), external)
        self.assertEqual(set(self.file.parent.iterdir()), {self.file, self.other})


if __name__ == "__main__":
    unittest.main()

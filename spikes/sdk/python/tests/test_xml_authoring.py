import os
from pathlib import Path
import shutil
import tempfile
import unittest

from n4 import NativeError, Project, encode_sequence, label


ENGINE = Path(__file__).resolve().parents[3] / "engine"
BINARY = os.getenv("N4_BIN", str(ENGINE / "target/debug/n4"))


class XmlAuthoringTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        (self.root / "sequences").mkdir()
        self.path = self.root / "sequences/title.xml"
        self.master = self.root / "project.proj"
        self.master.write_text("project test fps 30/1 size 128x128\n[0,12) V1 title\n")
        shutil.copy(ENGINE / "fixtures/fonts/N4GridSans.ttf", self.root / "font.ttf")
        self.copy = "A<&> \nB\rC "
        title = label("Title", self.copy, font="font.ttf", width=100, size=16)
        box = {"name": "Box", "content": {"type": "rect", "width": 32, "height": 32,
               "corner_radius": [{"frame": 0, "value": 8}, {"frame": 10, "value": [8, 8, 8, 0]}]}}
        self.initial = encode_sequence({"background": {"color": [40, 50, 60]}, "graphics": [box, title]})
        self.path.write_bytes(self.initial)

    def sequence(self):
        return Project.open(self.root, n4=BINARY).sequence("title")

    def test_independent_video_trim_round_trips_inside_a_moving_group(self):
        project = Project.open(self.root, n4=BINARY)
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        original = b"""<sequence background="#000000"><transform-group name="Wall">
          <video name="Monitor" source="plates/test.mp4" start="2" source-in="7"
            duration="8" size="64 36" position="8 5"/>
        </transform-group></sequence>"""
        doc = decode(original)
        clip = doc["graphics"][0]["content"]["children"][0]["content"]
        self.assertEqual(clip, {"type": "video", "source": "plates/test.mp4", "start": 2,
                               "source_in": 7, "duration": 8, "size": [64, 36]})
        clip["source_in"] = 4
        self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_footage_history_survives_sdk_round_trip(self):
        project = Project.open(self.root, n4=BINARY)
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        original = b'<sequence source="film.mkv"><footage><history frames="8" source="effects/tube.wgsl"/></footage></sequence>'
        doc = decode(original)
        self.assertEqual(doc["graphics"][0]["content"]["type"], "footage")
        history = doc["graphics"][0]["content"]["history"]
        self.assertEqual(history, {"frames": 8, "shader": "effects/tube.wgsl"})
        history["frames"] = 4
        self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_backdrop_sampling_round_trips_with_its_mask(self):
        project = Project.open(self.root, n4=BINARY)
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        for parameter, field in [("sample-radius", "sample_radius_px"), ("sigma", "sigma_px")]:
            original = (f'<sequence background="#123456"><backdrop source="effects/optics.wgsl" {parameter}="12">'
                        '<mask><rect width="40" height="24" corner-radius="10"/></mask></backdrop></sequence>')
            doc = decode(original.encode())
            self.assertEqual(doc["graphics"][0]["content"]["filter"], {"shader": "effects/optics.wgsl", field: 12})
            self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_backdrop_parameter_order_and_tracks_survive_an_edit(self):
        project = Project.open(self.root, n4=BINARY)
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        original = b"""<sequence background="#102030"><backdrop source="lens.wgsl" sample-radius="12">
          <param name="gap"><key frame="0" value="24" ease="cubic(0.1 0.4 0.5 1)"/>
            <key frame="9" value="-12"/></param>
          <param name="tint" value="0.25"/>
          <mask><rect width="40" height="24" corner-radius="10"/></mask></backdrop></sequence>"""
        doc = decode(original)
        slots = doc["graphics"][0]["content"]["filter"]["params"]
        self.assertEqual([slot["name"] for slot in slots], ["gap", "tint"])
        self.assertEqual(slots[1]["value"], 0.25)
        self.assertEqual(slots[0]["value"][1], {"frame": 9, "value": -12})
        slots[0]["value"][1]["value"] = -16
        self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_editing_a_camera_preserves_placement_cuts_and_independent_axes(self):
        master = b"project test fps 30/1 size 128x128\n[0,12) V1 title\n  [0,5) CAM wide\n  [5,12) CAM close\n"
        self.master.write_bytes(master)
        self.path.write_bytes(b'<sequence background="#121212" camera="wide">'
                              b'<camera id="wide"/><camera id="close" projection="perspective" '
                              b'position="0 0 22" fov-y="24"><animate attributeName="rotation-y">'
                              b'<key frame="0" value="0"/><key frame="9" value="-15"/>'
                              b'</animate></camera></sequence>')
        seq = self.sequence()
        seq.document["cameras"][1]["rotation"][1][1]["value"] = -21
        self.assertTrue(seq.write())
        self.assertEqual(self.master.read_bytes(), master)
        updated = self.sequence()
        self.assertEqual(updated.document["cameras"][1]["rotation"], [0, [
            {"frame": 0, "value": 0}, {"frame": 9, "value": -21}], 0])
        self.assertFalse(updated.write())

    def test_spatial_copy_rotation_and_post_filter_survive_sdk_round_trip(self):
        project = Project.open(self.root, n4=BINARY)
        original = b"""<sequence camera="wide" background="#101020">
          <camera id="wide" projection="perspective" position="0 0 3" fov-y="35"/>
          <scene3d pixels-per-meter="100">
            <distant-light direction="0 0 -1"/>
            <usd source="model.usda" prim="/Card" rotation-x="12">
              <animate attributeName="rotation-y"><key frame="0" value="5"/>
                <key frame="10" value="50"/></animate>
            </usd>
            <plane position="-1 1 0"><text font="font.ttf" width="100" size="16" fill="#ffffff">
              <copy>Scene title</copy></text></plane>
            <filter source="print.wgsl" amount="65"/>
          </scene3d></sequence>"""
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        doc = decode(original)
        layer = doc["graphics"][0]
        layer["content"]["planes"][0]["text"]["content"]["text"] = "New title"
        layer["content"]["models"][0]["rotation_y"][1]["value"] = 80
        layer["filter"]["amount"] = 35
        self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_matte_names_object_references_and_binding_order_survive_edits(self):
        project = Project.open(self.root, n4=BINARY)
        source = b"""<sequence camera="wide" background="#101020">
          <camera id="wide" projection="perspective" fov-y="35"/>
          <scene3d pixels-per-meter="100">
            <usd id="tape" source="model.usda" prim="/Ribbon"/>
            <plane id="type" position="0 0 0">
              <text font="font.ttf" width="100" size="16" fill="#ffffff"><copy>PHASE</copy></text>
            </plane>
            <filter source="ink.wgsl">
              <matte name="word" object="type"/>
              <matte name="hidden" object="type" occluded-by="tape"/>
              <matte name="front" object="tape" coverage="visible"/>
            </filter>
          </scene3d></sequence>"""
        def decode(data):
            return project._run("read-detail", "--detail", "title", "--stdin", candidate=data)
        doc = decode(source)
        layer = doc["graphics"][0]
        masks = layer["filter"]["mattes"]
        self.assertEqual(masks, [{"name": "word", "object": "type"},
                                 {"name": "hidden", "object": "type", "occluded_by": "tape"},
                                 {"name": "front", "object": "tape", "coverage": "visible"}])
        self.assertEqual(layer["content"]["planes"][0]["id"], "type")
        self.assertEqual(layer["content"]["models"][0]["id"], "tape")
        layer["content"]["models"][0]["id"] = "ribbon"
        masks[1]["occluded_by"] = "ribbon"
        masks[2]["object"] = "ribbon"
        masks.reverse()
        self.assertEqual(decode(encode_sequence(doc)), doc)

    def test_source_blur_on_footage_edits_on_its_own_clock(self):
        shutil.copy(ENGINE / "fixtures/numbered.mkv", self.root / "camera.mkv")
        self.master.write_text("project test fps 30/1 size 641x360\n[0,12) V1 title\n")
        original = (
            b'<sequence source="camera.mkv" source-in="7"><footage name="Plate">'
            b'<source-blur sigma="20"><focus-band half-width="3.5" feather="2">'
            b'<animate attributeName="center-y"><key frame="0" value="20"/>'
            b'<key frame="8" value="35"/></animate></focus-band></source-blur>'
            b'</footage></sequence>\n'
        )
        self.path.write_bytes(original)
        seq = self.sequence()
        self.assertFalse(seq.write())
        self.assertEqual(self.path.read_bytes(), original)
        formatted = encode_sequence(seq.document)
        blur = seq.layer("Plate")["effects"][0]
        blur["focus_band"]["center_y"][1]["value"] = 40
        self.assertTrue(seq.write())
        self.assertEqual(self.path.read_bytes(), formatted.replace(b'value="35"', b'value="40"'))

        # A key past this 12-frame cut can still shape the interpolation inside it.
        blur["sigma"] = [{"frame": 1, "value": 0}, {"frame": 16, "value": 32}]
        blur["amount"] = [{"frame": 0, "value": 1, "ease": "hold"}, {"frame": 9, "value": 0.3}]
        self.assertTrue(seq.write())
        saved = self.path.read_bytes()
        reopened = self.sequence()
        self.assertEqual(reopened.layer("Plate")["effects"][0], blur)
        self.assertEqual(reopened.document["source_in"], 7)
        self.assertFalse(reopened.write())

        for key, invalid in (("sigma", 33), ("amount", -0.001)):
            bad = self.sequence()
            bad.layer("Plate")["effects"][0][key] = invalid
            with self.assertRaises(NativeError):
                bad.write()
            self.assertEqual(self.path.read_bytes(), saved)
        for key, invalid in (("half_width", -1), ("feather", 0), ("center_y", [])):
            bad = self.sequence()
            bad.layer("Plate")["effects"][0]["focus_band"][key] = invalid
            with self.assertRaises(NativeError):
                bad.write()
            self.assertEqual(self.path.read_bytes(), saved)

    def test_reordering_source_effects_and_editing_one_grade_channel_round_trip(self):
        original = (
            b'<sequence background="#000000"><rect name="Paper" width="24" height="32">'
            b'<rgb-grade gain="1 .96 .84" offset="-.15 -.13 -.08"/>'
            b'<source-blur sigma="2"/><rgb-grade><animate attributeName="offset">'
            b'<key frame="0" value="0 0 0" ease="hold"/><key frame="3" value=".2 0 .1"/>'
            b'</animate></rgb-grade></rect></sequence>\n'
        )
        self.path.write_bytes(original)
        paper = self.sequence()
        effects = paper.layer("Paper")["effects"]
        self.assertEqual([effect["type"] for effect in effects], ["rgb_grade", "source_blur", "rgb_grade"])
        self.assertFalse(paper.write())
        self.assertEqual(self.path.read_bytes(), original)

        effects[0]["gain"][2] = 0.75
        effects[2]["offset"][1]["value"][1] = -0.1
        effects[0], effects[1] = effects[1], effects[0]
        self.assertTrue(paper.write())
        saved = self.path.read_bytes()
        self.assertLess(saved.index(b"<source-blur"), saved.index(b"<rgb-grade"))
        self.assertIn(b'gain="1 0.96 0.75"', saved)
        self.assertIn(b'value="0.2 -0.1 0.1"', saved)
        reopened = self.sequence()
        self.assertEqual(reopened.layer("Paper")["effects"], effects)
        self.assertFalse(reopened.write())

        reopened.layer("Paper")["effects"][-1]["offset"][1]["value"] = [1, 2]
        with self.assertRaisesRegex(NativeError, "three RGB coefficients"):
            reopened.write()
        self.assertEqual(self.path.read_bytes(), saved)

    def test_multiply_is_editable_on_a_path_and_an_isolated_group(self):
        original = (b'<sequence background="#7e6964">\n  <g name="Card">'
                    b'<path name="Ink" d="M2 2H20V8H2Z" fill="#e4af03" blend="multiply"/>'
                    b'</g>\n</sequence>\n')
        self.path.write_bytes(original)
        seq = self.sequence()
        self.assertEqual(seq.layer("Card", "Ink")["blend"], "multiply")
        self.assertFalse(seq.write())
        self.assertEqual(self.path.read_bytes(), original)
        seq.layer("Card")["blend"] = "multiply"
        self.assertTrue(seq.write())
        saved = self.path.read_bytes()
        self.assertEqual(saved.count(b'blend="multiply"'), 2)
        reopened = self.sequence()
        self.assertEqual(reopened.layer("Card")["blend"], "multiply")
        reopened.layer("Card")["blend"] = "unknown-ink"
        with self.assertRaises(NativeError):
            reopened.write()
        self.assertEqual(self.path.read_bytes(), saved)

    def test_edit_footage_sequence_preserves_source_and_in_point(self):
        shutil.copy(ENGINE / "fixtures/numbered.mkv", self.root / "camera & sound.mkv")
        self.master.write_text("project test fps 30/1 size 641x360\n[0,12) V1 title\n")
        original = b'<sequence source="camera &amp; sound.mkv" source-in="7" />\n'
        self.path.write_bytes(original)
        seq = self.sequence()
        self.assertFalse(seq.write())
        self.assertEqual(self.path.read_bytes(), original)
        title = label("Credit", "Interview", font="font.ttf", width=100, size=16)
        seq.add(title)
        self.assertTrue(seq.write())
        reopened = self.sequence()
        self.assertEqual(reopened.document["source"], "camera & sound.mkv")
        self.assertEqual(reopened.document["source_in"], 7)
        self.assertEqual(reopened.layer("Credit"), title)
        self.assertFalse(reopened.write())

    def test_native_read_and_stable_key_edits(self):
        seq = self.sequence()
        self.assertFalse(seq.write())
        self.assertEqual(self.copy, seq.layer("Title")["content"]["text"])
        self.assertGreater(len(seq.metrics("Title")["lines"]), 1)
        seq.layer("Box")["content"]["corner_radius"][1]["value"] = [8, 8, 8, 2]
        self.assertTrue(seq.write())
        self.assertEqual(self.path.read_bytes(), self.initial.replace(b'value="8 8 8 0"', b'value="8 8 8 2"'))
        self.assertEqual(seq.document, self.sequence().document)
        spans = [{"id": "item", "text": "Coffee"}, {"text": " "}, {"text": " & tea "}]
        seq.layer("Title")["content"]["text"] = spans
        seq.write()
        self.assertEqual(self.sequence().layer("Title")["content"]["text"], spans)
        self.assertIn(b"<span> </span><span> &amp; tea </span>", self.path.read_bytes())
        seq.layer("Box")["content"]["corner_radius"][1]["value"] = [1, 2, 3]
        with self.assertRaises(NativeError):
            seq.write()
        self.assertEqual(self.sequence().layer("Box")["content"]["corner_radius"][1]["value"], [8, 8, 8, 2])

    def test_stroke_width_keys_round_trip_on_rect_and_path(self):
        self.path.write_text(
            '<sequence background="#000000"><rect name="Box" width="32" height="16" '
            'stroke="#ffcc00" stroke-width="3" /><path name="Line" d="M2 2 L20 6" '
            'stroke="#ffffff" stroke-width="2.5" /></sequence>'
        )
        keys = [{"frame": 0, "value": 1.5, "ease": "hold"},
                {"frame": 2, "value": 4, "ease": {"cubic": [0.2, 0, 0.8, 1]}},
                {"frame": 10, "value": 0}]
        seq = self.sequence()
        for name in ("Box", "Line"):
            seq.layer(name)["content"]["stroke"]["width"] = keys
        self.assertTrue(seq.write())
        self.assertEqual(self.path.read_bytes().count(b'attributeName="stroke-width"'), 2)
        reopened = self.sequence()
        for name in ("Box", "Line"):
            self.assertEqual(reopened.layer(name)["content"]["stroke"]["width"], keys)
        self.assertEqual(reopened.layer("Box")["content"]["width"], 32)
        self.assertFalse(reopened.write())

    def test_comments_and_processing_instructions_survive_an_unambiguous_edit(self):
        markup = b'<!-- exposure note\n    <key frame="10" value="8 8 8 0" />\n-->\n<?cue f10?>\n'
        original = markup + self.initial
        self.path.write_bytes(original)
        seq = self.sequence()
        self.assertFalse(seq.write())
        seq.layer("Box")["content"]["corner_radius"][1]["value"] = 12
        self.assertTrue(seq.write())
        self.assertEqual(self.path.read_bytes(), markup + self.initial.replace(b'value="8 8 8 0"', b'value="12"'))
        self.assertFalse(seq.write())
        seq.add(label("New", "Item", font="font.ttf", width=100, size=16))
        with self.assertRaisesRegex(ValueError, "preserving XML comments"):
            seq.write()
        self.assertEqual(self.path.read_bytes(), markup + self.initial.replace(b'value="8 8 8 0"', b'value="12"'))

    def test_nested_transforms_and_independent_curves_round_trip(self):
        original = (
            '<sequence background="#000000"><transform-group name="Rig" position="5 -3" '
            'pivot="10.5 11.25" scale="0.84" rotation="90.42">'
            '<rect name="Art" width="32" height="16" scale-x="0.68"/>'
            '</transform-group></sequence>'
        ).encode()
        self.path.write_bytes(original)
        seq = self.sequence()
        self.assertFalse(seq.write())
        self.assertEqual(self.path.read_bytes(), original)
        rig = seq.layer("Rig")
        self.assertEqual(rig["transform"], {"pivot": [10.5, 11.25], "scale": 0.84, "rotation_degrees": 90.42})
        art = seq.layer("Rig", "Art")
        turns = [{"frame": 0, "value": -12.5}, {"frame": 10, "value": 720}]
        art["channels"] = {"rotation": turns, "scale_y": [{"frame": 0, "value": 1.25}]}
        self.assertTrue(seq.write())
        reopened = self.sequence()
        saved = self.path.read_bytes()
        for attr in (b'pivot="10.5 11.25"', b'rotation="90.42"', b'scale-x="0.68"',
                     b'attributeName="scale-y"', b'attributeName="rotation"'):
            self.assertIn(attr, saved)
        self.assertEqual(reopened.layer("Rig", "Art"), art)
        self.assertEqual(reopened.layer("Rig", "Art")["content"]["width"], 32)
        self.assertFalse(reopened.write())
        for conflicting_scale in ({"scale": 1}, {"scale_y": 1}):
            rejected = self.sequence()
            rejected.layer("Rig", "Art")["transform"].update(conflicting_scale)
            with self.assertRaises(NativeError):
                rejected.write()
            self.assertEqual(self.path.read_bytes(), saved)

    def test_path_trim_endpoints_retime_independently_and_round_trip(self):
        self.path.write_text(
            '<sequence background="#000000"><path name="Route" d="M2 2 H60 V30" '
            'stroke="#ffee00" stroke-width="3" stroke-start="0.2" stroke-end="0.6"/></sequence>'
        )
        keys = [{"frame": 1, "value": 0, "ease": "hold"},
                {"frame": 3, "value": 0.2, "ease": {"cubic": [0.25, 0, 0.75, 1]}},
                {"frame": 8, "value": 1}]
        seq = self.sequence()
        path = seq.layer("Route")["content"]
        path["stroke_start"] = keys
        self.assertTrue(seq.write())
        reopened = self.sequence()
        path = reopened.layer("Route")["content"]
        self.assertEqual(path["stroke_start"], keys)
        self.assertEqual(path["stroke_end"], 0.6)
        self.assertEqual(path["d"], "M2 2 H60 V30")
        self.assertFalse(reopened.write())
        path["stroke_start"], path["stroke_end"] = 0, keys
        self.assertTrue(reopened.write())
        accepted = self.path.read_bytes()
        self.assertIn(b'stroke-start="0"', accepted)
        self.assertIn(b'attributeName="stroke-end"', accepted)
        path["stroke_end"][1]["value"] = -0.2
        with self.assertRaises(NativeError):
            reopened.write()
        self.assertEqual(self.path.read_bytes(), accepted)


if __name__ == "__main__":
    unittest.main()

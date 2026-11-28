"""Independent reference for the reflection close-up, rendered by Blender's own Cycles.

Blender reads the same USD file with its own importer, and this script reads the same shot.json,
artwork frames and light rig as the harness. Nothing here is linked into the product.

  Blender -b --factory-startup --python reference/blender_reference.py -- \
      --shot shots/field-unit-reflection/shot.json --out out/reference --frames 0 51 --samples 256
"""
import argparse, json, math, sys, time
from pathlib import Path
import bpy
import numpy as np
from mathutils import Matrix, Vector

p = argparse.ArgumentParser()
p.add_argument("--shot", type=Path, required=True)
p.add_argument("--out", type=Path, required=True)
p.add_argument("--frames", type=int, nargs=2, default=[0, 1])
p.add_argument("--step", type=int, default=1)
p.add_argument("--samples", type=int, default=256)
p.add_argument("--scale", type=float, default=1.0)
p.add_argument("--denoise", action="store_true")
p.add_argument("--device", default="GPU")
p.add_argument("--artwork", default="artwork/base")
p.add_argument("--seed", type=int, default=0)
args = p.parse_args(sys.argv[sys.argv.index("--") + 1:])
shot = json.loads(args.shot.read_text())
root = args.shot.resolve().parent
args.out.mkdir(parents=True, exist_ok=True)


def B(v):
    """N4 is Y-up; Blender is Z-up. (x, y, z) -> (x, -z, y)."""
    return Vector((v[0], -v[2], v[1]))


bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
t0 = time.time()
bpy.ops.wm.usd_import(filepath=str(root / shot["asset"]))
import_seconds = time.time() - t0
meshes = [o for o in scene.objects if o.type == "MESH"]
lo = Vector((1e9,) * 3); hi = Vector((-1e9,) * 3)
for o in meshes:
    if "Floor" in o.name:
        continue
    for c in o.bound_box:
        w = o.matrix_world @ Vector(c)
        lo = Vector(map(min, lo, w)); hi = Vector(map(max, hi, w))
print("REFERENCE imported", len(meshes), "meshes; product bounds", tuple(round(x, 4) for x in lo), tuple(round(x, 4) for x in hi))

scene.render.engine = "CYCLES"
prefs = bpy.context.preferences.addons["cycles"].preferences
if args.device == "GPU":
    prefs.compute_device_type = "METAL"
    prefs.get_devices()
    for d in prefs.devices:
        d.use = d.type == "METAL"
    scene.cycles.device = "GPU"
else:
    scene.cycles.device = "CPU"
c = scene.cycles
c.samples = args.samples
c.use_adaptive_sampling = False
c.max_bounces = 8; c.diffuse_bounces = 8; c.glossy_bounces = 8
c.sample_clamp_indirect = 10.0; c.sample_clamp_direct = 0.0
c.use_denoising = args.denoise
c.denoiser = "OPENIMAGEDENOISE"
c.caustics_reflective = True; c.caustics_refractive = True
c.blur_glossy = 1.0  # Cycles default filter_glossy, as in the standalone library
scene.render.resolution_x = int(shot["size"][0] * args.scale)
scene.render.resolution_y = int(shot["size"][1] * args.scale)
scene.render.resolution_percentage = 100
scene.render.film_transparent = False
scene.render.use_persistent_data = True
scene.render.image_settings.file_format = "OPEN_EXR"
scene.render.image_settings.color_depth = "32"
scene.render.image_settings.color_mode = "RGBA"
scene.view_settings.view_transform = "Standard"

world = bpy.data.worlds.new("rig"); scene.world = world; world.use_nodes = True
bg = world.node_tree.nodes["Background"]
bg.inputs["Color"].default_value = (*shot["lights"]["background"], 1); bg.inputs["Strength"].default_value = 1.0

cam = bpy.data.cameras.new("cam"); cam_ob = bpy.data.objects.new("cam", cam); scene.collection.objects.link(cam_ob); scene.camera = cam_ob
cam.sensor_fit = "VERTICAL"; cam.sensor_height = 24; cam.clip_start = 0.01; cam.clip_end = 1000
cpos = B(shot["camera"]["position"]); ctar = B(shot["camera"]["target"])
cam_ob.location = cpos
cam_ob.rotation_euler = (ctar - cpos).to_track_quat("-Z", "Y").to_euler()
cam.lens = cam.sensor_height / (2 * math.tan(math.radians(shot["camera"]["fov_y"]) / 2))

# Area lights are specified by radiance. Blender takes watts; a rectangle of area A emits
# radiance P / (pi A) per channel for unit colour (measured by reference/blender_calibrate.py).
lights = []
for l in shot["lights"]["rects"]:
    data = bpy.data.lights.new(l["name"], "AREA")
    data.shape = "RECTANGLE"; data.size = l["width"]; data.size_y = l["height"]
    L = l["radiance"]; peak = max(L)
    data.color = [x / peak for x in L]
    data.energy = peak * math.pi * l["width"] * l["height"]
    ob = bpy.data.objects.new(l["name"], data); scene.collection.objects.link(ob)
    n = (B(l["aim"]) - B(l["center"])).normalized()
    u = B(l["u"]).normalized(); v = n.cross(u) * -1  # local X = u, local -Z = n
    z = -n; y = z.cross(u)
    ob.matrix_world = Matrix(((u.x, y.x, z.x, 0), (u.y, y.y, z.y, 0), (u.z, y.z, z.z, 0), (0, 0, 0, 1)))
    ob.location = B(l["center"])
    ob.visible_camera = False
    lights.append(ob)

# Materials that carry live artwork, built to match the bridge.
def find_material(slot):
    name = slot.rsplit("/", 1)[-1]
    for m in bpy.data.materials:
        if m.name.split(".")[0] == name:
            return m
    raise SystemExit(f"material {name} not found in {[m.name for m in bpy.data.materials]}")

disp = find_material(shot["display"]["slot"]); disp.use_nodes = True
nt = disp.node_tree; nt.nodes.clear()
tex = nt.nodes.new("ShaderNodeTexImage"); tex.extension = "REPEAT"
em = nt.nodes.new("ShaderNodeEmission"); lp = nt.nodes.new("ShaderNodeLightPath")
mix = nt.nodes.new("ShaderNodeMix"); mix.data_type = "FLOAT"
mix.inputs["A"].default_value = shot["display"]["emission"] * shot["display"]["glow"]
mix.inputs["B"].default_value = shot["display"]["emission"]
out = nt.nodes.new("ShaderNodeOutputMaterial")
nt.links.new(lp.outputs["Is Camera Ray"], mix.inputs["Factor"])
nt.links.new(mix.outputs["Result"], em.inputs["Strength"])
nt.links.new(tex.outputs["Color"], em.inputs["Color"])
nt.links.new(em.outputs["Emission"], out.inputs["Surface"])
display_tex = tex

plate = find_material(shot["nameplate"]["slot"])
bsdf = next(n for n in plate.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
ptex = plate.node_tree.nodes.new("ShaderNodeTexImage")
ptex.image = bpy.data.images.load(str(root / shot["nameplate"]["image"]))
plate.node_tree.links.new(ptex.outputs["Color"], bsdf.inputs["Base Color"])

print("REFERENCE materials:")
for m in bpy.data.materials:
    b = next((n for n in m.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None) if m.node_tree else None
    if b:
        print("  ", m.name, "base", tuple(round(x, 3) for x in b.inputs["Base Color"].default_value[:3]),
              "metal", round(b.inputs["Metallic"].default_value, 3), "rough", round(b.inputs["Roughness"].default_value, 3),
              "spec", round(b.inputs["Specular IOR Level"].default_value, 3), "ior", round(b.inputs["IOR"].default_value, 3),
              "coat", round(b.inputs["Coat Weight"].default_value, 3))

hinge = next(o for o in scene.objects if o.name.split(".")[0] == shot["hinge"]["part"].rsplit("/", 1)[-1])
rest = hinge.matrix_world.copy()
pivot = rest.translation.copy()
print("REFERENCE hinge origin", tuple(round(x, 4) for x in pivot))


def key(keys, f):
    if f <= keys[0][0]:
        return keys[0][1]
    for a, b in zip(keys, keys[1:]):
        if f <= b[0]:
            t = (f - a[0]) / (b[0] - a[0]); t = t * t * (3 - 2 * t)
            return a[1] + (b[1] - a[1]) * t
    return keys[-1][1]


def exr_to_f32(exr, raw):
    im = bpy.data.images.load(str(exr))
    w, h = im.size
    px = np.empty(w * h * 4, dtype=np.float32); im.pixels.foreach_get(px)
    px = px.reshape(h, w, 4)[::-1]
    px.astype("<f4").tofile(raw)
    bpy.data.images.remove(im)


rows = []
start = time.time()
for f in range(args.frames[0], args.frames[1], args.step):
    angle = math.radians(key(shot["hinge"]["keys"], f))
    hinge.matrix_world = Matrix.Translation(pivot) @ Matrix.Rotation(angle, 4, "X") @ Matrix.Translation(-pivot) @ rest
    if display_tex.image:
        bpy.data.images.remove(display_tex.image)
    display_tex.image = bpy.data.images.load(str(root / args.artwork / f"signal-{f:03}.png"))
    c.seed = args.seed * 7919 + f
    exr = args.out / f"frame-{f:03}.exr"
    scene.render.filepath = str(exr)
    t = time.time()
    bpy.ops.render.render(write_still=True)
    wall = time.time() - t
    exr_to_f32(exr, args.out / f"frame-{f:03}.rgba.f32")
    exr.unlink()
    rows.append({"frame": f, "wall_seconds": wall})
    print(f"REFERENCE frame {f}: {wall:.3f}s")
receipt = {"blender": bpy.app.version_string, "device": args.device, "samples": args.samples, "denoise": args.denoise,
           "size": [scene.render.resolution_x, scene.render.resolution_y], "usd_import_seconds": import_seconds,
           "frames": rows, "total_seconds": time.time() - start}
(args.out / "receipt.json").write_text(json.dumps(receipt, indent=2))

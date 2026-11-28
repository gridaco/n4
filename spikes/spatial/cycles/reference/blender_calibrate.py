"""Measure the radiance Blender emits per watt for a rectangular area light, seen directly."""
import bpy, numpy as np, math, tempfile, os
bpy.ops.wm.read_factory_settings(use_empty=True)
s = bpy.context.scene; s.render.engine = "CYCLES"; s.cycles.device = "CPU"; s.cycles.samples = 16
s.render.resolution_x = s.render.resolution_y = 16; s.view_settings.view_transform = "Standard"
s.render.image_settings.file_format = "OPEN_EXR"; s.render.image_settings.color_depth = "32"
w = bpy.data.worlds.new("w"); s.world = w; w.use_nodes = True; w.node_tree.nodes["Background"].inputs["Strength"].default_value = 0
cam = bpy.data.objects.new("c", bpy.data.cameras.new("c")); s.collection.objects.link(cam); s.camera = cam
cam.location = (0, 0, 2)  # looks down -Z by default
for size, size_y, watts in [(1.0, 1.0, 1.0), (0.5, 0.3, 10.0)]:
    d = bpy.data.lights.new("l", "AREA"); d.shape = "RECTANGLE"; d.size = size; d.size_y = size_y; d.energy = watts
    o = bpy.data.objects.new("l", d); s.collection.objects.link(o)
    o.rotation_euler = (math.pi, 0, 0)  # emit +Z, towards the camera
    o.visible_camera = True
    path = os.path.join(tempfile.gettempdir(), "n4cal.exr"); s.render.filepath = path
    bpy.ops.render.render(write_still=True)
    im = bpy.data.images.load(path); px = np.array(im.pixels[:]).reshape(16, 16, 4)
    L = px[8, 8, 0]; A = size * size_y
    print(f"CAL area {A} m2, {watts} W: radiance {L:.6f}; L*A/P = {L*A/watts:.6f}; 1/(4pi)={1/(4*math.pi):.6f} 1/4={0.25} 1/pi={1/math.pi:.6f}")
    bpy.data.objects.remove(o)

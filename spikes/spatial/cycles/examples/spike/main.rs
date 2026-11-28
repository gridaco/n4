//! Headless host for the Cycles intake spike. It stands in for N4: it loads the asset through the
//! importer, evaluates one shot at a frame, hands the evaluated scene to Cycles and writes receipts.
use anyhow::{Context, Result, bail, ensure};
use nothing_spatial::Mesh;
use nothing_spatial_cycles::{
    Artwork, Frame, ImageId, LightId, MaterialId, ObjectId, Recipe, Session,
};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

mod draft;

type V3 = [f32; 3];
pub(crate) type M34 = [f32; 12];

pub(crate) fn v3(v: &Value) -> V3 {
    let a = v.as_array().expect("vec3");
    [
        a[0].as_f64().unwrap() as f32,
        a[1].as_f64().unwrap() as f32,
        a[2].as_f64().unwrap() as f32,
    ]
}
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

const IDENTITY: M34 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];

pub(crate) fn mul(a: &M34, b: &M34) -> M34 {
    let mut o = [0f32; 12];
    for r in 0..3 {
        for c in 0..4 {
            o[r * 4 + c] = a[r * 4] * b[c]
                + a[r * 4 + 1] * b[4 + c]
                + a[r * 4 + 2] * b[8 + c]
                + if c == 3 { a[r * 4 + 3] } else { 0. };
        }
    }
    o
}

/// Columns of Rz * Ry * Rx, matching nothing-spatial-wgpu's Euler convention.
fn euler(deg: V3) -> [V3; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    [
        [cz * cy, sz * cy, -sy],
        [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx],
        [cz * sy * cx + sz * sx, sz * sy * cx - cz * sx, cy * cx],
    ]
}

/// Rotation by `deg` in the part's own frame about its pivot, as the rasterizer does it.
pub(crate) fn part_rotation(frame: &[[f32; 4]; 4], pivot: V3, deg: V3) -> M34 {
    let x = norm([frame[0][0], frame[0][1], frame[0][2]]);
    let y = norm([frame[1][0], frame[1][1], frame[1][2]]);
    let z = norm([frame[2][0], frame[2][1], frame[2][2]]);
    let f: M34 = [
        x[0], y[0], z[0], pivot[0], x[1], y[1], z[1], pivot[1], x[2], y[2], z[2], pivot[2],
    ];
    let inv: M34 = [
        x[0],
        x[1],
        x[2],
        -dot(x, pivot),
        y[0],
        y[1],
        y[2],
        -dot(y, pivot),
        z[0],
        z[1],
        z[2],
        -dot(z, pivot),
    ];
    let [cx, cy, cz] = euler(deg);
    let r: M34 = [
        cx[0], cy[0], cz[0], 0., cx[1], cy[1], cz[1], 0., cx[2], cy[2], cz[2], 0.,
    ];
    mul(&mul(&f, &r), &inv)
}

/// Set by commands that watch a render in progress.
static SNAPSHOTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Model {
    mesh: Mesh,
    materials: Vec<MaterialId>,
    objects: Vec<ObjectId>,
}

struct Shot {
    dir: PathBuf,
    spec: Value,
    session: Session,
    product: Model,
    display: ImageId,
    lights: Vec<LightId>,
    size: [u32; 2],
}

pub(crate) fn load_png(path: &Path) -> Result<([u32; 2], Vec<u8>)> {
    let im = image::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .to_rgba8();
    Ok(([im.width(), im.height()], im.into_raw()))
}

pub(crate) fn key(keys: &Value, frame: f32) -> f32 {
    let k: Vec<(f32, f32)> = keys
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_f64().unwrap() as f32, p[1].as_f64().unwrap() as f32))
        .collect();
    if frame <= k[0].0 {
        return k[0].1;
    }
    for w in k.windows(2) {
        if frame <= w[1].0 {
            let t = (frame - w[0].0) / (w[1].0 - w[0].0);
            let t = t * t * (3. - 2. * t);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    k[k.len() - 1].1
}

impl Shot {
    fn open(path: &Path, device: &str, timings: &mut Value) -> Result<Self> {
        let dir = path.parent().unwrap().to_path_buf();
        let spec: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let size = [
            spec["size"][0].as_u64().unwrap() as u32,
            spec["size"][1].as_u64().unwrap() as u32,
        ];
        let t = Instant::now();
        let asset = dir.join(spec["asset"].as_str().unwrap());
        let product_mesh = nothing_openusd::load(&asset, spec["product"].as_str().unwrap())?;
        let ground_mesh = nothing_openusd::load(&asset, spec["ground"].as_str().unwrap())?;
        timings["import_seconds"] = json!(t.elapsed().as_secs_f64());
        let t = Instant::now();
        let mut session = if SNAPSHOTS.load(std::sync::atomic::Ordering::Relaxed) {
            Session::with_snapshots(device, 0)?
        } else {
            Session::new(device, 0)?
        };
        timings["session_create_seconds"] = json!(t.elapsed().as_secs_f64());
        timings["device"] = json!(session.device());

        let t = Instant::now();
        let display_slot = spec["display"]["slot"].as_str().unwrap().to_string();
        let scale = spec["display"]["emission"].as_f64().unwrap() as f32;
        let glow = spec["display"]["glow"].as_f64().unwrap() as f32;
        let plate_slot = spec["nameplate"]["slot"].as_str().unwrap().to_string();
        let (psize, plate) = load_png(&dir.join(spec["nameplate"]["image"].as_str().unwrap()))?;
        let plate = session.image(psize, true, &plate)?;
        let display = session.image([2, 2], true, &[0u8; 16])?;
        let art = |slot: &str| {
            if slot == display_slot {
                Artwork::Emission {
                    image: display,
                    scale,
                    glow,
                }
            } else if slot == plate_slot {
                Artwork::BaseColor(plate)
            } else {
                Artwork::None
            }
        };
        let materials = session.materials(&product_mesh, &art)?;
        let objects = session.model(&product_mesh, &materials, 1)?;
        let ground_materials = session.materials(&ground_mesh, &|_| Artwork::None)?;
        session.model(&ground_mesh, &ground_materials, 2)?;
        let mut lights = Vec::new();
        for l in spec["lights"]["rects"].as_array().unwrap() {
            lights.push(session.rect_light(
                v3(&l["center"]),
                v3(&l["u"]),
                v3(&l["v"]),
                v3(&l["radiance"]),
                false,
            ));
        }
        session.background(v3(&spec["lights"]["background"]), false);
        timings["scene_build_seconds"] = json!(t.elapsed().as_secs_f64());
        timings["triangles"] = json!((product_mesh.indices.len() + ground_mesh.indices.len()) / 3);
        timings["parts"] = json!(product_mesh.parts.len());
        timings["slots"] = json!(
            product_mesh
                .materials
                .iter()
                .map(|m| m.slot.clone())
                .collect::<Vec<_>>()
        );
        Ok(Self {
            dir,
            spec,
            session,
            product: Model {
                mesh: product_mesh,
                materials,
                objects,
            },
            display,
            lights,
            size,
        })
    }

    /// Evaluate the shot at a frame: hinge pose and camera. Artwork is set separately.
    fn pose(&mut self, frame: f32) {
        let hinge = self.spec["hinge"]["part"].as_str().unwrap();
        let axis = self.spec["hinge"]["axis"].as_u64().unwrap_or(0) as usize;
        let angle = key(&self.spec["hinge"]["keys"], frame);
        let mut deg = [0f32; 3];
        deg[axis] = angle;
        let parts = &self.product.mesh.parts;
        let mut world: Vec<M34> = Vec::with_capacity(parts.len());
        for part in parts {
            let parent = part.parent.map_or(IDENTITY, |p| world[p]);
            world.push(if part.path == hinge {
                mul(&parent, &part_rotation(&part.frame, part.pivot, deg))
            } else {
                parent
            });
        }
        for (object, m) in self.product.objects.iter().zip(&world) {
            self.session.transform(*object, m);
        }
        let cam = &self.spec["camera"];
        let [x, y, z] = euler(v3(&cam["rotation"]));
        let fov = if cam["fov_keys"].is_array() {
            key(&cam["fov_keys"], frame)
        } else {
            cam["fov_y"].as_f64().unwrap() as f32
        };
        self.session
            .camera(v3(&cam["position"]), x, y, [-z[0], -z[1], -z[2]], fov);
    }

    fn artwork(&mut self, dir: &Path, frame: u32) -> Result<()> {
        let (size, rgba) = load_png(&dir.join(format!("signal-{frame:03}.png")))?;
        self.session.update_image(self.display, size, &rgba)
    }
}

fn write_f32(path: &Path, data: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
    std::fs::write(path, bytes)?;
    Ok(())
}

fn save(out: &Path, name: &str, frame: &Frame) -> Result<()> {
    write_f32(&out.join(format!("{name}.rgba.f32")), &frame.color)?;
    for (i, m) in frame.masks.iter().enumerate() {
        write_f32(&out.join(format!("{name}.mask{i}.f32")), m)?;
    }
    Ok(())
}

pub(crate) fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}
fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn recipe(args: &[String]) -> Recipe {
    Recipe {
        samples: arg(args, "--samples").map_or(64, |s| s.parse().unwrap()),
        max_bounces: arg(args, "--bounces").map_or(8, |s| s.parse().unwrap()),
        clamp_indirect: arg(args, "--clamp").map_or(10., |s| s.parse().unwrap()),
        denoise: flag(args, "--denoise"),
        seed: arg(args, "--seed").map_or(0, |s| s.parse().unwrap()),
    }
}

/// Lights and camera conventions, checked against quantities with known answers.
fn calib(args: &[String]) -> Result<()> {
    let device = arg(args, "--device").unwrap_or("METAL");
    let mut s = Session::new(device, 0)?;
    println!("device: {}", s.device());
    // A 1 m white diffuse square on the floor, facing +Y.
    let mut mesh = Mesh::default();
    mesh.materials.push(nothing_spatial::Material {
        diffuse: [0.5; 3],
        roughness: 1.,
        ..Default::default()
    });
    for (x, z) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        mesh.vertices.push(nothing_spatial::Vertex {
            position: [x, 0., z],
            normal: [0., 1., 0.],
            material: 0,
            uv: [x + 0.5, z + 0.5],
        });
    }
    mesh.indices = vec![0, 2, 1, 0, 3, 2];
    mesh.parts.push(nothing_spatial::Part {
        path: "/floor".into(),
        parent: None,
        indices: 0..6,
        frame: [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ],
        pivot: [0.; 3],
    });
    let mats = s.materials(&mesh, &|_| Artwork::None)?;
    s.model(&mesh, &mats, 1)?;
    s.background([0.; 3], false);
    // Camera 2 m above, looking straight down.
    s.camera(
        [0., 2., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., -1., 0.],
        40.,
    );
    let r = Recipe {
        samples: 256,
        max_bounces: 1,
        clamp_indirect: 0.,
        denoise: false,
        seed: 0,
    };
    let size = [64u32, 64u32];
    let center = |f: &Frame| {
        let i = ((32 * 64 + 32) * 4) as usize;
        [f.color[i], f.color[i + 1], f.color[i + 2], f.color[i + 3]]
    };
    // 1. Distant light straight down with irradiance 3.0: expect radiance = albedo * E / pi = 0.4775.
    let _sun = s.distant_light([0., -1., 0.], [3., 3., 3.], 0.);
    let f = s.render(size, &r, &[1])?;
    println!(
        "distant E=3 on albedo 0.5: centre radiance {:?} (expect 0.4775), mask centre {}",
        center(&f),
        f.masks[0][32 * 64 + 32]
    );
    // 2. A camera-visible rect light of radiance 2.0 seen directly: expect 2.0.
    let mut s2 = Session::new(device, 0)?;
    s2.background([0.; 3], false);
    s2.camera(
        [0., 2., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., -1., 0.],
        40.,
    );
    s2.rect_light([0., 0., 0.], [0., 0., 1.], [1., 0., 0.], [2., 2., 2.], true);
    let f = s2.render(size, &r, &[])?;
    println!(
        "rect light radiance 2 seen directly (emitting +Y): {:?} (expect 2.0)",
        center(&f)
    );
    // 3. Same light lighting the floor from 1 m above, emitting -Y. Large-source check only.
    let mut s3 = Session::new(device, 0)?;
    let mats = s3.materials(&mesh, &|_| Artwork::None)?;
    s3.model(&mesh, &mats, 1)?;
    s3.background([0.; 3], false);
    s3.camera(
        [0., 2., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., -1., 0.],
        40.,
    );
    s3.rect_light(
        [0., 1., 0.],
        [0.2, 0., 0.],
        [0., 0., 0.2],
        [10., 10., 10.],
        false,
    );
    let f = s3.render(size, &r, &[])?;
    // Small source: E ~= L * A / d^2 = 10 * 0.04 = 0.4; radiance = 0.5 * 0.4 / pi = 0.0637.
    println!(
        "rect L=10, 0.2 m square, 1 m above floor: {:?} (expect about 0.062)",
        center(&f)
    );
    Ok(())
}

fn render(args: &[String]) -> Result<()> {
    let shot_path = PathBuf::from(arg(args, "--shot").context("--shot")?);
    let out = PathBuf::from(arg(args, "--out").context("--out")?);
    std::fs::create_dir_all(&out)?;
    let device = arg(args, "--device").unwrap_or("METAL");
    let mut receipt = json!({});
    let start = Instant::now();
    let mut shot = Shot::open(&shot_path, device, &mut receipt)?;
    let art = arg(args, "--artwork")
        .map(PathBuf::from)
        .unwrap_or(shot.dir.join("artwork/base"));
    let (a, b) = arg(args, "--frames")
        .unwrap_or("0..1")
        .split_once("..")
        .context("--frames a..b")?;
    let mut frames: Vec<u32> = (a.parse()?..b.parse()?).collect();
    if flag(args, "--reverse") {
        frames.reverse();
    }
    let step: usize = arg(args, "--step").map_or(1, |s| s.parse().unwrap());
    let frames: Vec<u32> = frames.into_iter().step_by(step).collect();
    let r = recipe(args);
    let freeze: Option<u32> = arg(args, "--freeze-artwork").map(|s| s.parse().unwrap());
    let freeze_pose: Option<f32> = arg(args, "--freeze-pose").map(|s| s.parse().unwrap());
    let masks: Vec<u32> = if flag(args, "--masks") {
        vec![1, 2]
    } else {
        vec![]
    };
    let size = arg(args, "--scale").map_or(shot.size, |s| {
        let k: f32 = s.parse().unwrap();
        [
            (shot.size[0] as f32 * k) as u32,
            (shot.size[1] as f32 * k) as u32,
        ]
    });
    let mut rows = Vec::new();
    for (n, &f) in frames.iter().enumerate() {
        let t = Instant::now();
        shot.pose(freeze_pose.unwrap_or(f as f32));
        shot.artwork(&art, freeze.unwrap_or(f))?;
        let seed = if flag(args, "--fixed-seed") {
            r.seed
        } else {
            r.seed.wrapping_mul(7919).wrapping_add(f)
        };
        let frame = shot
            .session
            .render(size, &Recipe { seed, ..r.clone() }, &masks)?;
        let wall = t.elapsed().as_secs_f64();
        save(&out, &format!("frame-{f:03}"), &frame)?;
        if n == 0 {
            receipt["first_frame_from_process_start_seconds"] =
                json!(start.elapsed().as_secs_f64());
        }
        println!(
            "frame {f}: wall {wall:.3}s sync {:.3}s render {:.3}s",
            frame.timing.sync_seconds, frame.timing.render_seconds
        );
        rows.push(json!({"frame": f, "wall_seconds": wall, "sync_seconds": frame.timing.sync_seconds, "render_seconds": frame.timing.render_seconds, "seed": seed}));
    }
    receipt["size"] = json!(size);
    receipt["recipe"] = json!({"samples": r.samples, "max_bounces": r.max_bounces, "clamp_indirect": r.clamp_indirect, "denoise": r.denoise, "seed": r.seed});
    receipt["frames"] = json!(rows);
    receipt["peak_device_memory_bytes"] = json!(shot.session.peak_device_memory());
    receipt["total_seconds"] = json!(start.elapsed().as_secs_f64());
    std::fs::File::create(out.join("receipt.json"))?
        .write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    Ok(())
}

/// One live session, a sequence of edits, the cost of each. Low sample counts expose fixed overhead.
fn edits(args: &[String]) -> Result<()> {
    let shot_path = PathBuf::from(arg(args, "--shot").context("--shot")?);
    let out = PathBuf::from(arg(args, "--out").context("--out")?);
    std::fs::create_dir_all(&out)?;
    let device = arg(args, "--device").unwrap_or("METAL");
    let mut receipt = json!({});
    let mut shot = Shot::open(&shot_path, device, &mut receipt)?;
    let base = shot.dir.join("artwork/base");
    let word = shot.dir.join("artwork/word");
    let frame: u32 = arg(args, "--frame").map_or(30, |s| s.parse().unwrap());
    let r = recipe(args);
    let size = shot.size;
    let mut rows = Vec::new();
    let mut run = |shot: &mut Shot,
                   label: &str,
                   samples: u32,
                   save_as: Option<&str>|
     -> Result<()> {
        let t = Instant::now();
        let f = shot.session.render(
            size,
            &Recipe {
                samples,
                ..r.clone()
            },
            &[],
        )?;
        let wall = t.elapsed().as_secs_f64();
        if let Some(name) = save_as {
            save(&out, name, &f)?;
        }
        println!(
            "{label:42} spp {samples:4}  wall {wall:7.3}s  sync {:6.3}s  render {:7.3}s",
            f.timing.sync_seconds, f.timing.render_seconds
        );
        rows.push(json!({"edit": label, "samples": samples, "wall_seconds": wall, "sync_seconds": f.timing.sync_seconds, "render_seconds": f.timing.render_seconds}));
        Ok(())
    };
    shot.pose(frame as f32);
    shot.artwork(&base, frame)?;
    run(&mut shot, "cold: first render in process", 1, None)?;
    run(&mut shot, "no change, 1 sample", 1, None)?;
    run(
        &mut shot,
        "no change, recipe",
        r.samples,
        Some("edit-0-base"),
    )?;
    for round in 0..3 {
        shot.artwork(&word, frame)?;
        run(
            &mut shot,
            &format!("word changed on screen (round {round})"),
            1,
            None,
        )?;
        shot.artwork(&base, frame)?;
        run(
            &mut shot,
            &format!("word changed back (round {round})"),
            1,
            None,
        )?;
    }
    shot.artwork(&word, frame)?;
    run(
        &mut shot,
        "word changed, recipe",
        r.samples,
        Some("edit-1-word"),
    )?;
    shot.artwork(&base, frame + 3)?;
    run(&mut shot, "footage slipped 3 frames, 1 sample", 1, None)?;
    run(
        &mut shot,
        "footage slipped 3 frames, recipe",
        r.samples,
        Some("edit-2-slip"),
    )?;
    shot.artwork(&base, frame)?;
    for round in 0..3 {
        shot.pose(frame as f32 + 1. + round as f32);
        run(
            &mut shot,
            &format!("hinge and camera to next frame (round {round})"),
            1,
            None,
        )?;
    }
    shot.pose(frame as f32);
    run(&mut shot, "pose restored, 1 sample", 1, None)?;
    // Move and resize the key light.
    let l = &shot.spec["lights"]["rects"][0].clone();
    let (c, u, v, rad) = (
        v3(&l["center"]),
        v3(&l["u"]),
        v3(&l["v"]),
        v3(&l["radiance"]),
    );
    let key = shot.lights[0];
    shot.session.update_rect_light(
        key,
        [c[0] + 0.08, c[1], c[2]],
        [u[0] * 0.5, u[1] * 0.5, u[2] * 0.5],
        v,
        [rad[0] * 2., rad[1] * 2., rad[2] * 2.],
    );
    run(&mut shot, "key light moved and halved, 1 sample", 1, None)?;
    run(
        &mut shot,
        "key light moved and halved, recipe",
        r.samples,
        Some("edit-3-light"),
    )?;
    shot.session.update_rect_light(key, c, u, v, rad);
    // Change one material value: make the first non-display slot glossier.
    let slot = arg(args, "--material-slot").map(str::to_string);
    if let Some(slot) = slot {
        let i = shot
            .product
            .mesh
            .materials
            .iter()
            .position(|m| m.slot == slot)
            .context("slot")?;
        let mut m = shot.product.mesh.materials[i].clone();
        m.roughness = (m.roughness * 0.4).max(0.05);
        let id = shot.product.materials[i];
        shot.session.update_material(id, &m, Artwork::None);
        run(
            &mut shot,
            "one material roughness changed, 1 sample",
            1,
            None,
        )?;
        run(
            &mut shot,
            "one material roughness changed, recipe",
            r.samples,
            Some("edit-4-material"),
        )?;
    }
    receipt["edits"] = json!(rows);
    receipt["size"] = json!(size);
    receipt["peak_device_memory_bytes"] = json!(shot.session.peak_device_memory());
    std::fs::File::create(out.join("receipt.json"))?
        .write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    Ok(())
}

/// Both of N4's mask kinds. "Visible" comes from the beauty pass. "Projected" (the object's whole
/// silhouette, ignoring occluders) has no Cycles pass, so it is a second, cheap render in which
/// every other object is hidden from the camera.
fn masks(args: &[String]) -> Result<()> {
    let shot_path = PathBuf::from(arg(args, "--shot").context("--shot")?);
    let out = PathBuf::from(arg(args, "--out").context("--out")?);
    std::fs::create_dir_all(&out)?;
    let mut receipt = json!({});
    let mut shot = Shot::open(
        &shot_path,
        arg(args, "--device").unwrap_or("METAL"),
        &mut receipt,
    )?;
    let frame: u32 = arg(args, "--frame").map_or(25, |s| s.parse().unwrap());
    let r = recipe(args);
    let size = shot.size;
    shot.pose(frame as f32);
    let base = shot.dir.join("artwork/base");
    shot.artwork(&base, frame)?;
    let t = Instant::now();
    let beauty = shot.session.render(size, &r, &[1, 2])?;
    let beauty_s = t.elapsed().as_secs_f64();
    save(&out, "beauty", &beauty)?;
    // Projected floor: hide the product from camera rays only. It still shadows and reflects.
    let objects = shot.product.objects.clone();
    for o in &objects {
        shot.session.camera_visible(*o, false);
    }
    let mut rows = Vec::new();
    for samples in [4u32, 16, 64] {
        let t = Instant::now();
        let p = shot.session.render(
            size,
            &Recipe {
                samples,
                denoise: false,
                ..r.clone()
            },
            &[2],
        )?;
        let wall = t.elapsed().as_secs_f64();
        write_f32(
            &out.join(format!("projected-floor-{samples}.f32")),
            &p.masks[0],
        )?;
        println!(
            "projected floor mask, {samples} spp: {wall:.3}s (sync {:.3}s)",
            p.timing.sync_seconds
        );
        rows.push(json!({"samples": samples, "wall_seconds": wall, "sync_seconds": p.timing.sync_seconds}));
    }
    for o in &objects {
        shot.session.camera_visible(*o, true);
    }
    let t = Instant::now();
    let again = shot.session.render(size, &r, &[1, 2])?;
    println!(
        "beauty {beauty_s:.3}s; beauty after restoring visibility {:.3}s",
        t.elapsed().as_secs_f64()
    );
    let same = beauty
        .color
        .iter()
        .zip(&again.color)
        .all(|(a, b)| a.to_bits() == b.to_bits());
    receipt["projected"] = json!(rows);
    receipt["beauty_seconds"] = json!(beauty_s);
    receipt["beauty_identical_after_restore"] = json!(same);
    std::fs::File::create(out.join("receipt.json"))?
        .write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    Ok(())
}

/// Features the first pass of the spike left untested: stopping a render, in-progress pictures,
/// depth, Cryptomatte against the bridge's own masks, and texture maps.
fn features(args: &[String]) -> Result<()> {
    use nothing_spatial_cycles::Outputs;
    let shot_path = PathBuf::from(arg(args, "--shot").context("--shot")?);
    let out = PathBuf::from(arg(args, "--out").context("--out")?);
    std::fs::create_dir_all(&out)?;
    SNAPSHOTS.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut receipt = json!({});
    let mut shot = Shot::open(
        &shot_path,
        arg(args, "--device").unwrap_or("METAL"),
        &mut receipt,
    )?;
    let size = shot.size;
    let pixels = (size[0] * size[1]) as usize;
    shot.pose(25.);
    let base = shot.dir.join("artwork/base");
    shot.artwork(&base, 25)?;
    let r = recipe(args);
    shot.session.render(
        size,
        &Recipe {
            samples: 1,
            denoise: false,
            ..r.clone()
        },
        &[],
    )?; // warm up

    // 1. Progress and in-progress pictures during a long render, then cancel it.
    let monitor = shot.session.monitor();
    let long = Recipe {
        samples: 2048,
        denoise: false,
        ..r.clone()
    };
    let start = Instant::now();
    let (first_snapshot, cancel_at, snapshots, progress_at_cancel, result) =
        std::thread::scope(|scope| {
            let watcher = scope.spawn(move || {
                let mut first = None;
                let mut buffer = vec![0f32; pixels * 4];
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    if first.is_none() && monitor.snapshots() > 0 && monitor.snapshot(&mut buffer) {
                        first = Some(start.elapsed().as_secs_f64());
                    }
                    if start.elapsed().as_secs_f64() > 3.0 {
                        let p = monitor.progress();
                        let n = monitor.snapshots();
                        let at = start.elapsed().as_secs_f64();
                        monitor.cancel();
                        return (first, at, n, p);
                    }
                }
            });
            let result = shot.session.render(size, &long, &[]);
            let (first, at, n, p) = watcher.join().unwrap();
            (first, at, n, p, result)
        });
    let returned = start.elapsed().as_secs_f64();
    println!(
        "cancel: requested at {cancel_at:.3}s with progress {progress_at_cancel:.3}; render returned at {returned:.3}s ({}); first in-progress picture at {:?}s; {snapshots} pictures before cancel",
        if result.is_err() {
            "cancelled"
        } else {
            "completed"
        },
        first_snapshot
    );
    let t = Instant::now();
    let after = shot.session.render(size, &r, &[1, 2])?;
    println!("render after cancel: {:.3}s, ok", t.elapsed().as_secs_f64());
    receipt["cancel"] = json!({"requested_at_s": cancel_at, "returned_at_s": returned, "seconds_to_stop": returned - cancel_at, "was_cancelled": result.is_err(),
        "first_snapshot_s": first_snapshot, "snapshots_before_cancel": snapshots, "progress_at_cancel": progress_at_cancel, "next_render_s": t.elapsed().as_secs_f64()});

    // 2. Depth and Cryptomatte, alongside the bridge's own coverage masks.
    let t = Instant::now();
    let f = shot.session.render_with(
        size,
        &r,
        Outputs {
            mask_pass_ids: &[1, 2],
            depth: true,
            cryptomatte: true,
            stop: None,
        },
    )?;
    let with_extras = t.elapsed().as_secs_f64();
    let depth = f.depth.as_ref().context("depth")?;
    let centre = (size[1] / 2 * size[0] + size[0] / 2) as usize;
    let finite: Vec<f32> = depth
        .iter()
        .copied()
        .filter(|d| d.is_finite() && *d > 0. && *d < 100.)
        .collect();
    let (dmin, dmax) = finite
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &d| (a.min(d), b.max(d)));
    // The product's Cryptomatte id is whichever id owns the centre pixel.
    let id = f.cryptomatte[0][centre * 4];
    let mut worst = 0f32;
    let mut sum_abs = 0f64;
    let mut decoded = vec![0f32; pixels];
    for p in 0..pixels {
        let mut c = 0f32;
        for layer in &f.cryptomatte {
            for pair in 0..2 {
                if layer[p * 4 + pair * 2] == id {
                    c += layer[p * 4 + pair * 2 + 1];
                }
            }
        }
        decoded[p] = c;
        let d = (c - f.masks[0][p]).abs();
        worst = worst.max(d);
        sum_abs += d as f64;
    }
    write_f32(&out.join("depth.f32"), depth)?;
    write_f32(&out.join("cryptomatte-product.f32"), &decoded)?;
    write_f32(&out.join("mask-product.f32"), &f.masks[0])?;
    println!(
        "depth: {} valid pixels, {dmin:.3}..{dmax:.3} m, centre {:.3} m",
        finite.len(),
        depth[centre]
    );
    println!(
        "cryptomatte vs bridge mask (product): max difference {worst:.5}, mean {:.6}; frame with extras {with_extras:.3}s vs plain {:.3}s",
        sum_abs / pixels as f64,
        after.timing.total_seconds
    );
    receipt["depth"] = json!({"valid_pixels": finite.len(), "min_m": dmin, "max_m": dmax, "centre_m": depth[centre]});
    receipt["cryptomatte_vs_mask"] = json!({"max_abs_difference": worst, "mean_abs_difference": sum_abs / pixels as f64, "frame_seconds_with_depth_and_cryptomatte": with_extras, "frame_seconds_plain": after.timing.total_seconds});
    std::fs::File::create(out.join("receipt.json"))?
        .write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    Ok(())
}

/// Texture maps through the bridge: base colour, a roughness channel, metallic and a normal map,
/// then a material edit that must keep them.
fn textures(args: &[String]) -> Result<()> {
    use nothing_spatial::{
        Image, Material, MaterialMaps, Part, TextureChannel, TextureMap, Vertex,
    };
    let mut s = Session::new(arg(args, "--device").unwrap_or("METAL"), 0)?;
    let mut mesh = Mesh::default();
    // Base colour: left half sRGB 188 (linear 0.50), right half sRGB 99 (linear 0.125). 2x1 image.
    mesh.images.push(Image {
        source: "base".into(),
        width: 2,
        height: 1,
        rgba: vec![188, 188, 188, 255, 99, 99, 99, 255],
    });
    // Data image: R unused, G roughness 1.0, B metallic 0.0, flat tangent normal would be (128,128,255).
    mesh.images.push(Image {
        source: "orm".into(),
        width: 1,
        height: 1,
        rgba: vec![0, 255, 0, 255],
    });
    mesh.images.push(Image {
        source: "normal".into(),
        width: 1,
        height: 1,
        rgba: vec![128, 128, 255, 255],
    });
    let map = |image, channel, srgb| {
        Some(TextureMap {
            image,
            channel,
            srgb,
            scale: [1.; 4],
            bias: [0.; 4],
        })
    };
    mesh.materials.push(Material {
        diffuse: [1., 0., 1.],
        roughness: 0.1,
        metallic: 1.,
        maps: MaterialMaps {
            diffuse: map(0, TextureChannel::Rgb, true),
            roughness: map(1, TextureChannel::G, false),
            metallic: map(1, TextureChannel::B, false),
            normal: map(2, TextureChannel::Rgb, false),
            occlusion: None,
        },
        slot: "/m".into(),
        ..Default::default()
    });
    for (x, z) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        mesh.vertices.push(Vertex {
            position: [x, 0., z],
            normal: [0., 1., 0.],
            material: 0,
            uv: [x + 0.5, z + 0.5],
        });
    }
    mesh.indices = vec![0, 2, 1, 0, 3, 2];
    mesh.parts.push(Part {
        path: "/floor".into(),
        parent: None,
        indices: 0..6,
        frame: [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ],
        pivot: [0.; 3],
    });
    let mats = s.materials(&mesh, &|_| Artwork::None)?;
    s.model(&mesh, &mats, 1)?;
    s.background([0.; 3], false);
    s.camera(
        [0., 2., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., -1., 0.],
        40.,
    );
    s.distant_light([0., -1., 0.], [3., 3., 3.], 0.);
    let r = Recipe {
        samples: 256,
        max_bounces: 1,
        clamp_indirect: 0.,
        denoise: false,
        seed: 0,
    };
    let size = [64u32, 64u32];
    let px = |f: &Frame, x: usize| f.color[(32 * 64 + x) * 4];
    let f = s.render(size, &r, &[])?;
    // Columns 21 and 43 look at the two texel centres. If the maps are honoured the surface is rough
    // and non-metallic: radiance = albedo * 3 / pi, plus a little from the specular layer.
    println!(
        "textured floor: left {:.4} (expect about {:.4}), right {:.4} (expect about {:.4})",
        px(&f, 21),
        0.5 * 3. / std::f32::consts::PI,
        px(&f, 43),
        0.125 * 3. / std::f32::consts::PI
    );
    println!(
        "  without the maps this material would be a magenta mirror: constant diffuse is (1, 0, 1), metallic 1, roughness 0.1; green channel left {:.4}",
        f.color[(32 * 64 + 21) * 4 + 1]
    );
    let mut edited = mesh.materials[0].clone();
    edited.clearcoat = 0.0;
    edited.roughness = 0.9;
    s.update_material(mats[0], &edited, Artwork::None);
    let g = s.render(size, &r, &[])?;
    println!(
        "after a material edit: left {:.4}, right {:.4} (maps kept if unchanged)",
        px(&g, 21),
        px(&g, 43)
    );
    Ok(())
}

/// Print what the importer returns, for writing the shot file.
fn inspect(args: &[String]) -> Result<()> {
    let mesh = nothing_openusd::load(Path::new(&args[0]), &args[1])?;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &mesh.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v.position[k]);
            hi[k] = hi[k].max(v.position[k]);
        }
    }
    println!(
        "{} triangles, {} parts, bounds {:?} .. {:?}",
        mesh.indices.len() / 3,
        mesh.parts.len(),
        lo,
        hi
    );
    for m in &mesh.materials {
        println!(
            "slot {:40} diffuse {:?} metal {} rough {} coat {} emissive {:?} maps d{} n{} r{} m{}",
            m.slot,
            m.diffuse,
            m.metallic,
            m.roughness,
            m.clearcoat,
            m.emissive,
            m.maps.diffuse.is_some() as u8,
            m.maps.normal.is_some() as u8,
            m.maps.roughness.is_some() as u8,
            m.maps.metallic.is_some() as u8
        );
    }
    for (i, p) in mesh.parts.iter().enumerate() {
        if args.get(2).is_some_and(|f| p.path.contains(f.as_str()))
            || p.parent.is_none()
            || mesh.parts.len() < 12
        {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &ix in &mesh.indices[p.indices.start as usize..p.indices.end as usize] {
                let v = &mesh.vertices[ix as usize];
                for k in 0..3 {
                    lo[k] = lo[k].min(v.position[k]);
                    hi[k] = hi[k].max(v.position[k]);
                }
            }
            let mats: std::collections::BTreeSet<u32> = mesh.indices
                [p.indices.start as usize..p.indices.end as usize]
                .iter()
                .map(|&ix| mesh.vertices[ix as usize].material)
                .collect();
            println!(
                "part {i} {} parent {:?} tris {} pivot {:?} bounds {:?}..{:?} mats {:?}",
                p.path,
                p.parent,
                (p.indices.end - p.indices.start) / 3,
                p.pivot,
                lo,
                hi,
                mats
            );
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        !args.is_empty(),
        "usage: spike <calib|render|edits|inspect> ..."
    );
    match args[0].as_str() {
        "calib" => calib(&args[1..]),
        "render" => render(&args[1..]),
        "edits" => edits(&args[1..]),
        "masks" => masks(&args[1..]),
        "features" => features(&args[1..]),
        "textures" => textures(&args[1..]),
        "inspect" => inspect(&args[1..]),
        "draft" => draft::run(&args[1..]),
        other => bail!("unknown command {other}"),
    }
}

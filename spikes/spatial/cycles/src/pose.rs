//! Rigid transforms as row-major 3x4 matrices, in the conventions of `nothing-spatial-wgpu`:
//! Euler angles in degrees compose as Rz * Ry * Rx, and a part rotates in its own frame about its pivot.
use nothing_spatial::Mesh;

pub type M34 = [f32; 12];
pub const IDENTITY: M34 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0.];

pub fn mul(a: &M34, b: &M34) -> M34 {
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

/// Columns of Rz * Ry * Rx.
pub fn axes(degrees: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = degrees[0].to_radians().sin_cos();
    let (sy, cy) = degrees[1].to_radians().sin_cos();
    let (sz, cz) = degrees[2].to_radians().sin_cos();
    [
        [cz * cy, sz * cy, -sy],
        [cz * sy * sx - sz * cx, sz * sy * sx + cz * cx, cy * sx],
        [cz * sy * cx + sz * sx, sz * sy * cx - cz * sx, cy * cx],
    ]
}

pub fn rigid(position: [f32; 3], degrees: [f32; 3]) -> M34 {
    let [x, y, z] = axes(degrees);
    [
        x[0],
        y[0],
        z[0],
        position[0],
        x[1],
        y[1],
        z[1],
        position[1],
        x[2],
        y[2],
        z[2],
        position[2],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    a.map(|v| v / l)
}

/// A rotation in the part's own orthonormal frame about its pivot, in asset space.
pub fn part_rotation(frame: &[[f32; 4]; 4], pivot: [f32; 3], degrees: [f32; 3]) -> M34 {
    let x = unit([frame[0][0], frame[0][1], frame[0][2]]);
    let y = unit([frame[1][0], frame[1][1], frame[1][2]]);
    let z = unit([frame[2][0], frame[2][1], frame[2][2]]);
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
    mul(&mul(&f, &rigid([0.; 3], degrees)), &inv)
}

/// Object-to-world transform of every part: the instance placement, then each ancestor's
/// override. `overrides` pairs a part path with its extra rotation in degrees.
pub fn part_transforms(mesh: &Mesh, instance: &M34, overrides: &[(&str, [f32; 3])]) -> Vec<M34> {
    let mut world: Vec<M34> = Vec::with_capacity(mesh.parts.len());
    for part in &mesh.parts {
        let parent = part.parent.map_or(*instance, |p| world[p]);
        world.push(
            match overrides.iter().find(|(path, _)| *path == part.path) {
                Some((_, degrees)) => {
                    mul(&parent, &part_rotation(&part.frame, part.pivot, *degrees))
                }
                None => parent,
            },
        );
    }
    world
}

// A gently rippling rounded silhouette. Coordinates come from the mask raster
// so the same recipe follows the card at different positions and sizes.
fn coverage_amount(local: vec2<f32>, phase: f32, alpha: f32) -> f32 {
    let size = mask_size();
    let p = mask_pixel(local) - 0.5 * size;
    let turn = phase * 6.2831853;
    let wave = sin(turn + p.x / size.x * 6.2831853);
    let drift = vec2<f32>(0.013 * size.x * cos(turn), 0.056 * size.y * wave);
    let half_box = size * vec2<f32>(0.468, 0.410 + 0.026 * cos(turn));
    let radius = min(size.x, size.y) * 0.215;
    let q = abs(p - drift) - half_box + vec2<f32>(radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    return alpha * (1.0 - smoothstep(-0.6, 0.6, distance));
}

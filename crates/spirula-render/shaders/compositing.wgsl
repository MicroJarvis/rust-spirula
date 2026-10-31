// spirula-shader: compositing version: 1
// Bind layout v1: uniform Params(16B) + storage read + storage read_write.
// Input: header16, projected records 16*N, sorted pairs 4*P, tile ranges 2*T.
// Projected: xy, unused, ray_z, unused×2, cov×3, opacity, rgb×3, pad×3.
// Output: 8 u32/pixel = rgb×3, T, expected, median, pad×2 (f32 bitcasts).
// Matches RasterizationEval3DFwd_kernel.cuh:145-322 (non-distortion).

struct Params { count: u32, sources: u32, pairs: u32, tiles: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn poff() -> u32 { return 16u; }
fn soff() -> u32 { return poff() + 16u * p.sources; }
fn roff() -> u32 { return soff() + 4u * p.pairs; }

fn finite(v: f32) -> bool { return abs(v) <= 3.402823466e38; }

@compute @workgroup_size(64)
fn composite(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let w = src[0];
    let h = src[1];
    if i >= w * h { return; }
    let ts = src[2];
    let tx = src[3];
    let cutoff = bitcast<f32>(src[4]);
    let early = bitcast<f32>(src[5]);
    let bg = vec3<f32>(
        bitcast<f32>(src[9]),
        bitcast<f32>(src[10]),
        bitcast<f32>(src[11]),
    );
    let x = i % w;
    let y = i / w;
    let tile = (y / ts) * tx + x / ts;
    let start = src[roff() + 2u * tile];
    let count = src[roff() + 2u * tile + 1u];

    var T = 1.0;
    var rgb = vec3<f32>(0.0);
    var depth_acc = 0.0;
    var median = 0.0;
    // Virtual previous (z=0, T=1).
    var median_prev_z = 0.0;
    var median_prev_T = 1.0;
    var done = false;
    var status = 0u;

    for (var j = 0u; j < count; j++) {
        if done { break; }
        let source = src[soff() + 4u * (start + j) + 2u];
        let at = poff() + source * 16u;
        let mean = vec2<f32>(bitcast<f32>(src[at]), bitcast<f32>(src[at + 1u]));
        let ray_z = bitcast<f32>(src[at + 3u]);
        let xx = bitcast<f32>(src[at + 6u]);
        let xy = bitcast<f32>(src[at + 7u]);
        let yy = bitcast<f32>(src[at + 8u]);
        let opacity = bitcast<f32>(src[at + 9u]);
        let color = vec3<f32>(
            bitcast<f32>(src[at + 10u]),
            bitcast<f32>(src[at + 11u]),
            bitcast<f32>(src[at + 12u]),
        );
        let det = xx * yy - xy * xy;
        let delta = mean - vec2<f32>(f32(x) + 0.5, f32(y) + 0.5);
        let sigma = 0.5 * ((yy / det) * delta.x * delta.x + (xx / det) * delta.y * delta.y)
            - (xy / det) * delta.x * delta.y;
        if !finite(det) || det <= 0.0 || !finite(sigma) || !finite(ray_z) || ray_z <= 0.0 {
            status = 1u;
            continue;
        }
        if sigma < 0.0 { continue; }
        let alpha = min(0.999, opacity * exp(-sigma));
        if !finite(alpha) {
            status = 1u;
            continue;
        }
        // alpha <= cutoff excluded.
        if alpha <= cutoff { continue; }

        let next_T = T * (1.0 - alpha);
        if !finite(next_T) {
            status = 1u;
            continue;
        }

        // Median before early-out; strict next_T < 0.5.
        if median_prev_T >= 0.5 && next_T < 0.5 {
            let a = log(median_prev_T);
            let b = log(next_T);
            let c = -0.6931471805599453; // ln(1/2)
            let d = b - a;
            var f = 0.0;
            if abs(d) > 1e-20 { f = (c - a) / d; }
            median = median_prev_z + (ray_z - median_prev_z) * f;
        }
        median_prev_T = next_T;
        median_prev_z = ray_z;

        // Accumulate only if next_T > early_out; else done without this splat.
        if next_T > early {
            let vis = alpha * T;
            rgb += color * vis;
            depth_acc += ray_z * vis;
            T = next_T;
        } else {
            done = true;
        }
    }

    let weight = 1.0 - T;
    // Unconditional C++ :315-322 — no weight==0 clear shortcut.
    let expected = depth_acc / max(weight, 1e-10);
    let out_rgb = rgb + T * bg;
    if !finite(out_rgb.x) || !finite(out_rgb.y) || !finite(out_rgb.z)
        || !finite(T) || !finite(expected) || !finite(median)
        || T < 0.0 || T > 1.0 || expected < 0.0 || median < 0.0
    {
        status = 1u;
    }
    let base = 8u * i;
    // Always write every pixel (including clears / uncovered). Host decode
    // rejects status != 0; success never relies on zero-init buffers.
    dst[base] = bitcast<u32>(out_rgb.x);
    dst[base + 1u] = bitcast<u32>(out_rgb.y);
    dst[base + 2u] = bitcast<u32>(out_rgb.z);
    dst[base + 3u] = bitcast<u32>(T);
    dst[base + 4u] = bitcast<u32>(expected);
    dst[base + 5u] = bitcast<u32>(median);
    dst[base + 6u] = status;
    dst[base + 7u] = 0u;
}

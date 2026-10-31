// spirula-shader: sort-raster version: 1
// Bind layout v1: uniform Params(16B) + storage read + storage read_write.
// State layout: header8, key records (tile,depth,source,input)*P,
// histogram 16*groups, tile ranges 2*T, projected records 16*N.
// Sixteen LSD 4-bit rounds: histogram → exclusive prefix → stable scatter.
// Raster writes coverage only (count/first_source/max_alpha); no transmittance.

struct Params { count: u32, sources: u32, shift: u32, tiles: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

var<workgroup> digits: array<u32, 64>;
var<workgroup> hist: array<atomic<u32>, 16>;

fn groups() -> u32 { return (p.count + 63u) / 64u; }
fn hoff() -> u32 { return 8u + 4u * p.count; }
fn roff() -> u32 { return hoff() + 16u * groups(); }
fn poff() -> u32 { return roff() + 2u * p.tiles; }

fn digit(i: u32) -> u32 {
    let off = 8u + 4u * i;
    if p.shift < 32u {
        return (src[off + 1u] >> p.shift) & 15u;
    }
    return (src[off] >> (p.shift - 32u)) & 15u;
}

fn copy_constants(i: u32) {
    if i < 8u { dst[i] = src[i]; }
    if i < p.sources {
        for (var k = 0u; k < 16u; k++) {
            dst[poff() + 16u * i + k] = src[poff() + 16u * i + k];
        }
    }
}

fn copy_key(i: u32) {
    if i < p.count {
        for (var k = 0u; k < 4u; k++) {
            dst[8u + 4u * i + k] = src[8u + 4u * i + k];
        }
    }
}

@compute @workgroup_size(64)
fn keys(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i < 8u { dst[i] = src[i]; }
    if i < p.sources {
        for (var k = 0u; k < 16u; k++) {
            dst[poff() + 16u * i + k] = src[8u + 16u * i + k];
        }
    }
    if i < p.count {
        let base = 8u + 16u * p.sources + 2u * i;
        let source = src[base + 1u];
        dst[8u + 4u * i] = src[base];
        // Positive axial depth bits XOR sign bit (IntersectTile.cu:228-235).
        dst[9u + 4u * i] = src[8u + 16u * source + 2u] ^ 0x80000000u;
        dst[10u + 4u * i] = source;
        dst[11u + 4u * i] = i;
    }
}

@compute @workgroup_size(64)
fn histogram(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) bid: vec3<u32>,
) {
    let i = gid.x;
    copy_constants(i);
    copy_key(i);
    if lid < 16u { atomicStore(&hist[lid], 0u); }
    workgroupBarrier();
    if i < p.count { atomicAdd(&hist[digit(i)], 1u); }
    workgroupBarrier();
    if bid.x < groups() && lid < 16u {
        dst[hoff() + lid * groups() + bid.x] = atomicLoad(&hist[lid]);
    }
}

@compute @workgroup_size(64)
fn prefix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    copy_constants(i);
    copy_key(i);
    if i < 16u {
        var total = 0u;
        for (var d = 0u; d < i; d++) {
            for (var b = 0u; b < groups(); b++) {
                total += src[hoff() + d * groups() + b];
            }
        }
        for (var b = 0u; b < groups(); b++) {
            let at = hoff() + i * groups() + b;
            dst[at] = total;
            total += src[at];
        }
    }
}

@compute @workgroup_size(64)
fn scatter(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) bid: vec3<u32>,
) {
    let i = gid.x;
    copy_constants(i);
    var d = 16u;
    if i < p.count { d = digit(i); }
    digits[lid] = d;
    workgroupBarrier();
    if i < p.count {
        var rank = 0u;
        for (var j = 0u; j < lid; j++) {
            if digits[j] == d { rank++; }
        }
        let at = src[hoff() + d * groups() + bid.x] + rank;
        for (var k = 0u; k < 4u; k++) {
            dst[8u + 4u * at + k] = src[8u + 4u * i + k];
        }
    }
}

fn lower(tile: u32) -> u32 {
    var lo = 0u;
    var hi = p.count;
    while lo < hi {
        let m = lo + (hi - lo) / 2u;
        if src[8u + 4u * m] < tile { lo = m + 1u; } else { hi = m; }
    }
    return lo;
}

@compute @workgroup_size(64)
fn ranges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    copy_constants(i);
    copy_key(i);
    if i < p.tiles {
        let start = lower(i);
        let end = lower(i + 1u);
        dst[roff() + 2u * i] = start;
        dst[roff() + 2u * i + 1u] = end - start;
    }
}

fn finite(v: f32) -> bool { return abs(v) <= 3.402823466e38; }

@compute @workgroup_size(64)
fn raster(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let w = src[0];
    let h = src[1];
    if i >= w * h { return; }
    let ts = src[2];
    let tx = src[3];
    let cutoff = bitcast<f32>(src[4]);
    let x = i % w;
    let y = i / w;
    let tile = (y / ts) * tx + x / ts;
    let start = src[roff() + 2u * tile];
    let count = src[roff() + 2u * tile + 1u];
    var hits = 0u;
    var first = 0xffffffffu;
    var maximum = 0.0;
    var status = 0u;
    for (var j = 0u; j < count; j++) {
        let source = src[8u + 4u * (start + j) + 2u];
        let at = poff() + source * 16u;
        let mean = vec2<f32>(bitcast<f32>(src[at]), bitcast<f32>(src[at + 1u]));
        let xx = bitcast<f32>(src[at + 6u]);
        let xy = bitcast<f32>(src[at + 7u]);
        let yy = bitcast<f32>(src[at + 8u]);
        let opacity = bitcast<f32>(src[at + 9u]);
        let det = xx * yy - xy * xy;
        let delta = mean - vec2<f32>(f32(x) + 0.5, f32(y) + 0.5);
        let sigma = 0.5 * ((yy / det) * delta.x * delta.x + (xx / det) * delta.y * delta.y)
            - (xy / det) * delta.x * delta.y;
        if !finite(det) || det <= 0.0 || !finite(sigma) {
            status = 1u;
            continue;
        }
        if sigma < 0.0 { continue; }
        let alpha = min(0.999, opacity * exp(-sigma));
        if !finite(alpha) {
            status = 1u;
            continue;
        }
        // alpha <= cutoff excluded (RasterizationEval3DFwd_kernel.cuh:247).
        if alpha > cutoff {
            if hits == 0u { first = source; }
            hits++;
            maximum = max(maximum, alpha);
        }
    }
    dst[4u * i] = hits;
    dst[4u * i + 1u] = first;
    dst[4u * i + 2u] = bitcast<u32>(maximum);
    dst[4u * i + 3u] = status;
}

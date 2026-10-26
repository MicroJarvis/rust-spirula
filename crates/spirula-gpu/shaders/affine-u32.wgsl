// spirula-shader: affine-u32 version: 1
// Bind layout v1: group0 binding0 uniform Params (16B), binding1 storage read,
// binding2 storage read_write. Entry: main. Override MODE is in the pipeline key.

struct Params {
    scale: u32,
    bias: u32,
    count: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> input_buf: array<u32>;
@group(0) @binding(2) var<storage, read_write> output_buf: array<u32>;

// Specialization / override: added to every written element (key field).
override MODE: u32 = 0;

fn affine_body(gid: vec3<u32>) {
    let i = gid.x;
    if i >= params.count {
        return;
    }
    output_buf[i] = input_buf[i] * params.scale + params.bias + MODE;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    affine_body(gid);
}

// Second entry for pipeline-key entry_point coverage (same body).
@compute @workgroup_size(64)
fn main_alt(@builtin(global_invocation_id) gid: vec3<u32>) {
    affine_body(gid);
}

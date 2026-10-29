//! Host-only regenerator for `fixtures/datasets/colmap-goldens/bake-pin/`.
//!
//! No C++ sources. Invoked via:
//! `cargo run -p spirula-io --example regen_bake_pin -- <bake-pin-dir>`

use std::env;
use std::fs;
use std::path::PathBuf;

use spirula_core::{FrameCamera, Vec3, source_project};
use spirula_io::colmap::{COLMAP_MODELS, ColmapCamera, bake_and_fit, colmap_to_c2w};

const W: u32 = 640;
const H: u32 = 480;
const RAYS: [(f64, f64, f64); 3] = [(0.0, 0.0, 1.0), (0.2, -0.1, 1.0), (-0.15, 0.25, 1.0)];

fn main() {
    let dir = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "fixtures/datasets/colmap-goldens/bake-pin".into()),
    );
    let inputs: Vec<Vec<f64>> =
        serde_json::from_str(&fs::read_to_string(dir.join("inputs.json")).expect("inputs.json"))
            .expect("inputs json");
    assert_eq!(inputs.len(), COLMAP_MODELS.len());

    let mut out = String::new();
    for (id, params) in inputs.iter().enumerate() {
        let info = &COLMAP_MODELS[id];
        let cam = ColmapCamera {
            camera_id: id as i32,
            model: info.name.into(),
            width: u64::from(W),
            height: u64::from(H),
            params: params.clone(),
        };
        let bi = bake_and_fit(&cam).unwrap_or_else(|e| panic!("{}: {e}", info.name));
        out.push_str(&format!(
            "{id} {} {} {} {} {} {} {} {}",
            bi.model.as_i32(),
            bi.distortion.as_i32(),
            bi.fx,
            bi.fy,
            bi.cx,
            bi.cy,
            bi.source.source_model,
            bi.source.fit_max_px,
        ));
        for d in &bi.dist {
            out.push_str(&format!(" {d}"));
        }
        out.push('\n');

        // Unfitted-ray pixels: source_project when active, else baked FrameCamera.
        out.push_str(&format!("ray {id}"));
        let coeffs = bi.dist.map(f64::from);
        let frame = FrameCamera::try_new(
            bi.model,
            bi.distortion,
            W,
            H,
            f64::from(bi.fx),
            f64::from(bi.fy),
            f64::from(bi.cx),
            f64::from(bi.cy),
            coeffs,
        )
        .unwrap_or_else(|e| panic!("{} frame: {e}", info.name));
        for &(x, y, z) in &RAYS {
            let (px, py) = if bi.source.source_model >= 0 {
                source_project(bi.source.source_model, &bi.source.params, x, y, z)
                    .unwrap_or_else(|e| panic!("{} source ray: {e}", info.name))
            } else {
                let ray = Vec3::try_new(x, y, z).expect("ray");
                let p = frame
                    .ray_to_pixel(ray)
                    .unwrap_or_else(|e| panic!("{} baked ray: {e}", info.name));
                (p.x(), p.y())
            };
            out.push_str(&format!(" {px} {py}"));
        }
        out.push('\n');
    }

    // Pin pose input from ColmapParser golden extract (host-only numbers).
    let q = [0.923_879_532_511_286_7, 0.0, 0.382_683_432_365_089_8, 0.0];
    let t = [1.0, 2.0, 3.0];
    let c2w = colmap_to_c2w(q, t).expect("pose");
    out.push_str("pose");
    for v in c2w {
        out.push_str(&format!(" {v}"));
    }
    out.push('\n');

    fs::write(dir.join("expected.txt"), &out).expect("write expected.txt");
    fs::write(
        dir.join("provenance.txt"),
        "pin=9a24f09927944a795e7ba1bd8243a625bf419479\n\
         host-only regen: cargo run -p spirula-io --example regen_bake_pin -- \\\n\
           fixtures/datasets/colmap-goldens/bake-pin\n\
         Rows: id model tier fx fy cx cy source fit_max_px d[8];\n\
         ray id px0 py0 px1 py1 px2 py2 (unfitted source_project or exact bake);\n\
         pose from qvec=[cos(π/8),0,sin(π/8),0] tvec=[1,2,3] via colmap_to_c2w.\n\
         No C++ sources checked in.\n",
    )
    .expect("provenance");
    println!("wrote {}", dir.join("expected.txt").display());
}

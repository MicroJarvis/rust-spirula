//! Independent C++ output, rather than CPU/GPU agreement alone.

use spirula_core::{
    CameraPose, GaussianSplat, Mat3, OpenCvDistortion, PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_gpu::{CapabilityRequest, GpuContext};
use spirula_render::{
    ColorSpace, CpuTileBuilder, GpuTileBuilder, RenderPrimitive, RenderRequest, RenderThresholds,
    TileBins, TileBuildOptions,
};

fn samples() -> Vec<Vec<f64>> {
    include_str!("data/p4-f03-reference.tsv")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let values: Vec<f64> = line
                .split_whitespace()
                .map(|value| value.parse().unwrap())
                .collect();
            assert_eq!(values.len(), 27);
            values
        })
        .collect()
}

fn request(rows: &[Vec<f64>]) -> RenderRequest {
    let pose = CameraPose::try_new(
        Mat3::try_from_row_major([1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0]).unwrap(),
        Vec3::try_new(0.0, 0.0, -4.0).unwrap(),
    )
    .unwrap();
    let camera = PinholeCamera::try_new(
        800,
        600,
        600.0,
        610.0,
        400.0,
        300.0,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        pose,
    )
    .unwrap();
    let splats = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            assert_eq!(row[0], index as f64);
            GaussianSplat::try_new(
                Vec3::try_new(row[1], row[2], row[3]).unwrap(),
                Quat::try_from_wxyz(row[4], row[5], row[6], row[7]).unwrap(),
                Vec3::try_new(row[8], row[9], row[10]).unwrap(),
                row[11],
                Vec3::try_new(row[12], row[13], row[14]).unwrap(),
            )
            .unwrap()
        })
        .collect();
    RenderRequest::try_new(
        camera,
        SplatSet::try_from_splats(splats).unwrap(),
        RenderPrimitive::Gaussian3d,
        ColorSpace::LinearRgb,
        0,
        RenderThresholds {
            alpha_cutoff: 1.0 / 255.0,
            ..RenderThresholds::default()
        },
    )
    .unwrap()
}

fn close(source: usize, field: &str, actual: f64, reference: f64) {
    // Reference projection_parity uses this f32 fast-math tolerance.
    let tolerance = 5e-3 + 5e-4 * reference.abs();
    assert!(
        actual.is_finite() && (actual - reference).abs() <= tolerance,
        "source={source} field={field} actual={actual} reference={reference} tolerance={tolerance}"
    );
}

fn compare_reference(bins: &TileBins, rows: &[Vec<f64>]) {
    bins.validate().unwrap();
    assert_eq!(bins.projected.len(), rows.len());
    let mut visible = 0;
    let mut culled = 0;
    for (index, row) in rows.iter().enumerate() {
        if row[17] <= row[15] || row[18] <= row[16] {
            assert!(bins.projected[index].is_none(), "source={index}");
            culled += 1;
            continue;
        }
        let projected = bins.projected[index].expect("reference-visible source must survive");
        close(index, "sorting_depth", projected.depth, row[19]);
        close(index, "pixel_x", projected.pixel[0], row[20]);
        close(index, "pixel_y", projected.pixel[1], row[21]);
        close(index, "ray_depth", projected.ray_depth, row[22]);
        close(index, "opacity", projected.opacity, row[26]);
        let [xx, xy, yy] = projected.covariance;
        let determinant = xx * yy - xy * xy;
        close(index, "conic_xx", yy / determinant, row[23]);
        close(index, "conic_xy", -xy / determinant, row[24]);
        close(index, "conic_yy", xx / determinant, row[25]);
        // Quantized reference bounds enclose the original floating bounds.
        // A full-precision AABB can differ by at most one quantization step.
        for (field, actual, quantized, extent) in [
            (
                "xmin",
                projected.pixel[0] - projected.radius_px[0],
                row[15],
                800.0,
            ),
            (
                "ymin",
                projected.pixel[1] - projected.radius_px[1],
                row[16],
                600.0,
            ),
            (
                "xmax",
                projected.pixel[0] + projected.radius_px[0],
                row[17],
                800.0,
            ),
            (
                "ymax",
                projected.pixel[1] + projected.radius_px[1],
                row[18],
                600.0,
            ),
        ] {
            let decoded = quantized * extent / 65535.0;
            let clamped = actual.clamp(0.0, extent);
            assert!(
                (clamped - decoded).abs() <= extent / 65535.0 + 2e-3,
                "source={index} field={field} actual={clamped} quantized={decoded}"
            );
        }
        visible += 1;
    }
    assert!(
        visible >= 16 && culled >= 4,
        "fixture must exercise both branches"
    );
}

#[test]
fn cpu_covariance_projection_matches_recorded_cpp_outputs() {
    let rows = samples();
    let bins = CpuTileBuilder::build(&request(&rows), TileBuildOptions::default()).unwrap();
    compare_reference(&bins, &rows);
}

#[test]
fn gpu_covariance_projection_matches_recorded_cpp_outputs() {
    let rows = samples();
    let mut context = match GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(context) => context,
        Err(failure) => {
            eprintln!("reference_gpu_unavailable: {failure:?}");
            assert!(
                std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                "the required real GPU reference gate is unavailable"
            );
            return;
        }
    };
    let baseline_bytes = context.resource_accounting().current_bytes;
    let (bins, metrics) =
        GpuTileBuilder::build(&mut context, &request(&rows), TileBuildOptions::default()).unwrap();
    compare_reference(&bins, &rows);
    let cpu = CpuTileBuilder::build(&request(&rows), TileBuildOptions::default()).unwrap();
    assert_eq!(bins.inclusive_offsets, cpu.inclusive_offsets);
    assert_eq!(bins.coverage, cpu.coverage);
    assert_eq!(bins.pairs, cpu.pairs);
    assert_eq!(metrics.intersection_peak, bins.pairs.len() as u64);
    assert_eq!(context.resource_accounting().current_bytes, baseline_bytes);
    assert!(metrics.scan_scratch_bytes > 0);
    println!(
        "reference_parity adapter={} sources={} pairs={} scratch={} peak_bound={} encode_ns={} count_sync_ns={} emission_sync_ns={}",
        metrics.adapter,
        rows.len(),
        bins.pairs.len(),
        metrics.scan_scratch_bytes,
        metrics.accounted_peak_bound_bytes,
        metrics.projection_scan_encode_ns,
        metrics.count_sync_ns,
        metrics.emission_sync_ns,
    );
}

#[test]
fn gpu_axis_depths_do_not_turn_roundoff_into_invalid_geometry() {
    let mut context = match GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(context) => context,
        Err(failure) => {
            eprintln!("axis_gpu_unavailable: {failure:?}");
            assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
            return;
        }
    };
    let depths = [0.0001, 0.001, 0.01, 0.1, 0.2, 0.3, 0.7, 1.001, 2.3, 100.001];
    let mut request = request(&samples());
    request.camera = PinholeCamera::try_new(
        64,
        48,
        32.0,
        32.0,
        32.0,
        24.0,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    request.scene = SplatSet::try_from_splats(
        depths
            .iter()
            .map(|depth| {
                GaussianSplat::try_new(
                    Vec3::try_new(0.0, 0.0, -*depth).unwrap(),
                    Quat::identity(),
                    Vec3::try_new(-6.0, -6.0, -6.0).unwrap(),
                    0.0,
                    Vec3::try_new(0.0, 0.0, 0.0).unwrap(),
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap();
    let (bins, _) =
        GpuTileBuilder::build(&mut context, &request, TileBuildOptions::default()).unwrap();
    assert!(bins.projected.iter().all(Option::is_some));
    for (index, projected) in bins.projected.iter().enumerate() {
        let projected = projected.unwrap();
        close(index, "axis_depth", projected.depth, depths[index]);
        close(index, "axis_ray_depth", projected.ray_depth, depths[index]);
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
}

//! Independent closed-form goldens from the pinned C++ compositing equations.
//! Expected pixels come from the checked-in TSV, never from the CPU renderer.
use spirula_core::{
    CameraPose, GaussianSplat, OpenCvDistortion, PinholeCamera, Quat, SplatSet, Vec3,
};
use spirula_render::{
    BackgroundPolicy, ColorSpace, CompositeImage, CompositeOptions, CpuCompositor, GpuCompositor,
    RenderPrimitive, RenderRequest, RenderThresholds,
};

type Layer = (f64, [f64; 3], f64);

fn request(width: u32, cx: f64, layers: &[Layer], cutoff: f64, early_out: f64) -> RenderRequest {
    let camera = PinholeCamera::try_new(
        width,
        1,
        1.0,
        1.0,
        cx,
        0.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let splats = layers
        .iter()
        .map(|&(depth, dc, logit)| {
            // cx=0 case places the mean on (.5,.5) while making ray depth
            // sqrt(5), distinct from the sorting axial depth of 2.
            let x = if cx == 0.0 { 0.5 * depth } else { 0.0 };
            GaussianSplat::try_new(
                Vec3::try_new(x, 0.0, -depth).unwrap(),
                Quat::try_from_wxyz(1.0, 0.0, 0.0, 0.0).unwrap(),
                Vec3::try_new(-30.0, -30.0, -30.0).unwrap(),
                logit,
                Vec3::try_new(dc[0], dc[1], dc[2]).unwrap(),
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
            alpha_cutoff: cutoff,
            early_out_transmittance: early_out,
        },
    )
    .unwrap()
}

fn golden(name: &str) -> Vec<f64> {
    include_str!("data/p4-f05-review.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == name).then(|| fields.map(|v| v.parse().unwrap()).collect())
        })
        .unwrap_or_else(|| panic!("missing independent golden {name}"))
}

fn check_pixel(image: &CompositeImage, pixel: usize, name: &str, tolerance: f64) {
    let actual = image.pixels[pixel];
    let expected = golden(name);
    assert_eq!(expected.len(), 7);
    for (field, (value, target)) in actual
        .rgb
        .into_iter()
        .chain([
            actual.transmittance,
            actual.alpha,
            actual.expected_depth,
            actual.median_depth,
        ])
        .zip(expected)
        .enumerate()
    {
        // A blanket absolute tolerance would incorrectly accept zero for the
        // tiny but nonzero color/depth in rounding_weight_zero.
        let allowed = if target != 0.0 && target.abs() < 1e-12 {
            tolerance * target.abs()
        } else {
            tolerance
        };
        assert!(
            value.is_finite() && (value - target).abs() <= allowed,
            "{name} pixel={pixel} field={field} actual={value:.17e} golden={target:.17e} tolerance={allowed}"
        );
    }
    assert!((actual.alpha - (1.0 - actual.transmittance)).abs() <= tolerance);
}

fn cases() -> Vec<(&'static str, RenderRequest, CompositeOptions)> {
    let clear = CompositeOptions::default();
    let background = CompositeOptions {
        background: BackgroundPolicy::Constant([0.2, 0.4, 0.6]),
        ..clear
    };
    let red = [2.0, -2.0, -2.0];
    let green = [-2.0, 2.0, -2.0];
    let blue = [-2.0, -2.0, 2.0];
    vec![
        (
            "single_half",
            request(1, 0.5, &[(2.0, [0.0; 3], 0.0)], 1e-4, 1e-4),
            background,
        ),
        (
            "red_green",
            request(1, 0.5, &[(2.0, red, 0.0), (4.0, green, 0.0)], 1e-4, 1e-4),
            background,
        ),
        // Reverse scene input; actual axial-depth sorting must recover the same image.
        (
            "red_green",
            request(1, 0.5, &[(4.0, green, 0.0), (2.0, red, 0.0)], 1e-4, 1e-4),
            background,
        ),
        (
            "same_depth",
            request(1, 0.5, &[(2.0, red, 0.0), (2.0, green, 0.0)], 1e-4, 1e-4),
            clear,
        ),
        (
            "single_three_quarter",
            request(1, 0.5, &[(4.0, [0.0; 3], 3.0_f64.ln())], 1e-4, 1e-4),
            clear,
        ),
        (
            "median_before_earlyout",
            request(1, 0.5, &[(4.0, [0.0; 3], 3.0_f64.ln())], 1e-4, 0.5),
            clear,
        ),
        (
            "early_out_clamp",
            request(
                1,
                0.5,
                &[(1.0, red, 20.0), (2.0, green, 20.0), (3.0, blue, 20.0)],
                1e-4,
                1e-4,
            ),
            clear,
        ),
        (
            "quarter_equality",
            request(1, 0.5, &[(2.0, red, 0.0), (4.0, green, 0.0)], 1e-4, 0.25),
            clear,
        ),
        (
            "off_axis",
            request(1, 0.0, &[(2.0, [0.0; 3], 0.0)], 1e-4, 1e-4),
            clear,
        ),
        (
            "rounding_weight_zero",
            request(1, 0.5, &[(2.0, [0.0; 3], 1e-30_f64.ln())], 1e-35, 1e-4),
            clear,
        ),
    ]
}

#[test]
fn cpu_matches_independent_layer_depth_and_earlyout_goldens() {
    for (name, request, options) in cases() {
        let image = CpuCompositor::compose(&request, options).unwrap();
        assert_eq!((image.width, image.height, image.pixels.len()), (1, 1, 1));
        check_pixel(&image, 0, name, 5e-13);
    }
}

#[test]
fn cpu_small_image_clears_holes_and_clamps_only_lower_sh0_bound() {
    let request = request(3, 1.5, &[(2.0, [-2.0, 0.0, 2.0], 0.0)], 0.1, 1e-4);
    let image = CpuCompositor::compose(
        &request,
        CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            ..CompositeOptions::default()
        },
    )
    .unwrap();
    for (pixel, name) in ["image_left", "image_center", "image_right"]
        .into_iter()
        .enumerate()
    {
        check_pixel(&image, pixel, name, 5e-13);
    }
    let cleared = CpuCompositor::compose_empty(
        17,
        9,
        0.1,
        0.25,
        CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            ..CompositeOptions::default()
        },
    )
    .unwrap();
    assert_eq!(cleared.alpha_cutoff, 0.1);
    assert_eq!(cleared.early_out_transmittance, 0.25);
    for pixel in 0..153 {
        check_pixel(&cleared, pixel, "image_left", 5e-13);
    }
}

#[test]
fn invalid_tile_geometry_is_a_typed_error_before_arithmetic() {
    let request = request(1, 0.5, &[(2.0, [0.0; 3], 0.0)], 1e-4, 1e-4);
    for tile_size in [0, 3, 256] {
        let options = CompositeOptions {
            tile: spirula_render::TileBuildOptions {
                tile_size,
                ..spirula_render::TileBuildOptions::default()
            },
            ..CompositeOptions::default()
        };
        let outcome = std::panic::catch_unwind(|| CpuCompositor::compose(&request, options));
        let error = outcome
            .unwrap_or_else(|_| panic!("tile_size={tile_size} panicked instead of rejecting input"))
            .unwrap_err();
        assert_eq!(error.code(), spirula_core::ErrorCode::ValueOutOfRange);
    }
}

#[test]
fn gpu_matches_independent_goldens_through_real_projection_sort_composite_chain() {
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    for (name, request, options) in cases() {
        let (image, _) = GpuCompositor::compose(&mut context, &request, options).unwrap();
        check_pixel(&image, 0, name, 2e-5);
        assert_eq!(context.resource_accounting().current_bytes, 0);
        assert_eq!(context.retained_submission_count(), 0);
    }
    let request = request(3, 1.5, &[(2.0, [-2.0, 0.0, 2.0], 0.0)], 0.1, 1e-4);
    let (image, _) = GpuCompositor::compose(
        &mut context,
        &request,
        CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            ..CompositeOptions::default()
        },
    )
    .unwrap();
    for (pixel, name) in ["image_left", "image_center", "image_right"]
        .into_iter()
        .enumerate()
    {
        check_pixel(&image, pixel, name, 2e-5);
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
    let (cleared, metrics) = GpuCompositor::compose_empty(
        &mut context,
        17,
        9,
        0.1,
        0.25,
        CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            ..CompositeOptions::default()
        },
    )
    .unwrap();
    assert_eq!(cleared.alpha_cutoff, 0.1);
    assert_eq!(cleared.early_out_transmittance, 0.25);
    assert_eq!(
        metrics.dispatch_count, 1,
        "clear must execute the GPU shader"
    );
    for pixel in 0..153 {
        check_pixel(&cleared, pixel, "image_left", 2e-5);
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);

    let allocated = context.resource_accounting().allocated_bytes;
    assert!(
        GpuCompositor::compose(
            &mut context,
            &request,
            CompositeOptions {
                max_host_bytes: 64,
                ..CompositeOptions::default()
            },
        )
        .is_err()
    );
    assert_eq!(context.resource_accounting().allocated_bytes, allocated);
}

#[test]
fn gpu_known_unrepresentable_inputs_reject_before_stage_allocation() {
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let allocated = context.resource_accounting().allocated_bytes;
    let valid = request(1, 0.5, &[(2.0, [0.0; 3], 0.0)], 1e-4, 1e-4);
    for value in [1e-60, 1e100, f64::NAN] {
        let error = GpuCompositor::compose(
            &mut context,
            &valid,
            CompositeOptions {
                background: BackgroundPolicy::Constant([value, 0.0, 0.0]),
                ..CompositeOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), spirula_core::ErrorCode::ValueOutOfRange);
        assert_eq!(context.resource_accounting().allocated_bytes, allocated);
    }
    for (cutoff, early_out) in [(1e-60, 1e-4), (1e-4, 1e-60)] {
        let unrepresentable = request(1, 0.5, &[(2.0, [0.0; 3], 0.0)], cutoff, early_out);
        let error =
            GpuCompositor::compose(&mut context, &unrepresentable, CompositeOptions::default())
                .unwrap_err();
        assert_eq!(error.code(), spirula_core::ErrorCode::ValueOutOfRange);
        assert_eq!(context.resource_accounting().allocated_bytes, allocated);
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

#[test]
fn partial_tile_last_pixel_has_independent_nonzero_color_and_depth() {
    let mut tail = request(17, 16.5, &[(2.0, [-2.0, 0.0, 2.0], 0.0)], 0.1, 1e-4);
    tail.camera = PinholeCamera::try_new(
        17,
        9,
        1.0,
        1.0,
        16.5,
        8.5,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let options = CompositeOptions {
        background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
        ..CompositeOptions::default()
    };
    let cpu = CpuCompositor::compose(&tail, options).unwrap();
    for pixel in 0..153 {
        check_pixel(
            &cpu,
            pixel,
            if pixel == 152 {
                "image_center"
            } else {
                "image_left"
            },
            5e-13,
        );
    }
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let (gpu, _) = GpuCompositor::compose(&mut context, &tail, options).unwrap();
    for pixel in 0..153 {
        check_pixel(
            &gpu,
            pixel,
            if pixel == 152 {
                "image_center"
            } else {
                "image_left"
            },
            2e-5,
        );
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

#[test]
fn composite_stage_oom_and_accepted_timeout_recover_without_leaking() {
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let options = CompositeOptions {
        background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
        ..CompositeOptions::default()
    };
    // The empty path has no F03/F04 GPU submissions. Both injected failures
    // therefore exercise the new F05 stage itself.
    context.inject_next_scope_error(spirula_gpu::ScopeErrorInject::OutOfMemory {
        requested_bytes: Some(4096),
        current_bytes: Some(0),
        budget_bytes: Some(1024),
    });
    let error = GpuCompositor::compose_empty(&mut context, 17, 9, 0.1, 0.25, options).unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::AllocationFailed);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
    assert_eq!(context.pending_resource_counts().commands, 0);
    assert_eq!(context.pending_resource_counts().stagings, 0);

    context.inject_next_wait_timeout();
    let error = GpuCompositor::compose_empty(&mut context, 17, 9, 0.1, 0.25, options).unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::SubmitFailed);
    assert_eq!(context.retained_submission_count(), 1);
    assert!(context.resource_accounting().current_bytes > 0);
    context
        .wait_retained_submissions(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    assert_eq!(context.retained_submission_count(), 0);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.pending_resource_counts().commands, 0);
    assert_eq!(context.pending_resource_counts().stagings, 0);

    let (recovered, metrics) =
        GpuCompositor::compose_empty(&mut context, 17, 9, 0.1, 0.25, options).unwrap();
    assert_eq!(metrics.dispatch_count, 1);
    for pixel in 0..153 {
        check_pixel(&recovered, pixel, "image_left", 2e-5);
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

#[test]
fn composite_dispatch_rejects_foreign_contracts_ranges_and_stale_handles() {
    use spirula_core::ErrorCode;
    use spirula_gpu::{
        BufferDesc, BufferUsage, CapabilityRequest, ComputePipelineDesc, GpuContext, ResourceLabel,
        SpecConstants,
    };
    let mut context = match GpuContext::open(&CapabilityRequest::diagnose_default()) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("compositor_review_gpu_unavailable: {error:?}");
            assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
            return;
        }
    };
    let label = || ResourceLabel::parse("spirula:render:buffer:review-composite").unwrap();
    let params = context
        .create_buffer(BufferDesc {
            label: label(),
            size: 16,
            usage: BufferUsage::UNIFORM_UPLOAD,
        })
        .unwrap();
    let input = context
        .create_buffer(BufferDesc {
            label: label(),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    let output = context
        .create_buffer(BufferDesc {
            label: label(),
            size: 64,
            usage: BufferUsage::STORAGE_COPY,
        })
        .unwrap();
    context.upload_buffer(&params, 0, &[0; 16]).unwrap();
    // Width/height=0 ensures the shader returns before reading tile ranges.
    context.upload_buffer(&input, 0, &[0; 64]).unwrap();
    let shader = include_str!("../shaders/compositing.wgsl");
    let descriptor = |source: String, entry: &str, layout| ComputePipelineDesc {
        source,
        entry_point: entry.to_owned(),
        spec: SpecConstants::empty(),
        bind_layout_version: layout,
        label: ResourceLabel::parse("spirula:render:pipeline:review-composite").unwrap(),
    };
    let valid = context
        .obtain_compute_pipeline(descriptor(shader.to_owned(), "composite", 1))
        .unwrap();
    context
        .dispatch_compositing(&valid, &params, &input, &output, 64, 64, 1)
        .unwrap();
    for (source, entry, layout) in [
        (
            include_str!("../shaders/projection-tiles.wgsl").to_owned(),
            "project",
            1,
        ),
        (
            shader.replacen("version: 1", "version: 2", 1),
            "composite",
            1,
        ),
        (shader.to_owned(), "composite", 2),
        (
            shader.replace("fn composite(", "fn other_composite("),
            "other_composite",
            1,
        ),
    ] {
        let invalid = context
            .obtain_compute_pipeline(descriptor(source, entry, layout))
            .unwrap();
        assert_eq!(
            context
                .dispatch_compositing(&invalid, &params, &input, &output, 64, 64, 1)
                .unwrap_err()
                .code(),
            ErrorCode::InvalidArgument,
        );
    }
    assert_eq!(
        context
            .dispatch_compositing(&valid, &params, &input, &input, 64, 64, 1)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument,
    );
    for (input_bytes, output_bytes, invocations) in [(0, 64, 1), (128, 64, 1), (64, 64, 0)] {
        assert_eq!(
            context
                .dispatch_compositing(
                    &valid,
                    &params,
                    &input,
                    &output,
                    input_bytes,
                    output_bytes,
                    invocations,
                )
                .unwrap_err()
                .code(),
            ErrorCode::ValueOutOfRange,
        );
    }
    let mut other = GpuContext::open(&CapabilityRequest::diagnose_default()).unwrap();
    assert_eq!(
        other
            .dispatch_compositing(&valid, &params, &input, &output, 64, 64, 1)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidArgument,
    );
    let submission = context.submit().unwrap();
    context.wait(submission, None).unwrap();
    context
        .reopen(&CapabilityRequest::diagnose_default())
        .unwrap();
    assert_eq!(
        context
            .dispatch_compositing(&valid, &params, &input, &output, 64, 64, 1)
            .unwrap_err()
            .code(),
        ErrorCode::DeviceLost,
    );
    drop((valid, params, input, output));
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

#[test]
fn many_culled_sources_respect_host_budget_without_becoming_a_pair_cap() {
    let layers = vec![(-2.0, [0.0; 3], 0.0); 10_000];
    // Negative helper depth puts all sources behind the camera, so P=0.
    let culled = request(1, 0.5, &layers, 0.1, 1e-4);
    let options = CompositeOptions {
        max_pairs: 4,
        max_host_bytes: 8 << 20,
        ..CompositeOptions::default()
    };
    let cpu = CpuCompositor::compose(&culled, options).unwrap();
    assert_eq!(cpu.pixels[0], spirula_render::CompositePixel::default());
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let allocated = context.resource_accounting().allocated_bytes;
    let error = GpuCompositor::compose(
        &mut context,
        &culled,
        CompositeOptions {
            max_host_bytes: 1 << 20,
            ..options
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::ValueOutOfRange);
    assert_eq!(context.resource_accounting().allocated_bytes, allocated);
    let (gpu, metrics) = GpuCompositor::compose(&mut context, &culled, options).unwrap();
    assert_eq!(metrics.pair_count, 0);
    assert_eq!(gpu.pixels, cpu.pixels);
    assert!(metrics.host_peak_bound_bytes <= options.max_host_bytes);
    assert!(context.resource_accounting().peak_bytes <= metrics.accounted_peak_bound_bytes);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
    context.close();
    let error = GpuCompositor::compose_empty(&mut context, 1, 1, 0.1, 1e-4, options).unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::DeviceLost);
}

#[test]
fn known_impossible_composite_gpu_budget_rejects_before_projection_allocation() {
    let mut large = request(512, 256.0, &[(2.0, [0.0; 3], 0.0)], 1e-4, 1e-4);
    large.camera = PinholeCamera::try_new(
        512,
        512,
        600.0,
        600.0,
        256.0,
        256.0,
        OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
        CameraPose::identity(),
    )
    .unwrap();
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("compositor_review_gpu_unavailable: {error:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let before = context.resource_accounting().allocated_bytes;
    // Even P=0 needs 2*32*512*512 bytes for output + readback. This is
    // already impossible at entry, before projection can discover any pairs.
    let error = GpuCompositor::compose(
        &mut context,
        &large,
        CompositeOptions {
            max_gpu_bytes: 1 << 20,
            ..CompositeOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::AllocationFailed);
    assert_eq!(
        context.resource_accounting().allocated_bytes,
        before,
        "known impossible output budget must fail before upstream GPU allocation"
    );
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);

    context.set_resource_budget(Some(1 << 20));
    let error =
        GpuCompositor::compose(&mut context, &large, CompositeOptions::default()).unwrap_err();
    assert_eq!(error.code(), spirula_core::ErrorCode::AllocationFailed);
    assert_eq!(context.resource_accounting().allocated_bytes, before);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

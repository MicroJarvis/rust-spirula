//! P4-F06: float composite → opaque RGB8 pack and headless GPU preview facade.
//!
//! Product bytes are working-space linear RGB after Constant background blend,
//! quantized with clamp[0,1] + half-away (`lround` for non-negative).
//! Not webviewer premultiplied RGBA; not sRGB/appearance gamma.
//!
//! Pack validates public `CompositeImage` version / extent / finite RGB /
//! Constant background. Transmittance and depth are **ignored metadata** for
//! the PNG path (not re-validated as a full composite contract).

use crate::RenderRequest;
use crate::compositing::{
    BackgroundPolicy, COMPOSITE_CONTRACT_VERSION, CompositeImage, CompositeOptions, CompositePixel,
};
use crate::gpu_compositing::{CompositeMetrics, GpuCompositor};
use crate::tiles::error;
use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_gpu::{CapabilityRequest, GpuContext, OpenError};
use std::mem::size_of;
use std::time::{Duration, Instant};

pub const PREVIEW_PACK_CONTRACT_VERSION: u32 = 1;

/// Bytes per packed RGB8 pixel.
pub const PREVIEW_RGB8_BYTES_PER_PIXEL: u64 = 3;

/// Host float `CompositePixel` size used for simultaneous peak reservation.
pub const PREVIEW_FLOAT_BYTES_PER_PIXEL: u64 = size_of::<CompositePixel>() as u64;

/// Packed opaque RGB8 preview frame (tight row-major, top-left origin).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewRgb8 {
    pub contract_version: u32,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub rgb8: Vec<u8>,
    pub source_background: BackgroundPolicy,
}

/// Wall-clock and payload reservation accounting for one headless preview.
///
/// Honesty labels:
/// - `rgb8_bytes` / `host_rgb8_payload_bytes`: packed RGB8 **payload size** only
///   (allocation), **not** bytes moved on the bus.
/// - `host_float_plus_rgb8_reserve_bytes`: checked simultaneous host peak
///   (`56*M + 3*M`) reserved **before** F05; not measured RSS.
/// - `composite`: F05 wall encode/sync times + accounted GPU buffer peaks —
///   **does not** report D2H moved-byte volume.
#[derive(Clone, Debug)]
pub struct HeadlessPreviewMetrics {
    pub splat_count: u64,
    pub width: u32,
    pub height: u32,
    pub wall_render_ns: u128,
    pub composite: CompositeMetrics,
    pub rgb8_bytes: u64,
    /// Alias of payload size (not bytes-moved).
    pub host_rgb8_payload_bytes: u64,
    /// Reserved simultaneous host peak: float frame + RGB8 pack (checked before F05).
    pub host_float_plus_rgb8_reserve_bytes: u64,
    /// True when `wait_retained_submissions` completed after compose / on error.
    pub retained_wait_ok: bool,
}

#[derive(Clone, Debug)]
pub struct HeadlessPreviewResult {
    pub frame: PreviewRgb8,
    pub metrics: HeadlessPreviewMetrics,
}

/// Quantize one linear working-space channel to u8 (TrainerCore `to_png_bytes`
/// arithmetic on already-blended floats — no gamma).
///
/// For `v` in `[0,1]`, matches C++ `lround(v * 255)` (half away from zero).
pub fn quantize_linear_channel_u8(v: f64) -> Result<u8, DomainError> {
    if !v.is_finite() {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "preview_rgb",
            "nonfinite channel",
        ));
    }
    let clamped = v.clamp(0.0, 1.0);
    Ok((clamped * 255.0 + 0.5).floor() as u8)
}

fn checked_pixels(width: u32, height: u32) -> Result<u64, DomainError> {
    if width == 0 || height == 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "extent",
            "width and height must be > 0",
        ));
    }
    u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "extent",
                "width*height overflow",
            )
        })
}

/// Host bytes that must coexist: F05 float output + RGB8 pack payload.
pub fn preview_host_float_plus_rgb8_bytes(pixel_count: u64) -> Result<u64, DomainError> {
    let float = pixel_count
        .checked_mul(PREVIEW_FLOAT_BYTES_PER_PIXEL)
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "host_bytes",
                "float frame byte overflow",
            )
        })?;
    let rgb8 = pixel_count
        .checked_mul(PREVIEW_RGB8_BYTES_PER_PIXEL)
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "host_bytes",
                "rgb8 payload byte overflow",
            )
        })?;
    float.checked_add(rgb8).ok_or_else(|| {
        error(
            ErrorCode::ValueOutOfRange,
            "host_bytes",
            "float+rgb8 reserve overflow",
        )
    })
}

/// Pack a Constant-background composite into opaque RGB8. Rejects Clear.
///
/// Validates: `contract_version`, positive extent, pixel len, Constant finite
/// background, finite RGB. Does **not** re-validate transmittance/depth.
pub fn pack_composite_to_preview_rgb8(image: &CompositeImage) -> Result<PreviewRgb8, DomainError> {
    if image.contract_version != COMPOSITE_CONTRACT_VERSION {
        return Err(error(
            ErrorCode::UnsupportedVersion,
            "contract_version",
            format!(
                "composite contract {} unsupported (expected {COMPOSITE_CONTRACT_VERSION})",
                image.contract_version
            ),
        ));
    }
    match image.background {
        BackgroundPolicy::Constant(rgb) => {
            if rgb.iter().any(|c| !c.is_finite()) {
                return Err(error(
                    ErrorCode::ValueOutOfRange,
                    "background",
                    "Constant background channels must be finite",
                ));
            }
        }
        BackgroundPolicy::Clear => {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "background",
                "product preview PNG requires Constant background (opaque RGB8)",
            ));
        }
    }
    let width = image.width;
    let height = image.height;
    let pixels = checked_pixels(width, height)?;
    if pixels as usize != image.pixels.len() {
        return Err(error(
            ErrorCode::InvalidArgument,
            "pixels",
            "composite pixel count mismatch",
        ));
    }
    let stride_u64 = u64::from(width)
        .checked_mul(PREVIEW_RGB8_BYTES_PER_PIXEL)
        .ok_or_else(|| error(ErrorCode::ValueOutOfRange, "stride", "width*3 overflow"))?;
    let stride_bytes = u32::try_from(stride_u64)
        .map_err(|_| error(ErrorCode::ValueOutOfRange, "stride", "stride exceeds u32"))?;
    let len = stride_u64.checked_mul(u64::from(height)).ok_or_else(|| {
        error(
            ErrorCode::ValueOutOfRange,
            "rgb8",
            "rgb8 byte length overflow",
        )
    })?;
    let len_usize = usize::try_from(len).map_err(|_| {
        error(
            ErrorCode::ValueOutOfRange,
            "rgb8",
            "rgb8 length exceeds usize",
        )
    })?;
    let mut rgb8 = Vec::new();
    rgb8.try_reserve_exact(len_usize).map_err(|_| {
        error(
            ErrorCode::AllocationFailed,
            "rgb8",
            "host rgb8 allocation failed",
        )
    })?;
    for y in 0..height {
        for x in 0..width {
            let idx = (y as usize)
                .checked_mul(width as usize)
                .and_then(|r| r.checked_add(x as usize))
                .ok_or_else(|| {
                    error(ErrorCode::ValueOutOfRange, "index", "pixel index overflow")
                })?;
            let p = &image.pixels[idx];
            // T/depth intentionally ignored for opaque RGB8 packaging.
            rgb8.push(quantize_linear_channel_u8(p.rgb[0])?);
            rgb8.push(quantize_linear_channel_u8(p.rgb[1])?);
            rgb8.push(quantize_linear_channel_u8(p.rgb[2])?);
        }
    }
    debug_assert_eq!(rgb8.len(), len_usize);
    Ok(PreviewRgb8 {
        contract_version: PREVIEW_PACK_CONTRACT_VERSION,
        width,
        height,
        stride_bytes,
        rgb8,
        source_background: image.background,
    })
}

fn open_err(err: OpenError) -> DomainError {
    err.error
}

fn settle_retained(ctx: &mut GpuContext) -> bool {
    ctx.wait_retained_submissions(Some(Duration::from_secs(30)))
        .is_ok()
}

fn validate_preview_options(options: &CompositeOptions) -> Result<(), DomainError> {
    match options.background {
        BackgroundPolicy::Constant(rgb) => {
            if rgb.iter().any(|c| !c.is_finite()) {
                return Err(error(
                    ErrorCode::ValueOutOfRange,
                    "background",
                    "Constant background channels must be finite",
                ));
            }
        }
        BackgroundPolicy::Clear => {
            return Err(error(
                ErrorCode::UnimplementedOption,
                "background",
                "product preview PNG requires Constant background",
            ));
        }
    }
    Ok(())
}

fn prepare_compose_options(
    request: &RenderRequest,
    options: CompositeOptions,
) -> Result<(CompositeOptions, u64, u64), DomainError> {
    let width = request.camera.width();
    let height = request.camera.height();
    let pixel_count = checked_pixels(width, height)?;
    let reserve = preview_host_float_plus_rgb8_bytes(pixel_count)?;
    if reserve > options.max_host_bytes {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "host_bytes",
            format!(
                "float+rgb8 reserve {reserve} exceeds max_host_bytes {}",
                options.max_host_bytes
            ),
        ));
    }
    // Leave RGB8 pack room inside the F05 host budget (simultaneous peak).
    let rgb8_payload = pixel_count
        .checked_mul(PREVIEW_RGB8_BYTES_PER_PIXEL)
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "host_bytes",
                "rgb8 payload overflow",
            )
        })?;
    let mut compose_options = options;
    compose_options.max_host_bytes = options.max_host_bytes.saturating_sub(rgb8_payload);
    if compose_options.max_host_bytes == 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "host_bytes",
            "no host budget remains after rgb8 pack reserve",
        ));
    }
    Ok((compose_options, reserve, rgb8_payload))
}

/// Render-private helper: run headless preview on a caller-owned live context.
/// Enables GPU inject (accepted wait timeout) without a public product test flag.
///
/// `wall_started` is the Instant that owns `wall_render_ns`. The public facade
/// starts it **before** `GpuContext::open` (open+compose+pack). Internal
/// inject/cleanup tests may pass `Instant::now()` at helper entry.
pub(crate) fn run_headless_gpu_preview_on_context(
    ctx: &mut GpuContext,
    request: &RenderRequest,
    options: CompositeOptions,
    cancel: &CancelToken,
    wall_started: Instant,
) -> Result<HeadlessPreviewResult, DomainError> {
    cancel.check()?;
    validate_preview_options(&options)?;
    let (compose_options, reserve, _rgb8_payload) = prepare_compose_options(request, options)?;

    let compose_result = GpuCompositor::compose_cancellable(ctx, request, compose_options, cancel);
    let (image, composite, retained_wait_ok) = match compose_result {
        Ok(v) => {
            let ok = settle_retained(ctx);
            (v.0, v.1, ok)
        }
        Err(e) => {
            // Timeout / cancel may leave accepted submissions; wait explicitly
            // instead of claiming Drop alone completes GPU work.
            let _ = settle_retained(ctx);
            return Err(e);
        }
    };
    cancel.check()?;
    let frame = pack_composite_to_preview_rgb8(&image)?;
    let rgb8_bytes = frame.rgb8.len() as u64;
    Ok(HeadlessPreviewResult {
        metrics: HeadlessPreviewMetrics {
            splat_count: request.scene.len() as u64,
            width: frame.width,
            height: frame.height,
            wall_render_ns: wall_started.elapsed().as_nanos(),
            composite,
            rgb8_bytes,
            host_rgb8_payload_bytes: rgb8_bytes,
            host_float_plus_rgb8_reserve_bytes: reserve,
            retained_wait_ok,
        },
        frame,
    })
}

/// Render-owned headless preview: one private `GpuContext`, real compose, pack,
/// honest retained-submission wait before drop (timeout paths included).
///
/// `wall_render_ns` covers **open + compose + pack** (contract).
pub fn run_headless_gpu_preview(
    request: &RenderRequest,
    options: CompositeOptions,
    cancel: &CancelToken,
) -> Result<HeadlessPreviewResult, DomainError> {
    cancel.check()?;
    validate_preview_options(&options)?;
    // Fail fast on host budget before opening a device.
    let _ = prepare_compose_options(request, options)?;
    let wall = Instant::now();
    let mut ctx = GpuContext::open(&CapabilityRequest::diagnose_default()).map_err(open_err)?;
    run_headless_gpu_preview_on_context(&mut ctx, request, options, cancel, wall)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositing::CpuCompositor;
    use crate::{ColorSpace, RenderPrimitive, RenderThresholds};
    use spirula_core::{
        CameraPose, GaussianSplat, OpenCvDistortion, PinholeCamera, Quat, SplatSet, Vec3,
    };

    #[test]
    fn quantize_matches_lround_samples() {
        assert_eq!(quantize_linear_channel_u8(0.0).unwrap(), 0);
        assert_eq!(quantize_linear_channel_u8(1.0).unwrap(), 255);
        assert_eq!(quantize_linear_channel_u8(0.5).unwrap(), 128);
        assert_eq!(quantize_linear_channel_u8(2.0).unwrap(), 255);
        assert_eq!(quantize_linear_channel_u8(-0.1).unwrap(), 0);
    }

    #[test]
    fn float_pixel_size_is_56() {
        assert_eq!(PREVIEW_FLOAT_BYTES_PER_PIXEL, 56);
    }

    #[test]
    fn pack_rejects_clear_and_bad_version() {
        let mut image = CpuCompositor::compose_empty(
            1,
            1,
            0.01,
            1e-4,
            CompositeOptions {
                background: BackgroundPolicy::Constant([0.2, 0.4, 0.6]),
                ..CompositeOptions::default()
            },
        )
        .unwrap();
        image.background = BackgroundPolicy::Clear;
        assert_eq!(
            pack_composite_to_preview_rgb8(&image).unwrap_err().code(),
            ErrorCode::UnimplementedOption
        );
        image.background = BackgroundPolicy::Constant([0.2, 0.4, 0.6]);
        image.contract_version = 0;
        assert_eq!(
            pack_composite_to_preview_rgb8(&image).unwrap_err().code(),
            ErrorCode::UnsupportedVersion
        );
    }

    #[test]
    fn pack_asymmetric_row_major_rgb_order() {
        let mut pixels = Vec::new();
        for y in 0..2u32 {
            for x in 0..3u32 {
                pixels.push(CompositePixel {
                    rgb: [
                        (x as f64) / 255.0,
                        (y as f64) / 255.0,
                        ((x + 10 * y) as f64) / 255.0,
                    ],
                    transmittance: 0.0,
                    alpha: 1.0,
                    expected_depth: 0.0,
                    median_depth: 0.0,
                });
            }
        }
        let image = CompositeImage {
            contract_version: COMPOSITE_CONTRACT_VERSION,
            width: 3,
            height: 2,
            alpha_cutoff: 0.01,
            effective_alpha_cutoff: 0.01,
            early_out_transmittance: 1e-4,
            effective_early_out: 1e-4,
            background: BackgroundPolicy::Constant([0.0, 0.0, 0.0]),
            pixels,
        };
        let frame = pack_composite_to_preview_rgb8(&image).unwrap();
        assert_eq!((frame.width, frame.height, frame.stride_bytes), (3, 2, 9));
        assert_eq!(&frame.rgb8[0..3], &[0, 0, 0]);
        assert_eq!(&frame.rgb8[6..9], &[2, 0, 2]);
        assert_eq!(&frame.rgb8[9..12], &[0, 1, 10]);
        assert_eq!(&frame.rgb8[15..18], &[2, 1, 12]);
    }

    #[test]
    fn facade_rejects_host_budget_that_cannot_hold_float_and_rgb8() {
        let camera = PinholeCamera::try_new(
            3,
            2,
            2.0,
            2.0,
            1.5,
            1.0,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .unwrap();
        let splat = GaussianSplat::try_new(
            Vec3::try_new(0.0, 0.0, -1.0).unwrap(),
            Quat::try_from_wxyz(1.0, 0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(-20.0, -20.0, -20.0).unwrap(),
            0.0,
            Vec3::try_new(0.5, 0.5, 0.5).unwrap(),
        )
        .unwrap();
        let request = RenderRequest::try_new(
            camera,
            SplatSet::try_from_splats(vec![splat]).unwrap(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds {
                alpha_cutoff: 0.01,
                early_out_transmittance: 1e-4,
            },
        )
        .unwrap();
        let options = CompositeOptions {
            background: BackgroundPolicy::Constant([0.0, 0.0, 0.0]),
            max_host_bytes: 100, // far below 6*(56+3)
            ..CompositeOptions::default()
        };
        let err = run_headless_gpu_preview(&request, options, &CancelToken::new()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }

    fn require_host() -> Option<GpuContext> {
        match GpuContext::open(&CapabilityRequest::diagnose_default()) {
            Ok(ctx) => Some(ctx),
            Err(err) => {
                assert!(
                    std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none(),
                    "required GPU unavailable: {err:?}"
                );
                eprintln!("preview retained-cleanup GPU test skipped: {err:?}");
                None
            }
        }
    }

    fn tiny_request() -> RenderRequest {
        let camera = PinholeCamera::try_new(
            8,
            8,
            8.0,
            8.0,
            4.0,
            4.0,
            OpenCvDistortion::try_new(0.0, 0.0, 0.0, 0.0).unwrap(),
            CameraPose::identity(),
        )
        .unwrap();
        let splat = GaussianSplat::try_new(
            Vec3::try_new(0.0, 0.0, -2.0).unwrap(),
            Quat::try_from_wxyz(1.0, 0.0, 0.0, 0.0).unwrap(),
            Vec3::try_new(-20.0, -20.0, -20.0).unwrap(),
            0.0,
            Vec3::try_new(0.5, 0.5, 0.5).unwrap(),
        )
        .unwrap();
        RenderRequest::try_new(
            camera,
            SplatSet::try_from_splats(vec![splat]).unwrap(),
            RenderPrimitive::Gaussian3d,
            ColorSpace::LinearRgb,
            0,
            RenderThresholds {
                alpha_cutoff: 0.01,
                early_out_transmittance: 1e-4,
            },
        )
        .unwrap()
    }

    #[test]
    fn injected_wait_timeout_settles_retained_then_retry_succeeds() {
        // Uses render-private on_context helper + GpuContext::inject_next_wait_timeout
        // (no public product test flag). Proves retained cleanup — not pre-open cancel.
        let Some(mut ctx) = require_host() else {
            return;
        };
        let request = tiny_request();
        let options = CompositeOptions {
            background: BackgroundPolicy::Constant([0.1, 0.2, 0.3]),
            max_pixels: 64,
            max_host_bytes: 64 << 20,
            max_gpu_bytes: 64 << 20,
            ..CompositeOptions::default()
        };
        ctx.inject_next_wait_timeout();
        let err = run_headless_gpu_preview_on_context(
            &mut ctx,
            &request,
            options,
            &CancelToken::new(),
            Instant::now(),
        )
        .expect_err("injected accepted wait timeout");
        assert_eq!(err.code(), ErrorCode::SubmitFailed);
        assert!(err.message().contains("timed out"), "timeout message={err}");
        // settle_retained inside the helper should already have drained; assert ledger.
        assert_eq!(ctx.retained_submission_count(), 0);
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
        let pending = ctx.pending_resource_counts();
        assert_eq!(pending.commands, 0);
        assert_eq!(pending.stagings, 0);
        assert_eq!(pending.buffers, 0);
        assert_eq!(pending.textures, 0);

        let ok = run_headless_gpu_preview_on_context(
            &mut ctx,
            &request,
            options,
            &CancelToken::new(),
            Instant::now(),
        )
        .expect("retry after retained settle");
        assert!(ok.metrics.retained_wait_ok);
        assert_eq!(ok.frame.width, 8);
        assert_eq!(ctx.retained_submission_count(), 0);
        assert_eq!(ctx.resource_accounting().current_bytes, 0);
    }
}

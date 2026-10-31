//! P4-F06: application-owned headless preview orchestration.
//!
//! Loads the versioned request via `spirula-io`, renders through the
//! render-owned GPU facade (not the presentation GpuContext edge), encodes
//! and atomically publishes PNG. Control history only sees a bounded summary.

use std::path::Path;
use std::time::Instant;

use spirula_core::{CancelToken, DomainError, ErrorCode};
use spirula_io::{
    encode_preview_png_rgb8, load_preview_scene, preview_encode_host_need, publish_preview_png,
};
use spirula_render::{
    BackgroundPolicy, ColorSpace, CompositeOptions, RenderPrimitive, RenderRequest,
    RenderThresholds, run_headless_gpu_preview,
};

/// Bounded summary retained in control history / CLI presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewSummary {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub splat_count: u64,
    pub rgb8_bytes: u64,
    pub png_bytes: u64,
    pub wall_load_ns: u128,
    pub wall_render_ns: u128,
    pub wall_encode_ns: u128,
    pub wall_publish_ns: u128,
}

/// Worker outcome. `published` is true iff atomic rename committed.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // Done carries DomainError + summary once
pub(crate) enum PreviewWorkResult {
    /// Rename committed. `Ok` = full success; `Err` may be post-publish durability.
    Done {
        summary: PreviewSummary,
        error: Option<DomainError>,
        published: bool,
    },
    Canceled,
}

fn canceled(cancel: Option<&CancelToken>) -> bool {
    cancel.is_some_and(|c| c.is_cancelled())
}

fn host_err(need: u64, have: u64) -> DomainError {
    DomainError::try_new(
        ErrorCode::ValueOutOfRange,
        format!("preview encode/publish needs {need} host bytes, budget is {have}"),
    )
    .expect("message")
    .with_context(spirula_core::ContextKey::Field, "max_host_bytes")
    .expect("field")
}

/// Run one preview on the worker thread.
pub(crate) fn run_preview_cancelable(
    request_path: &Path,
    output_path: &Path,
    cancel: Option<&CancelToken>,
) -> PreviewWorkResult {
    if canceled(cancel) {
        return PreviewWorkResult::Canceled;
    }
    let owned;
    let token = match cancel {
        Some(c) => c,
        None => {
            owned = CancelToken::new();
            &owned
        }
    };

    let loaded = match load_preview_scene(request_path, Some(token)) {
        Ok(v) => v,
        Err(e) if e.code() == ErrorCode::Canceled => return PreviewWorkResult::Canceled,
        Err(e) => {
            return PreviewWorkResult::Done {
                summary: empty_summary(output_path),
                error: Some(e),
                published: false,
            };
        }
    };
    if canceled(cancel) {
        return PreviewWorkResult::Canceled;
    }

    let spirula_io::LoadedPreviewScene {
        request,
        splats,
        wall_load_ns,
    } = loaded;
    let budgets = request.budgets;
    let alpha_cutoff = request.alpha_cutoff;
    let early_out = request.early_out_transmittance;
    let background = request.background;
    let camera = request.camera;
    // Descriptor/paths dropped with `request`; scene moves into RenderRequest.
    drop(request);

    let render_request = match RenderRequest::try_new(
        camera,
        splats,
        RenderPrimitive::Gaussian3d,
        ColorSpace::LinearRgb,
        0,
        RenderThresholds {
            alpha_cutoff,
            early_out_transmittance: early_out,
        },
    ) {
        Ok(r) => r,
        Err(e) => {
            return PreviewWorkResult::Done {
                summary: empty_summary(output_path),
                error: Some(e),
                published: false,
            };
        }
    };

    // F05 host_peak_bound excludes the caller scene. Reserve live splat Vec
    // capacity before handing the remainder to the render facade.
    let scene_host = render_request.scene.host_capacity_bytes();
    let render_host = match budgets.max_host_bytes.checked_sub(scene_host) {
        Some(v) if v > 0 => v,
        _ => {
            return PreviewWorkResult::Done {
                summary: empty_summary(output_path),
                error: Some(host_err(
                    scene_host.saturating_add(1),
                    budgets.max_host_bytes,
                )),
                published: false,
            };
        }
    };

    let options = CompositeOptions {
        background: BackgroundPolicy::Constant(background),
        max_pixels: budgets.max_pixels,
        max_host_bytes: render_host,
        max_gpu_bytes: budgets.max_gpu_bytes,
        ..CompositeOptions::default()
    };

    let preview = match run_headless_gpu_preview(&render_request, options, token) {
        Ok(v) => v,
        Err(e) if e.code() == ErrorCode::Canceled => return PreviewWorkResult::Canceled,
        Err(e) => {
            return PreviewWorkResult::Done {
                summary: empty_summary(output_path),
                error: Some(e),
                published: false,
            };
        }
    };
    // F05 host peak excludes caller scene — free it before encode allocations.
    drop(render_request);
    if canceled(cancel) {
        return PreviewWorkResult::Canceled;
    }

    let encode_need = match preview_encode_host_need(
        preview.frame.width,
        preview.frame.height,
        preview.metrics.rgb8_bytes,
        budgets.max_image_bytes,
    ) {
        Ok(n) => n,
        Err(e) => {
            return PreviewWorkResult::Done {
                summary: empty_summary(output_path),
                error: Some(e),
                published: false,
            };
        }
    };
    if encode_need > budgets.max_host_bytes {
        return PreviewWorkResult::Done {
            summary: PreviewSummary {
                path: output_path.display().to_string(),
                width: preview.frame.width,
                height: preview.frame.height,
                splat_count: preview.metrics.splat_count,
                rgb8_bytes: preview.metrics.rgb8_bytes,
                png_bytes: 0,
                wall_load_ns,
                wall_render_ns: preview.metrics.wall_render_ns,
                wall_encode_ns: 0,
                wall_publish_ns: 0,
            },
            error: Some(host_err(encode_need, budgets.max_host_bytes)),
            published: false,
        };
    }

    let encode_started = Instant::now();
    let png = match encode_preview_png_rgb8(
        preview.frame.width,
        preview.frame.height,
        &preview.frame.rgb8,
        budgets.max_image_bytes,
    ) {
        Ok(b) => b,
        Err(e) => {
            return PreviewWorkResult::Done {
                summary: PreviewSummary {
                    path: output_path.display().to_string(),
                    width: preview.frame.width,
                    height: preview.frame.height,
                    splat_count: preview.metrics.splat_count,
                    rgb8_bytes: preview.metrics.rgb8_bytes,
                    png_bytes: 0,
                    wall_load_ns,
                    wall_render_ns: preview.metrics.wall_render_ns,
                    wall_encode_ns: encode_started.elapsed().as_nanos(),
                    wall_publish_ns: 0,
                },
                error: Some(e),
                published: false,
            };
        }
    };
    let wall_encode_ns = encode_started.elapsed().as_nanos();
    let width = preview.frame.width;
    let height = preview.frame.height;
    let splat_count = preview.metrics.splat_count;
    let rgb8_bytes = preview.metrics.rgb8_bytes;
    let wall_render_ns = preview.metrics.wall_render_ns;
    // Drop source RGB8 before publish validation decode (avoids 2× RGB8 peak).
    drop(preview);
    if canceled(cancel) {
        return PreviewWorkResult::Canceled;
    }

    let publish_started = Instant::now();
    match publish_preview_png(output_path, &png, Some(token)) {
        Ok(_) => PreviewWorkResult::Done {
            summary: PreviewSummary {
                path: output_path.display().to_string(),
                width,
                height,
                splat_count,
                rgb8_bytes,
                png_bytes: png.len() as u64,
                wall_load_ns,
                wall_render_ns,
                wall_encode_ns,
                wall_publish_ns: publish_started.elapsed().as_nanos(),
            },
            error: None,
            published: true,
        },
        Err(e) if e.code() == ErrorCode::Canceled => PreviewWorkResult::Canceled,
        Err(e) => {
            // Durability failure after rename carries actual=postpublish_directory_sync.
            let published = e.context_value(spirula_core::ContextKey::Actual)
                == Some("postpublish_directory_sync");
            PreviewWorkResult::Done {
                summary: PreviewSummary {
                    path: output_path.display().to_string(),
                    width,
                    height,
                    splat_count,
                    rgb8_bytes,
                    png_bytes: png.len() as u64,
                    wall_load_ns,
                    wall_render_ns,
                    wall_encode_ns,
                    wall_publish_ns: publish_started.elapsed().as_nanos(),
                },
                error: Some(e),
                published,
            }
        }
    }
}

fn empty_summary(output_path: &Path) -> PreviewSummary {
    PreviewSummary {
        path: output_path.display().to_string(),
        width: 0,
        height: 0,
        splat_count: 0,
        rgb8_bytes: 0,
        png_bytes: 0,
        wall_load_ns: 0,
        wall_render_ns: 0,
        wall_encode_ns: 0,
        wall_publish_ns: 0,
    }
}

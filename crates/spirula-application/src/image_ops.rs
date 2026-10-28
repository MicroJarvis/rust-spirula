//! Minimal application/session entry for P3-F02 image orient / mask normalize.
//!
//! Real `spirula-io` decode + bake (not a fake Probe*). Wired through
//! [`crate::command::CommandPayload::PrepareOrientedImage`] /
//! [`crate::command::CommandPayload::PrepareNormalizedMask`] and also exposed as
//! sync helpers on [`ApplicationService`] for the same Idle|Ready gate.

use std::path::Path;

use spirula_core::{CancelToken, ContextKey, DomainError, ErrorCode};
use spirula_io::{
    ApplyPolicy, ExifOrientation, MaskNormalizeOptions, MaskSourceKind,
    apply_orientation_cancelable, decode_mask_image_bytes_hinted, decode_rgb8_bytes_hinted,
    normalize_mask_cancelable, oriented_size, probe_image_size, read_image_bytes_capped,
    read_path_exif_orientation,
};

fn path_ext_hint(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
}

use crate::session::ApplicationService;
use crate::state::SessionState;

fn err(code: ErrorCode, field: &str, msg: impl Into<String>) -> DomainError {
    DomainError::try_new(code, msg)
        .expect("msg")
        .with_context(ContextKey::Field, field)
        .expect("field")
}

/// Oriented RGB8 produced through the application owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrientedRgb {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub orientation: u8,
    pub policy: ApplyPolicy,
}

/// Normalized 0/1 mask produced through the application owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedMask {
    pub width: u32,
    pub height: u32,
    pub samples: Vec<u8>,
}

/// Arguments for [`ApplicationService::prepare_normalized_mask_file`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareMaskArgs {
    pub dst_w: u32,
    pub dst_h: u32,
    pub kind: MaskSourceKind,
    pub orientation: u8,
    pub policy: ApplyPolicy,
    pub flip: bool,
    /// `mask_boundary_offset` fraction (`sqrt(W*H)` pixels).
    pub boundary_offset_frac: f32,
}

/// Worker-facing prepare outcome. `Canceled` is observed at cooperative
/// checkpoints between IO stages — not mid-`image` crate decode.
#[derive(Debug)]
pub(crate) enum ImageWorkResult<T> {
    Ready(T),
    Failed(DomainError),
    Canceled,
}

fn canceled(cancel: Option<&CancelToken>) -> bool {
    cancel.is_some_and(|c| c.is_cancelled())
}

pub(crate) fn run_prepare_oriented_rgb(
    path: &Path,
    orientation_override: Option<u8>,
    policy: ApplyPolicy,
) -> Result<OrientedRgb, DomainError> {
    match run_prepare_oriented_rgb_cancelable(path, orientation_override, policy, None) {
        ImageWorkResult::Ready(v) => Ok(v),
        ImageWorkResult::Failed(e) => Err(e),
        ImageWorkResult::Canceled => Err(err(
            ErrorCode::Internal,
            "cancel",
            "unexpected cancel without token",
        )),
    }
}

pub(crate) fn run_prepare_oriented_rgb_cancelable(
    path: &Path,
    orientation_override: Option<u8>,
    policy: ApplyPolicy,
    cancel: Option<&CancelToken>,
) -> ImageWorkResult<OrientedRgb> {
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    let orientation = match orientation_override {
        Some(v) => match ExifOrientation::from_u8(v) {
            Ok(o) => o,
            Err(e) => return ImageWorkResult::Failed(e),
        },
        None => match read_path_exif_orientation(path) {
            Ok(o) => o,
            Err(e) => return ImageWorkResult::Failed(e),
        },
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    let bytes = match read_image_bytes_capped(path) {
        Ok(b) => b,
        Err(e) => return ImageWorkResult::Failed(e),
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    // Preserve path extension hint (TGA has no reliable magic).
    let hint = path_ext_hint(path);
    let (w, h, rgb) = match decode_rgb8_bytes_hinted(&bytes, hint.as_deref()) {
        Ok(v) => v,
        Err(e) => return ImageWorkResult::Failed(e),
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    let xf = {
        let mut t = orientation.transform();
        if matches!(policy, ApplyPolicy::TurnsOnly) {
            t.mirror = false;
        }
        t
    };
    let (ow, oh) = oriented_size(w, h, xf.turns_cw);
    if let Err(e) = spirula_io::ensure_pixel_budget(ow, oh) {
        return ImageWorkResult::Failed(e);
    }
    let dst_len = match (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(3))
    {
        Some(n) => n,
        None => {
            return ImageWorkResult::Failed(err(
                ErrorCode::ValueOutOfRange,
                "image_size",
                "oriented byte product overflow",
            ));
        }
    };
    let mut dst = vec![0u8; dst_len];
    let (dw, dh) =
        match apply_orientation_cancelable(&rgb, w, h, 3, orientation, policy, &mut dst, cancel) {
            Ok(v) => v,
            Err(e) if e.code() == ErrorCode::Canceled => return ImageWorkResult::Canceled,
            Err(e) => return ImageWorkResult::Failed(e),
        };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    ImageWorkResult::Ready(OrientedRgb {
        width: dw,
        height: dh,
        rgb: dst,
        orientation: orientation.get(),
        policy,
    })
}

pub(crate) fn run_prepare_normalized_mask(
    path: &Path,
    args: PrepareMaskArgs,
) -> Result<NormalizedMask, DomainError> {
    match run_prepare_normalized_mask_cancelable(path, args, None) {
        ImageWorkResult::Ready(v) => Ok(v),
        ImageWorkResult::Failed(e) => Err(e),
        ImageWorkResult::Canceled => Err(err(
            ErrorCode::Internal,
            "cancel",
            "unexpected cancel without token",
        )),
    }
}

pub(crate) fn run_prepare_normalized_mask_cancelable(
    path: &Path,
    args: PrepareMaskArgs,
    cancel: Option<&CancelToken>,
) -> ImageWorkResult<NormalizedMask> {
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    // Validate dims/options before any read/decode/allocation (incl. u32::MAX).
    if let Err(e) = spirula_io::ensure_pixel_budget(args.dst_w, args.dst_h) {
        return ImageWorkResult::Failed(e);
    }
    if !args.boundary_offset_frac.is_finite() {
        return ImageWorkResult::Failed(err(
            ErrorCode::ValueOutOfRange,
            "mask_boundary_offset",
            "boundary_offset_frac must be finite",
        ));
    }
    let orientation = match ExifOrientation::from_u8(args.orientation) {
        Ok(o) => o,
        Err(e) => return ImageWorkResult::Failed(e),
    };
    let dst_len = match (args.dst_w as usize).checked_mul(args.dst_h as usize) {
        Some(n) => n,
        None => {
            return ImageWorkResult::Failed(err(
                ErrorCode::ValueOutOfRange,
                "dst_size",
                "destination byte product overflow",
            ));
        }
    };
    let opts = MaskNormalizeOptions {
        kind: args.kind,
        flip: args.flip,
        boundary_offset_frac: args.boundary_offset_frac,
        orientation,
        apply_policy: args.policy,
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    let bytes = match read_image_bytes_capped(path) {
        Ok(b) => b,
        Err(e) => return ImageWorkResult::Failed(e),
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    // Preserve path extension hint (TGA has no reliable magic).
    let hint = path_ext_hint(path);
    let (w, h, ch, samples) = match decode_mask_image_bytes_hinted(&bytes, hint.as_deref()) {
        Ok(v) => v,
        Err(e) => return ImageWorkResult::Failed(e),
    };
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    let mut dst = vec![0u8; dst_len];
    match normalize_mask_cancelable(
        &samples, w, h, ch, args.dst_w, args.dst_h, opts, &mut dst, cancel,
    ) {
        Ok(()) => {}
        Err(e) if e.code() == ErrorCode::Canceled => return ImageWorkResult::Canceled,
        Err(e) => return ImageWorkResult::Failed(e),
    }
    if canceled(cancel) {
        return ImageWorkResult::Canceled;
    }
    ImageWorkResult::Ready(NormalizedMask {
        width: args.dst_w,
        height: args.dst_h,
        samples: dst,
    })
}

impl ApplicationService {
    pub(crate) fn require_idle_or_ready(&self) -> Result<(), DomainError> {
        let phase = self.snapshot().state;
        match phase {
            SessionState::Idle | SessionState::Ready => Ok(()),
            other => Err(err(
                ErrorCode::InvalidState,
                "session",
                format!("image ops require Idle|Ready, got {other:?}"),
            )),
        }
    }

    /// Probe PNG/JPEG size via `spirula-io` (header-only) under session ownership.
    pub fn probe_image_file_size(&self, path: &Path) -> Result<(u32, u32), DomainError> {
        self.require_idle_or_ready()?;
        probe_image_size(path)
    }

    /// Decode + bake EXIF orientation into RGB8. Prefer the session command
    /// [`crate::CommandPayload::PrepareOrientedImage`] for admitted e2e paths.
    pub fn prepare_oriented_rgb_file(
        &self,
        path: &Path,
        orientation_override: Option<u8>,
        policy: ApplyPolicy,
    ) -> Result<OrientedRgb, DomainError> {
        self.require_idle_or_ready()?;
        run_prepare_oriented_rgb(path, orientation_override, policy)
    }

    /// Decode a mask file and normalize. Prefer
    /// [`crate::CommandPayload::PrepareNormalizedMask`] for admitted e2e paths.
    pub fn prepare_normalized_mask_file(
        &self,
        path: &Path,
        args: PrepareMaskArgs,
    ) -> Result<NormalizedMask, DomainError> {
        self.require_idle_or_ready()?;
        run_prepare_normalized_mask(path, args)
    }
}

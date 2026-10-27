//! Host presentation frame values for P6-F07 (no GPU / Slint types).
//!
//! Binding: `docs/phase-6/presentation-path.md`.

use std::time::Duration;

/// How host pixels were obtained from the GPU path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationPathKind {
    /// Texture → staging readback → host tight RGBA (explicit fallback).
    ReadbackToHost,
}

/// Sync / copy milestones recorded for one produced frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameCopyReport {
    /// Tight RGBA byte length (`width * height * 4`).
    pub logical_rgba_bytes: u64,
    /// wgpu-aligned bytes-per-row used for staging readback, when known.
    pub readback_bytes_per_row: Option<u32>,
    /// Staging / padded readback bytes when known.
    pub readback_staging_bytes: Option<u64>,
    /// Extra host copy into a UI pixel buffer (e.g. Slint upload), when known.
    pub ui_pixel_upload_bytes: Option<u64>,
    /// Ordered sync labels (`submit`, `wait`, `map`, `set-image`, …).
    pub sync_points: Vec<&'static str>,
    /// Host produce duration (GPU open work for this frame), when measured.
    pub produce_duration: Option<Duration>,
    /// Notes for unavailable measurements (compositor present, etc.).
    pub unavailable: Vec<&'static str>,
}

/// Application-owned host frame delivered to UI (no `wgpu` types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedFrame {
    pub frame_id: u64,
    /// Size-generation captured when produce started (bumps on resize).
    pub size_generation: u64,
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8 (`width * height * 4`).
    pub rgba8: Vec<u8>,
    pub path: PresentationPathKind,
    pub copies: FrameCopyReport,
    /// Device / backend string for evidence (empty if unavailable).
    pub device_label: String,
}

impl PresentedFrame {
    pub fn expected_bytes(width: u32, height: u32) -> Option<usize> {
        (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)
    }
}

/// CPU reference pattern used by the GPU probe and tests.
pub fn cpu_probe_rgba8(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = (y * width as usize + x) * 4;
            out[i] = (x % 256) as u8;
            out[i + 1] = (y % 256) as u8;
            out[i + 2] = ((x + y) % 256) as u8;
            out[i + 3] = 255;
        }
    }
    out
}

/// Corner RGBA samples (TL, TR, BL, BR) for `cpu_probe_rgba8`.
///
/// For 1×1 all four aliases collapse to the single pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeCorners {
    pub top_left: [u8; 4],
    pub top_right: [u8; 4],
    pub bottom_left: [u8; 4],
    pub bottom_right: [u8; 4],
}

pub fn cpu_probe_corners(width: u32, height: u32) -> Option<ProbeCorners> {
    if width == 0 || height == 0 {
        return None;
    }
    let px = |x: u32, y: u32| -> [u8; 4] {
        [(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]
    };
    let xr = width - 1;
    let yb = height - 1;
    Some(ProbeCorners {
        top_left: px(0, 0),
        top_right: px(xr, 0),
        bottom_left: px(0, yb),
        bottom_right: px(xr, yb),
    })
}

/// Sample corners from tightly packed RGBA8.
pub fn rgba8_corners(width: u32, height: u32, pixels: &[u8]) -> Option<ProbeCorners> {
    let expected = PresentedFrame::expected_bytes(width, height)?;
    if pixels.len() != expected {
        return None;
    }
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = (y as usize * width as usize + x as usize) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    let xr = width - 1;
    let yb = height - 1;
    Some(ProbeCorners {
        top_left: at(0, 0),
        top_right: at(xr, 0),
        bottom_left: at(0, yb),
        bottom_right: at(xr, yb),
    })
}

/// Exact match of packed RGBA8 against [`cpu_probe_rgba8`].
pub fn rgba8_matches_cpu_probe(width: u32, height: u32, pixels: &[u8]) -> bool {
    let Some(expected_len) = PresentedFrame::expected_bytes(width, height) else {
        return false;
    };
    if pixels.len() != expected_len {
        return false;
    }
    pixels == cpu_probe_rgba8(width, height).as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_pattern_is_deterministic() {
        let a = cpu_probe_rgba8(4, 3);
        let b = cpu_probe_rgba8(4, 3);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4 * 3 * 4);
        assert_eq!(a[0..4], [0, 0, 0, 255]);
        assert!(rgba8_matches_cpu_probe(4, 3, &a));
        let c = cpu_probe_corners(4, 3).unwrap();
        assert_eq!(c.top_left, [0, 0, 0, 255]);
        assert_eq!(c.top_right, [3, 0, 3, 255]);
        assert_eq!(c.bottom_left, [0, 2, 2, 255]);
        assert_eq!(c.bottom_right, [3, 2, 5, 255]);
        assert_eq!(rgba8_corners(4, 3, &a), Some(c));
    }

    #[test]
    fn one_by_one_corners_collapse() {
        let c = cpu_probe_corners(1, 1).unwrap();
        assert_eq!(c.top_left, [0, 0, 0, 255]);
        assert_eq!(c.top_left, c.top_right);
        assert_eq!(c.top_left, c.bottom_right);
    }
}

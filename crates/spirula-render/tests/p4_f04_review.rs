//! Independent analytic cases from the pinned Primitive3DGS alpha equation.
//! These expectations do not call the CPU raster evaluator to derive answers.
use spirula_render::{
    CpuSortRaster, GpuSortRaster, ProjectedTileSplat, SortRasterOptions, TILE_CONTRACT_VERSION,
    TileBins, TileCoverage, TilePair,
};

fn full_tile_fixture(width: u32, height: u32, depths: &[f64]) -> TileBins {
    let tiles_x = width.div_ceil(8);
    let tiles_y = height.div_ceil(8);
    let tile_count = (tiles_x * tiles_y) as usize;
    let projected = depths
        .iter()
        .enumerate()
        .map(|(source_index, &depth)| {
            Some(ProjectedTileSplat {
                source_index,
                pixel: [0.5, 0.5],
                depth,
                ray_depth: depth,
                radius_px: [64.0, 64.0],
                covariance: [2.0, 0.5, 1.0],
                opacity: 0.8,
                tile_bounds: [0, 0, tiles_x, tiles_y],
            })
        })
        .collect();
    let bins = TileBins {
        contract_version: TILE_CONTRACT_VERSION,
        projection_alpha_cutoff: 1e-4,
        width,
        height,
        tile_size: 8,
        tiles_x,
        tiles_y,
        projected,
        coverage: (0..depths.len())
            .map(|source_index| TileCoverage {
                source_index,
                pair_offset: source_index * tile_count,
                pair_count: tile_count,
            })
            .collect(),
        pairs: (0..depths.len())
            .flat_map(|source_index| {
                (0..tile_count).map(move |tile_id| TilePair {
                    source_index,
                    tile_id,
                })
            })
            .collect(),
        inclusive_offsets: (1..=depths.len())
            .map(|n| (n * tile_count) as u32)
            .collect(),
    };
    bins.validate().unwrap();
    bins
}

#[test]
fn gpu_cross_workgroup_stability_and_raster_match_analytic_oracle() {
    let mut context =
        match spirula_gpu::GpuContext::open(&spirula_gpu::CapabilityRequest::diagnose_default()) {
            Ok(context) => context,
            Err(failure) => {
                eprintln!("sort_raster_review_gpu_unavailable: {failure:?}");
                assert!(std::env::var_os("SPIRULA_REQUIRE_RENDER_GPU").is_none());
                return;
            }
        };
    let mut depths: Vec<f64> = (0..67).map(|source| (source % 5 + 1) as f64).collect();
    depths[0] = 1.0 + 1e-9;
    depths[1] = 1.0;
    let bins = full_tile_fixture(17, 9, &depths);
    let (output, metrics) =
        GpuSortRaster::build(&mut context, &bins, 0.01, SortRasterOptions::default()).unwrap();
    // Five known positive depth buckets; preserve original source order in each.
    let expected: Vec<u32> = (1..=5)
        .flat_map(|depth| {
            (0..67).filter(move |&source| {
                let bucket = if source <= 1 { 1 } else { source % 5 + 1 };
                bucket == depth
            })
        })
        .collect();
    for range in &output.tile_ranges {
        let pairs = &output.pairs[range.start as usize..(range.start + range.count) as usize];
        assert_eq!(
            pairs
                .iter()
                .map(|pair| pair.source_index)
                .collect::<Vec<_>>(),
            expected
        );
    }
    for (index, pixel) in output.pixels.iter().enumerate() {
        let x = (index % 17) as f64;
        let y = (index / 17) as f64;
        let sigma = (2.0 * x * x - 2.0 * x * y + 4.0 * y * y) / 7.0;
        let alpha = 0.8 * (-sigma).exp();
        if alpha > 0.01 {
            assert_eq!(pixel.count, 67, "pixel={index}");
            assert_eq!(pixel.first_source, 0, "pixel={index}");
            assert!((pixel.max_alpha - alpha).abs() < 2e-6, "pixel={index}");
        } else {
            assert_eq!(pixel.count, 0, "pixel={index}");
            assert_eq!(pixel.first_source, u32::MAX);
        }
    }
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
    assert!(context.resource_accounting().peak_bytes <= metrics.accounted_peak_bound_bytes);
    eprintln!("independent_sort_raster_review sources=67 pairs=402 pixels=153 metrics={metrics:?}");

    let allocated = context.resource_accounting().allocated_bytes;
    let short_budget = SortRasterOptions {
        max_gpu_bytes: metrics.accounted_peak_bound_bytes - 1,
        ..SortRasterOptions::default()
    };
    assert_eq!(
        GpuSortRaster::build(&mut context, &bins, 0.01, short_budget)
            .unwrap_err()
            .code(),
        spirula_core::ErrorCode::AllocationFailed
    );
    assert_eq!(context.resource_accounting().allocated_bytes, allocated);

    // The ordinary execution case uses the exact same requested threshold at
    // projection and raster stages. A f32 round-down must not invalidate it.
    let (matching_cutoff, _) = GpuSortRaster::build(
        &mut context,
        &bins,
        bins.projection_alpha_cutoff,
        SortRasterOptions::default(),
    )
    .unwrap();
    assert_eq!(matching_cutoff.alpha_cutoff, bins.projection_alpha_cutoff);

    let mut threshold_bins = full_tile_fixture(2, 1, &[1.0]);
    threshold_bins.projection_alpha_cutoff = 0.5;
    threshold_bins.projected[0].as_mut().unwrap().opacity = 0.5;
    let (threshold, _) = GpuSortRaster::build(
        &mut context,
        &threshold_bins,
        0.5,
        SortRasterOptions::default(),
    )
    .unwrap();
    assert!(threshold.pixels.iter().all(|pixel| pixel.count == 0));
    assert!(
        GpuSortRaster::build(
            &mut context,
            &threshold_bins,
            0.25,
            SortRasterOptions::default(),
        )
        .is_err()
    );
    threshold_bins.projected[0].as_mut().unwrap().opacity = 1.0;
    let (clamped, _) = GpuSortRaster::build(
        &mut context,
        &threshold_bins,
        0.5,
        SortRasterOptions::default(),
    )
    .unwrap();
    assert_eq!(clamped.pixels[0].count, 1);
    assert!((clamped.pixels[0].max_alpha - 0.999).abs() < 1e-7);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);

    // Exercise every nibble of the low 32-bit key across several workgroups,
    // using positive normal floats over the full exponent range. Verify the
    // numeric order directly, without deriving an expected order via CPU sort.
    let mut random = 0x37b4_592du32;
    let varied_depths: Vec<f64> = (0..257)
        .map(|_| {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let bits = 0x0080_0000 | (random & 0x7eff_ffff);
            f64::from(f32::from_bits(bits))
        })
        .collect();
    let varied = full_tile_fixture(1, 1, &varied_depths);
    let (sorted, _) =
        GpuSortRaster::build(&mut context, &varied, 0.01, SortRasterOptions::default()).unwrap();
    assert_eq!(sorted.pairs.len(), 257);
    for adjacent in sorted.pairs.windows(2) {
        let left = adjacent[0].source_index as usize;
        let right = adjacent[1].source_index as usize;
        assert!(
            varied_depths[left] < varied_depths[right]
                || (varied_depths[left] == varied_depths[right] && left < right)
        );
    }
    assert_eq!(sorted.pixels[0].count, 257);
    assert_eq!(sorted.pixels[0].first_source, sorted.pairs[0].source_index);
    assert_eq!(context.resource_accounting().current_bytes, 0);
    assert_eq!(context.retained_submission_count(), 0);
}

#[test]
fn canonical_float_ties_preserve_original_order_in_every_tile() {
    // Source 0 is further away in f64 but rounds to exactly the same f32 as 1.
    // A f64 comparison would incorrectly reverse these two sources.
    let bins = full_tile_fixture(17, 9, &[1.0 + 1e-9, 1.0, 0.5, 4.0]);
    let output = CpuSortRaster::build(&bins, 0.01, SortRasterOptions::default()).unwrap();
    for (tile, range) in output.tile_ranges.iter().enumerate() {
        assert_eq!(range.count, 4);
        let pairs = &output.pairs[range.start as usize..(range.start + range.count) as usize];
        assert_eq!(
            pairs
                .iter()
                .map(|pair| pair.source_index)
                .collect::<Vec<_>>(),
            [2, 0, 1, 3],
            "tile={tile}"
        );
        assert!(pairs.iter().all(|pair| pair.tile_id as usize == tile));
    }
    assert_eq!(output.pixels.len(), 153);
}

#[test]
fn rotated_conic_matches_hand_derived_pixel_alpha() {
    let bins = full_tile_fixture(3, 2, &[1.0]);
    let output = CpuSortRaster::build(&bins, 0.01, SortRasterOptions::default()).unwrap();
    // Covariance inverse = [4/7,-2/7; -2/7,8/7]. At offsets
    // (0,0), (1,0), (2,0), (0,1), (1,1), (2,1), sigma is below.
    for (pixel, sigma) in output.pixels.iter().zip([
        0.0_f64,
        2.0 / 7.0,
        8.0 / 7.0,
        4.0 / 7.0,
        4.0 / 7.0,
        8.0 / 7.0,
    ]) {
        assert_eq!(pixel.count, 1);
        assert_eq!(pixel.first_source, 0);
        assert!((pixel.max_alpha - 0.8 * (-sigma).exp()).abs() < 1e-14);
    }
}

#[test]
fn cutoff_equality_is_excluded_and_center_alpha_is_clamped() {
    let mut bins = full_tile_fixture(2, 1, &[1.0]);
    let projected = bins.projected[0].as_mut().unwrap();
    projected.covariance = [1.0, 0.0, 1.0];
    projected.opacity = 0.5;
    let output = CpuSortRaster::build(&bins, 0.5, SortRasterOptions::default()).unwrap();
    assert!(output.pixels.iter().all(|pixel| pixel.count == 0));

    bins.projected[0].as_mut().unwrap().opacity = 1.0;
    let output = CpuSortRaster::build(&bins, 0.5, SortRasterOptions::default()).unwrap();
    assert_eq!(output.pixels[0].count, 1);
    assert_eq!(output.pixels[0].max_alpha, 0.999);
    assert!((output.pixels[1].max_alpha - (-0.5_f64).exp()).abs() < 1e-14);
}

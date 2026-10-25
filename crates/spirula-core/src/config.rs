//! Typed training-config table transcribed from the pinned reference
//! `src/config/TrainConfig.h` `SS_CONFIG_FIELDS` table.
//!
//! JSON load and save rules are in `docs/phase-1/config.md`.

/// Value kind of one `SS_CONFIG_FIELDS` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldType {
    String,
    Int,
    Bool,
    Float,
    OptionalBool,
    OptionalFloat,
    OptionalInt,
    Vec3Int,
    Vec3Float,
}

/// How the reference `choices` cell constrains a string field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChoiceKind {
    /// Empty `choices`: any string.
    Free,
    /// `choices` is exactly `none`: any string, and empty displays as none.
    FreeForm,
    /// `<folder>`, `<file>` or `<data>/...`: any path string.
    Path,
    /// Pipe-separated tokens. Empty is valid only when [`FieldSpec::empty_is_unset`].
    Closed(&'static [&'static str]),
}

/// One row of the reference field table, in table order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub value_type: FieldType,
    pub section: &'static str,
    pub tier: &'static str,
    pub choices: &'static str,
    pub choice_kind: ChoiceKind,
    /// Closed-set empty string is the reference unset default, not an unknown enum.
    pub empty_is_unset: bool,
}

/// Number of rows in [`TRAIN_CONFIG_FIELDS`].
pub const TRAIN_CONFIG_FIELD_COUNT: usize = 251;

/// Section headings in reference listing order. Presentational only.
pub const TRAIN_CONFIG_SECTIONS: &[&str] = &[
    "run",
    "dataset",
    "scene",
    "splats",
    "detail",
    "loss",
    "geometry",
    "shape",
    "correction",
    "colorspace",
    "perf",
    "rates",
];

/// Specialist tiers, least specialist first.
pub const TRAIN_CONFIG_TIERS: &[&str] = &["basic", "advanced", "expert", "stub"];

const CLOSED_15: &[&str] = &["colmap", "nerfstudio", "metashape", "none"];
const CLOSED_42: &[&str] = &["floor", "ceil", "round"];
const CLOSED_49: &[&str] = &["fraction", "filename", "interval", "all"];
const CLOSED_55: &[&str] = &["uniform", "per-face"];
const CLOSED_58: &[&str] = &["none", "orient", "apply"];
const CLOSED_59: &[&str] = &["pca", "up", "vertical", "none", "gsplat"];
const CLOSED_60: &[&str] = &["poses", "focus", "none", "gsplat"];
const CLOSED_63: &[&str] = &[
    "auto",
    "none",
    "point-median",
    "camera-median",
    "camera-focus",
    "point-mean",
    "camera-mean",
];
const CLOSED_66: &[&str] = &["normalized", "camera", "points"];
const CLOSED_67: &[&str] = &["3dgs", "mip", "3dgut"];
const CLOSED_71: &[&str] = &["color", "noise", "pseudorandom", "random", "sh"];
const CLOSED_80: &[&str] = &["never", "auto", "always"];
const CLOSED_82: &[&str] = &[
    "isotropic-gaussian",
    "anisotropic-gaussian",
    "ellipsoid",
    "box",
];
const CLOSED_83: &[&str] = &["camera-median", "camera-focus", "camera-mean", "origin"];
const CLOSED_84: &[&str] = &["median", "mean"];
const CLOSED_87: &[&str] = &["low", "medium", "high", "ultra"];
const CLOSED_89: &[&str] = &["off", "mild", "strong"];
const CLOSED_93: &[&str] = &["opacity", "max_weight", "either"];
const CLOSED_101: &[&str] = &["mean", "max", "median", "geom"];
const CLOSED_103: &[&str] = &[
    "auto",
    "none",
    "loss_full",
    "ssim_full",
    "ssim_cs",
    "ssim_structure",
    "s_cielab",
    "flip",
    "flip_feature",
    "edge_aware",
    "robust_edge_aware",
    "loss_full_nms",
    "ssim_full_nms",
    "ssim_cs_nms",
    "ssim_structure_nms",
    "s_cielab_nms",
    "flip_nms",
    "flip_feature_nms",
];
const CLOSED_109: &[&str] = &["max", "sum", "avg"];
const CLOSED_126: &[&str] = &["ssim", "ssim_blur_tolerant", "s_cielab", "flip"];
const CLOSED_139: &[&str] = &["uniform", "deficit"];
const CLOSED_142: &[&str] = &["off", "mild", "strong"];
const CLOSED_177: &[&str] = &["affine", "ppisp", "loglinear"];
const CLOSED_187: &[&str] = &[
    "original",
    "rqs",
    "no_crf",
    "no_crf_clamp",
    "no_crf_no_vig",
    "no_crf_no_vig_clamp",
];
const CLOSED_216: &[&str] = &[
    "srgb",
    "srgb-clamped",
    "aces",
    "filmic",
    "uncharted2",
    "none",
];
const CLOSED_217: &[&str] = &[
    "Rec.709",
    "ACES2065-1",
    "ACEScg",
    "Rec.2020",
    "AdobeRGB",
    "DCI-P3",
    "none",
];
const CLOSED_219: &[&str] = &[
    "srgb",
    "srgb-clamped",
    "aces",
    "filmic",
    "uncharted2",
    "none",
];
const CLOSED_220: &[&str] = &[
    "Rec.709",
    "ACES2065-1",
    "ACEScg",
    "Rec.2020",
    "AdobeRGB",
    "DCI-P3",
    "none",
];
const CLOSED_222: &[&str] = &[
    "srgb",
    "srgb-clamped",
    "aces",
    "filmic",
    "uncharted2",
    "none",
];
const CLOSED_223: &[&str] = &[
    "Rec.709",
    "ACES2065-1",
    "ACEScg",
    "Rec.2020",
    "AdobeRGB",
    "DCI-P3",
    "none",
];
const CLOSED_224: &[&str] = &["cpu", "gpu", "disk"];
const CLOSED_235: &[&str] = &["sh", "all", "none"];

/// Every training flag. JSON keys are these names, flat, in this order.
#[derive(Clone, Debug)]
pub struct TrainConfig {
    pub data: String,
    pub resume: String,
    pub output_dir_prefix: String,
    pub output_dir_name: String,
    pub num_iterations: i32,
    pub steps_per_save: i32,
    pub save_only_latest_checkpoint: bool,
    pub save_full_checkpoint: bool,
    pub save_eval_images: bool,
    pub eval_lpips: bool,
    pub lpips_checkpoints: String,
    pub log_performance: bool,
    pub viewer_port: i32,
    pub disable_viewer: bool,
    pub keep_viewer_alive: bool,
    pub data_format: String,
    pub image_dir: String,
    pub mask_dir: String,
    pub load_masks: bool,
    pub apply_loss_for_mask: Option<bool>,
    pub flip_mask: bool,
    pub mask_boundary_offset: f32,
    pub depth_dir: String,
    pub normal_dir: String,
    pub load_depths: bool,
    pub load_normals: bool,
    pub depth_unit_scale_factor: f32,
    pub colmap_recon_dir: String,
    pub metashape_xml: String,
    pub metashape_ply: String,
    pub metashape_psx: String,
    pub seed_pointcloud: String,
    pub init_ply: String,
    pub init_ply_add_points: bool,
    pub partition: String,
    pub partition_part: i32,
    pub roi_region: String,
    pub roi_outside_weight: f32,
    pub roi_outside_opacity_decay: f32,
    pub roi_splat_samples: i32,
    pub roi_mask_pixels: bool,
    pub train_resolution_divisor: f32,
    pub downscale_rounding_mode: String,
    pub progressive_resolution: bool,
    pub progressive_resolution_start: i32,
    pub progressive_resolution_full_at: f32,
    pub progressive_resolution_schedule: String,
    pub progressive_splat_budget: bool,
    pub progressive_splat_budget_schedule: String,
    pub eval_mode: String,
    pub eval_interval: i32,
    pub train_split_fraction: f32,
    pub validation_fraction: f32,
    pub warp_to_pinhole: bool,
    pub warp_spherical_to_pinhole: bool,
    pub warp_face_fit: String,
    pub warp_back_face: bool,
    pub deblur_training_images: bool,
    pub exif_orientation: String,
    pub orientation_method: String,
    pub center_method: String,
    pub auto_scale_poses: bool,
    pub outlier_threshold: f32,
    pub scene_center: String,
    pub scene_center_threshold: f32,
    pub relative_scale: Option<f32>,
    pub train_frame: String,
    pub primitive: String,
    pub sh_degree: i32,
    pub sb_lobes: i32,
    pub sh_degree_warmup_every: i32,
    pub background_mode: String,
    pub background_color: [f32; 3],
    pub background_sh_degree: i32,
    pub background_noise_warmup: i32,
    pub background_noise_pre_warmup: f32,
    pub background_match_luminance: bool,
    pub scale_init: Option<f32>,
    pub opacity_init: Option<f32>,
    pub suppress_initial_scales: bool,
    pub random_init: String,
    pub random_init_fraction: f32,
    pub random_init_distribution: String,
    pub random_init_center: String,
    pub random_init_spread: String,
    pub random_init_std: f32,
    pub use_camera_optimizer: bool,
    pub quality: String,
    pub cap_max: i32,
    pub distraction_robustness: String,
    pub min_init_fraction: f32,
    pub growth_factor: f32,
    pub min_opacity: f32,
    pub dead_criterion: String,
    pub refine_every: i32,
    pub refine_start_iter: i32,
    pub refine_stop_num_iter: i32,
    pub refine_stop_iter: i32,
    pub noise_lr: f32,
    pub noise_lr_final: f32,
    pub use_revised_densification: bool,
    pub densify_score_mode: String,
    pub densify_score_blend_world_grad: f32,
    pub densify_loss_map_mode: String,
    pub densify_robust_edge_aware_quantile: f32,
    pub densify_nms_falloff: f32,
    pub densify_loss_map_normalize: bool,
    pub densify_loss_map_clip_quantile: f32,
    pub densify_loss_map_power: f32,
    pub densify_accum_mode: String,
    pub densify_score_power: f32,
    pub densify_score_clip_quantile: f32,
    pub densify_final_score_power: f32,
    pub densify_learned_score: bool,
    pub densify_oversize_split_fraction: f32,
    pub densify_oversize_score_blend: f32,
    pub use_long_axis_split: bool,
    pub long_axis_split_opacity_k: [f32; 3],
    pub max_split_fraction: f32,
    pub split_weight_by_renders: bool,
    pub dead_after_epochs: f32,
    pub max_screen_size: f32,
    pub max_screen_size_clip_hardness: f32,
    pub max_screen_size_penalty: f32,
    pub max_world_size: f32,
    pub ssim_lambda: f32,
    pub perceptual_loss: String,
    pub l1_weight: f32,
    pub l2_weight: f32,
    pub l1_weight_y: f32,
    pub l2_weight_y: f32,
    pub l2_weight_u: f32,
    pub l2_weight_v: f32,
    pub loss_scale_min_pixels: i32,
    pub num_loss_scales: i32,
    pub alpha_loss_weight: f32,
    pub alpha_loss_weight_under: f32,
    pub loss_saturation_threshold: f32,
    pub loss_luminance_normalization: f32,
    pub view_sampling: String,
    pub view_deficit_power: f32,
    pub view_deficit_max_ratio: f32,
    pub floater_suppression: String,
    pub depth_distortion_reg: f32,
    pub normal_distortion_reg: f32,
    pub rgb_distortion_reg: f32,
    pub distortion_reg_warmup: i32,
    pub normal_reg_weight: f32,
    pub normal_reg_warmup: i32,
    pub alpha_reg_weight: f32,
    pub alpha_reg_warmup: i32,
    pub reg_warmup_length: i32,
    pub depth_supervision_weight: f32,
    pub normal_supervision_weight: f32,
    pub input_depth_is_ray_depth: Option<bool>,
    pub supervision_warmup: i32,
    pub mean_median_depth_weight: f32,
    pub median_depth_normal_reg_weight: f32,
    pub median_normal_supervision_weight: f32,
    pub median_render_normal_reg_weight: f32,
    pub median_warmup: i32,
    pub opacity_reg: f32,
    pub reg_rendered_only: bool,
    pub scale_reg: f32,
    pub opacity_reg_decay_power: f32,
    pub scale_reg_decay_power: f32,
    pub opacity_decay: f32,
    pub scale_decay: f32,
    pub erank_reg: f32,
    pub erank_reg_s3: f32,
    pub scale_regularization_weight: f32,
    pub max_gauss_ratio: f32,
    pub dc_reg: f32,
    pub sh_reg: f32,
    pub overexposure_reg: f32,
    pub quat_norm_reg: f32,
    pub use_bilateral_grid: bool,
    pub bilagrid_type: String,
    pub bilagrid_shape: [i32; 3],
    pub bilagrid_tv_loss_weight: f32,
    pub color_shift_reg_weight: f32,
    pub color_shift_reg_ema_period: i32,
    pub use_bilateral_grid_for_geometry: bool,
    pub bilagrid_shape_geometry: [i32; 3],
    pub bilagrid_tv_loss_weight_geometry: f32,
    pub use_adagrad_bilagrid_optim: bool,
    pub use_ppisp: bool,
    pub ppisp_param_type: String,
    pub ppisp_exposure_from_exif: bool,
    pub ppisp_exposure_arithmetic_mean: bool,
    pub apply_ppisp_before_bilagrid: bool,
    pub apply_ppisp_before_color_space: bool,
    pub use_adagrad_ppisp_optim: bool,
    pub ppisp_reg_exposure_mean: f32,
    pub ppisp_reg_color_mean: f32,
    pub ppisp_reg_vig_center: f32,
    pub ppisp_reg_vig_non_pos: f32,
    pub ppisp_reg_vig_channel_var: f32,
    pub ppisp_reg_crf_channel_var: f32,
    pub bilagrid_adagrad_lr: f32,
    pub bilagrid_adagrad_depth_lr: f32,
    pub bilagrid_adagrad_normal_lr: f32,
    pub ppisp_adagrad_lr: f32,
    pub bilagrid_lr: f32,
    pub bilagrid_lr_final: Option<f32>,
    pub bilagrid_lr_warmup: i32,
    pub bilagrid_depth_lr: f32,
    pub bilagrid_depth_lr_final: Option<f32>,
    pub bilagrid_depth_lr_warmup: i32,
    pub bilagrid_normal_lr: f32,
    pub bilagrid_normal_lr_final: Option<f32>,
    pub bilagrid_normal_lr_warmup: i32,
    pub ppisp_lr: f32,
    pub ppisp_lr_final: Option<f32>,
    pub ppisp_lr_warmup: i32,
    pub image_color_is_linear: Option<bool>,
    pub image_color_transfer: String,
    pub image_color_gamut: String,
    pub splat_color_is_linear: Option<bool>,
    pub splat_color_transfer: String,
    pub splat_color_gamut: String,
    pub point_color_is_linear: Option<bool>,
    pub point_color_transfer: String,
    pub point_color_gamut: String,
    pub cache_images: String,
    pub max_batch_per_epoch: i32,
    pub min_renders_per_refine: f32,
    pub render_quantile: f32,
    pub max_train_batch_size: i32,
    pub split_batch: bool,
    pub use_fused_proj_bwd_optim: bool,
    pub packed: bool,
    pub quantization_level: i32,
    pub bin_tile_size: i32,
    pub preallocate_splat_tensors: bool,
    pub optimizer_offload: String,
    pub use_bvh: bool,
    pub means_lr: f32,
    pub means_lr_final: Option<f32>,
    pub scales_lr: f32,
    pub scales_lr_final: Option<f32>,
    pub quats_lr: f32,
    pub opacities_lr: f32,
    pub features_dc_lr: f32,
    pub features_sh_lr: f32,
    pub features_sb_lr: f32,
    pub background_dc_lr: f32,
    pub background_sh_lr: f32,
    pub max_steps: Option<i32>,
    pub use_scale_agnostic_mean: bool,
    pub use_per_splat_bias_correction: bool,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            data: String::new(),
            resume: String::new(),
            output_dir_prefix: "outputs".to_owned(),
            output_dir_name: String::new(),
            num_iterations: 30000,
            steps_per_save: 2000,
            save_only_latest_checkpoint: true,
            save_full_checkpoint: false,
            save_eval_images: false,
            eval_lpips: true,
            lpips_checkpoints: String::new(),
            log_performance: false,
            viewer_port: 7007,
            disable_viewer: false,
            keep_viewer_alive: true,
            data_format: String::new(),
            image_dir: "images".to_owned(),
            mask_dir: "masks".to_owned(),
            load_masks: true,
            apply_loss_for_mask: None,
            flip_mask: false,
            mask_boundary_offset: 0.0f32,
            depth_dir: "depths".to_owned(),
            normal_dir: "normals".to_owned(),
            load_depths: true,
            load_normals: true,
            depth_unit_scale_factor: 0.001f32,
            colmap_recon_dir: String::new(),
            metashape_xml: String::new(),
            metashape_ply: String::new(),
            metashape_psx: String::new(),
            seed_pointcloud: String::new(),
            init_ply: String::new(),
            init_ply_add_points: false,
            partition: String::new(),
            partition_part: -1,
            roi_region: String::new(),
            roi_outside_weight: 1e-4f32,
            roi_outside_opacity_decay: 1.0f32,
            roi_splat_samples: 4,
            roi_mask_pixels: true,
            train_resolution_divisor: 0.0f32,
            downscale_rounding_mode: "floor".to_owned(),
            progressive_resolution: false,
            progressive_resolution_start: 4,
            progressive_resolution_full_at: 0.3f32,
            progressive_resolution_schedule: String::new(),
            progressive_splat_budget: true,
            progressive_splat_budget_schedule: String::new(),
            eval_mode: "all".to_owned(),
            eval_interval: 8,
            train_split_fraction: 0.9f32,
            validation_fraction: 0.0f32,
            warp_to_pinhole: false,
            warp_spherical_to_pinhole: true,
            warp_face_fit: "uniform".to_owned(),
            warp_back_face: false,
            deblur_training_images: false,
            exif_orientation: "orient".to_owned(),
            orientation_method: "up".to_owned(),
            center_method: "poses".to_owned(),
            auto_scale_poses: true,
            outlier_threshold: f32::INFINITY,
            scene_center: "auto".to_owned(),
            scene_center_threshold: 20.0f32,
            relative_scale: None,
            train_frame: "points".to_owned(),
            primitive: "3dgs".to_owned(),
            sh_degree: 3,
            sb_lobes: 0,
            sh_degree_warmup_every: 0,
            background_mode: "color".to_owned(),
            background_color: [0.0f32, 0.0f32, 0.0f32],
            background_sh_degree: 4,
            background_noise_warmup: 2000,
            background_noise_pre_warmup: 0.25f32,
            background_match_luminance: false,
            scale_init: Some(0.35f32),
            opacity_init: None,
            suppress_initial_scales: false,
            random_init: "auto".to_owned(),
            random_init_fraction: 0.1f32,
            random_init_distribution: "isotropic-gaussian".to_owned(),
            random_init_center: "camera-median".to_owned(),
            random_init_spread: "median".to_owned(),
            random_init_std: 1.0f32,
            use_camera_optimizer: false,
            quality: "medium".to_owned(),
            cap_max: 1000000,
            distraction_robustness: "off".to_owned(),
            min_init_fraction: 0.1f32,
            growth_factor: 1.05f32,
            min_opacity: 0.005f32,
            dead_criterion: "either".to_owned(),
            refine_every: 100,
            refine_start_iter: 500,
            refine_stop_num_iter: 2500,
            refine_stop_iter: 14000,
            noise_lr: 160.0f32,
            noise_lr_final: 1.6f32,
            use_revised_densification: true,
            densify_score_mode: "mean".to_owned(),
            densify_score_blend_world_grad: 0.0f32,
            densify_loss_map_mode: "auto".to_owned(),
            densify_robust_edge_aware_quantile: 0.9f32,
            densify_nms_falloff: 0.5f32,
            densify_loss_map_normalize: false,
            densify_loss_map_clip_quantile: 1.0f32,
            densify_loss_map_power: 2.0f32,
            densify_accum_mode: "sum".to_owned(),
            densify_score_power: 0.2f32,
            densify_score_clip_quantile: 1.0f32,
            densify_final_score_power: 1.0f32,
            densify_learned_score: false,
            densify_oversize_split_fraction: 0.15f32,
            densify_oversize_score_blend: 1.0f32,
            use_long_axis_split: true,
            long_axis_split_opacity_k: [0.5f32, 0.55f32, 15000.0f32],
            max_split_fraction: 0.1f32,
            split_weight_by_renders: false,
            dead_after_epochs: 2.0f32,
            max_screen_size: 0.3f32,
            max_screen_size_clip_hardness: 1.5f32,
            max_screen_size_penalty: 1.0f32,
            max_world_size: f32::INFINITY,
            ssim_lambda: 0.2f32,
            perceptual_loss: "ssim_blur_tolerant".to_owned(),
            l1_weight: 1.0f32,
            l2_weight: 0.0f32,
            l1_weight_y: 0.0f32,
            l2_weight_y: 0.0f32,
            l2_weight_u: 0.0f32,
            l2_weight_v: 0.0f32,
            loss_scale_min_pixels: 1920,
            num_loss_scales: 0,
            alpha_loss_weight: 0.5f32,
            alpha_loss_weight_under: 0.0f32,
            loss_saturation_threshold: -1.0f32,
            loss_luminance_normalization: 0.0f32,
            view_sampling: "uniform".to_owned(),
            view_deficit_power: 0.5f32,
            view_deficit_max_ratio: 8.0f32,
            floater_suppression: "off".to_owned(),
            depth_distortion_reg: 0.0f32,
            normal_distortion_reg: 0.0f32,
            rgb_distortion_reg: 0.0f32,
            distortion_reg_warmup: 6000,
            normal_reg_weight: 0.04f32,
            normal_reg_warmup: 6000,
            alpha_reg_weight: 0.0f32,
            alpha_reg_warmup: 12000,
            reg_warmup_length: 0,
            depth_supervision_weight: 0.0f32,
            normal_supervision_weight: 0.01f32,
            input_depth_is_ray_depth: None,
            supervision_warmup: 0,
            mean_median_depth_weight: 0.0f32,
            median_depth_normal_reg_weight: 0.0f32,
            median_normal_supervision_weight: 0.0f32,
            median_render_normal_reg_weight: 0.0f32,
            median_warmup: 6000,
            opacity_reg: 0.01f32,
            reg_rendered_only: true,
            scale_reg: 0.01f32,
            opacity_reg_decay_power: 1.0f32,
            scale_reg_decay_power: 0.4f32,
            opacity_decay: 0.0f32,
            scale_decay: 0.0f32,
            erank_reg: 0.001f32,
            erank_reg_s3: 0.0f32,
            scale_regularization_weight: 0.0f32,
            max_gauss_ratio: 10.0f32,
            dc_reg: 0.001f32,
            sh_reg: 0.001f32,
            overexposure_reg: 0.0f32,
            quat_norm_reg: 0.01f32,
            use_bilateral_grid: true,
            bilagrid_type: "ppisp".to_owned(),
            bilagrid_shape: [16, 16, 8],
            bilagrid_tv_loss_weight: 10.0f32,
            color_shift_reg_weight: 0.0f32,
            color_shift_reg_ema_period: 750,
            use_bilateral_grid_for_geometry: true,
            bilagrid_shape_geometry: [8, 8, 4],
            bilagrid_tv_loss_weight_geometry: 10.0f32,
            use_adagrad_bilagrid_optim: true,
            use_ppisp: true,
            ppisp_param_type: "no_crf_no_vig".to_owned(),
            ppisp_exposure_from_exif: false,
            ppisp_exposure_arithmetic_mean: true,
            apply_ppisp_before_bilagrid: true,
            apply_ppisp_before_color_space: false,
            use_adagrad_ppisp_optim: true,
            ppisp_reg_exposure_mean: 1.0f32,
            ppisp_reg_color_mean: 1.0f32,
            ppisp_reg_vig_center: 0.02f32,
            ppisp_reg_vig_non_pos: 0.01f32,
            ppisp_reg_vig_channel_var: 0.1f32,
            ppisp_reg_crf_channel_var: 0.1f32,
            bilagrid_adagrad_lr: 0.04f32,
            bilagrid_adagrad_depth_lr: 0.04f32,
            bilagrid_adagrad_normal_lr: 0.01f32,
            ppisp_adagrad_lr: 0.1f32,
            bilagrid_lr: 0.002f32,
            bilagrid_lr_final: Some(0.0001f32),
            bilagrid_lr_warmup: 1000,
            bilagrid_depth_lr: 0.002f32,
            bilagrid_depth_lr_final: Some(0.0001f32),
            bilagrid_depth_lr_warmup: 2000,
            bilagrid_normal_lr: 0.0005f32,
            bilagrid_normal_lr_final: Some(4e-05f32),
            bilagrid_normal_lr_warmup: 2000,
            ppisp_lr: 0.002f32,
            ppisp_lr_final: Some(2e-05f32),
            ppisp_lr_warmup: 500,
            image_color_is_linear: None,
            image_color_transfer: String::new(),
            image_color_gamut: String::new(),
            splat_color_is_linear: None,
            splat_color_transfer: String::new(),
            splat_color_gamut: String::new(),
            point_color_is_linear: None,
            point_color_transfer: String::new(),
            point_color_gamut: String::new(),
            cache_images: "disk".to_owned(),
            max_batch_per_epoch: 800,
            min_renders_per_refine: 0.0f32,
            render_quantile: 0.1f32,
            max_train_batch_size: -1,
            split_batch: true,
            use_fused_proj_bwd_optim: true,
            packed: true,
            quantization_level: 1,
            bin_tile_size: 0,
            preallocate_splat_tensors: true,
            optimizer_offload: String::new(),
            use_bvh: false,
            means_lr: 0.000256f32,
            means_lr_final: Some(3.2e-06f32),
            scales_lr: 0.02f32,
            scales_lr_final: Some(0.005f32),
            quats_lr: 0.0015f32,
            opacities_lr: 0.025f32,
            features_dc_lr: 0.005f32,
            features_sh_lr: 0.00025f32,
            features_sb_lr: 0.0025f32,
            background_dc_lr: 0.0025f32,
            background_sh_lr: 0.0005f32,
            max_steps: None,
            use_scale_agnostic_mean: true,
            use_per_splat_bias_correction: true,
        }
    }
}

impl TrainConfig {
    /// Field-wise equality. NaN matches NaN so a loaded non-finite value can round-trip.
    pub fn same_values(&self, other: &Self) -> bool {
        true && self.data == other.data
            && self.resume == other.resume
            && self.output_dir_prefix == other.output_dir_prefix
            && self.output_dir_name == other.output_dir_name
            && self.num_iterations == other.num_iterations
            && self.steps_per_save == other.steps_per_save
            && self.save_only_latest_checkpoint == other.save_only_latest_checkpoint
            && self.save_full_checkpoint == other.save_full_checkpoint
            && self.save_eval_images == other.save_eval_images
            && self.eval_lpips == other.eval_lpips
            && self.lpips_checkpoints == other.lpips_checkpoints
            && self.log_performance == other.log_performance
            && self.viewer_port == other.viewer_port
            && self.disable_viewer == other.disable_viewer
            && self.keep_viewer_alive == other.keep_viewer_alive
            && self.data_format == other.data_format
            && self.image_dir == other.image_dir
            && self.mask_dir == other.mask_dir
            && self.load_masks == other.load_masks
            && self.apply_loss_for_mask == other.apply_loss_for_mask
            && self.flip_mask == other.flip_mask
            && f32_same(self.mask_boundary_offset, other.mask_boundary_offset)
            && self.depth_dir == other.depth_dir
            && self.normal_dir == other.normal_dir
            && self.load_depths == other.load_depths
            && self.load_normals == other.load_normals
            && f32_same(self.depth_unit_scale_factor, other.depth_unit_scale_factor)
            && self.colmap_recon_dir == other.colmap_recon_dir
            && self.metashape_xml == other.metashape_xml
            && self.metashape_ply == other.metashape_ply
            && self.metashape_psx == other.metashape_psx
            && self.seed_pointcloud == other.seed_pointcloud
            && self.init_ply == other.init_ply
            && self.init_ply_add_points == other.init_ply_add_points
            && self.partition == other.partition
            && self.partition_part == other.partition_part
            && self.roi_region == other.roi_region
            && f32_same(self.roi_outside_weight, other.roi_outside_weight)
            && f32_same(
                self.roi_outside_opacity_decay,
                other.roi_outside_opacity_decay,
            )
            && self.roi_splat_samples == other.roi_splat_samples
            && self.roi_mask_pixels == other.roi_mask_pixels
            && f32_same(
                self.train_resolution_divisor,
                other.train_resolution_divisor,
            )
            && self.downscale_rounding_mode == other.downscale_rounding_mode
            && self.progressive_resolution == other.progressive_resolution
            && self.progressive_resolution_start == other.progressive_resolution_start
            && f32_same(
                self.progressive_resolution_full_at,
                other.progressive_resolution_full_at,
            )
            && self.progressive_resolution_schedule == other.progressive_resolution_schedule
            && self.progressive_splat_budget == other.progressive_splat_budget
            && self.progressive_splat_budget_schedule == other.progressive_splat_budget_schedule
            && self.eval_mode == other.eval_mode
            && self.eval_interval == other.eval_interval
            && f32_same(self.train_split_fraction, other.train_split_fraction)
            && f32_same(self.validation_fraction, other.validation_fraction)
            && self.warp_to_pinhole == other.warp_to_pinhole
            && self.warp_spherical_to_pinhole == other.warp_spherical_to_pinhole
            && self.warp_face_fit == other.warp_face_fit
            && self.warp_back_face == other.warp_back_face
            && self.deblur_training_images == other.deblur_training_images
            && self.exif_orientation == other.exif_orientation
            && self.orientation_method == other.orientation_method
            && self.center_method == other.center_method
            && self.auto_scale_poses == other.auto_scale_poses
            && f32_same(self.outlier_threshold, other.outlier_threshold)
            && self.scene_center == other.scene_center
            && f32_same(self.scene_center_threshold, other.scene_center_threshold)
            && opt_f32_same(self.relative_scale, other.relative_scale)
            && self.train_frame == other.train_frame
            && self.primitive == other.primitive
            && self.sh_degree == other.sh_degree
            && self.sb_lobes == other.sb_lobes
            && self.sh_degree_warmup_every == other.sh_degree_warmup_every
            && self.background_mode == other.background_mode
            && self
                .background_color
                .iter()
                .zip(other.background_color)
                .all(|(left, right)| f32_same(*left, right))
            && self.background_sh_degree == other.background_sh_degree
            && self.background_noise_warmup == other.background_noise_warmup
            && f32_same(
                self.background_noise_pre_warmup,
                other.background_noise_pre_warmup,
            )
            && self.background_match_luminance == other.background_match_luminance
            && opt_f32_same(self.scale_init, other.scale_init)
            && opt_f32_same(self.opacity_init, other.opacity_init)
            && self.suppress_initial_scales == other.suppress_initial_scales
            && self.random_init == other.random_init
            && f32_same(self.random_init_fraction, other.random_init_fraction)
            && self.random_init_distribution == other.random_init_distribution
            && self.random_init_center == other.random_init_center
            && self.random_init_spread == other.random_init_spread
            && f32_same(self.random_init_std, other.random_init_std)
            && self.use_camera_optimizer == other.use_camera_optimizer
            && self.quality == other.quality
            && self.cap_max == other.cap_max
            && self.distraction_robustness == other.distraction_robustness
            && f32_same(self.min_init_fraction, other.min_init_fraction)
            && f32_same(self.growth_factor, other.growth_factor)
            && f32_same(self.min_opacity, other.min_opacity)
            && self.dead_criterion == other.dead_criterion
            && self.refine_every == other.refine_every
            && self.refine_start_iter == other.refine_start_iter
            && self.refine_stop_num_iter == other.refine_stop_num_iter
            && self.refine_stop_iter == other.refine_stop_iter
            && f32_same(self.noise_lr, other.noise_lr)
            && f32_same(self.noise_lr_final, other.noise_lr_final)
            && self.use_revised_densification == other.use_revised_densification
            && self.densify_score_mode == other.densify_score_mode
            && f32_same(
                self.densify_score_blend_world_grad,
                other.densify_score_blend_world_grad,
            )
            && self.densify_loss_map_mode == other.densify_loss_map_mode
            && f32_same(
                self.densify_robust_edge_aware_quantile,
                other.densify_robust_edge_aware_quantile,
            )
            && f32_same(self.densify_nms_falloff, other.densify_nms_falloff)
            && self.densify_loss_map_normalize == other.densify_loss_map_normalize
            && f32_same(
                self.densify_loss_map_clip_quantile,
                other.densify_loss_map_clip_quantile,
            )
            && f32_same(self.densify_loss_map_power, other.densify_loss_map_power)
            && self.densify_accum_mode == other.densify_accum_mode
            && f32_same(self.densify_score_power, other.densify_score_power)
            && f32_same(
                self.densify_score_clip_quantile,
                other.densify_score_clip_quantile,
            )
            && f32_same(
                self.densify_final_score_power,
                other.densify_final_score_power,
            )
            && self.densify_learned_score == other.densify_learned_score
            && f32_same(
                self.densify_oversize_split_fraction,
                other.densify_oversize_split_fraction,
            )
            && f32_same(
                self.densify_oversize_score_blend,
                other.densify_oversize_score_blend,
            )
            && self.use_long_axis_split == other.use_long_axis_split
            && self
                .long_axis_split_opacity_k
                .iter()
                .zip(other.long_axis_split_opacity_k)
                .all(|(left, right)| f32_same(*left, right))
            && f32_same(self.max_split_fraction, other.max_split_fraction)
            && self.split_weight_by_renders == other.split_weight_by_renders
            && f32_same(self.dead_after_epochs, other.dead_after_epochs)
            && f32_same(self.max_screen_size, other.max_screen_size)
            && f32_same(
                self.max_screen_size_clip_hardness,
                other.max_screen_size_clip_hardness,
            )
            && f32_same(self.max_screen_size_penalty, other.max_screen_size_penalty)
            && f32_same(self.max_world_size, other.max_world_size)
            && f32_same(self.ssim_lambda, other.ssim_lambda)
            && self.perceptual_loss == other.perceptual_loss
            && f32_same(self.l1_weight, other.l1_weight)
            && f32_same(self.l2_weight, other.l2_weight)
            && f32_same(self.l1_weight_y, other.l1_weight_y)
            && f32_same(self.l2_weight_y, other.l2_weight_y)
            && f32_same(self.l2_weight_u, other.l2_weight_u)
            && f32_same(self.l2_weight_v, other.l2_weight_v)
            && self.loss_scale_min_pixels == other.loss_scale_min_pixels
            && self.num_loss_scales == other.num_loss_scales
            && f32_same(self.alpha_loss_weight, other.alpha_loss_weight)
            && f32_same(self.alpha_loss_weight_under, other.alpha_loss_weight_under)
            && f32_same(
                self.loss_saturation_threshold,
                other.loss_saturation_threshold,
            )
            && f32_same(
                self.loss_luminance_normalization,
                other.loss_luminance_normalization,
            )
            && self.view_sampling == other.view_sampling
            && f32_same(self.view_deficit_power, other.view_deficit_power)
            && f32_same(self.view_deficit_max_ratio, other.view_deficit_max_ratio)
            && self.floater_suppression == other.floater_suppression
            && f32_same(self.depth_distortion_reg, other.depth_distortion_reg)
            && f32_same(self.normal_distortion_reg, other.normal_distortion_reg)
            && f32_same(self.rgb_distortion_reg, other.rgb_distortion_reg)
            && self.distortion_reg_warmup == other.distortion_reg_warmup
            && f32_same(self.normal_reg_weight, other.normal_reg_weight)
            && self.normal_reg_warmup == other.normal_reg_warmup
            && f32_same(self.alpha_reg_weight, other.alpha_reg_weight)
            && self.alpha_reg_warmup == other.alpha_reg_warmup
            && self.reg_warmup_length == other.reg_warmup_length
            && f32_same(
                self.depth_supervision_weight,
                other.depth_supervision_weight,
            )
            && f32_same(
                self.normal_supervision_weight,
                other.normal_supervision_weight,
            )
            && self.input_depth_is_ray_depth == other.input_depth_is_ray_depth
            && self.supervision_warmup == other.supervision_warmup
            && f32_same(
                self.mean_median_depth_weight,
                other.mean_median_depth_weight,
            )
            && f32_same(
                self.median_depth_normal_reg_weight,
                other.median_depth_normal_reg_weight,
            )
            && f32_same(
                self.median_normal_supervision_weight,
                other.median_normal_supervision_weight,
            )
            && f32_same(
                self.median_render_normal_reg_weight,
                other.median_render_normal_reg_weight,
            )
            && self.median_warmup == other.median_warmup
            && f32_same(self.opacity_reg, other.opacity_reg)
            && self.reg_rendered_only == other.reg_rendered_only
            && f32_same(self.scale_reg, other.scale_reg)
            && f32_same(self.opacity_reg_decay_power, other.opacity_reg_decay_power)
            && f32_same(self.scale_reg_decay_power, other.scale_reg_decay_power)
            && f32_same(self.opacity_decay, other.opacity_decay)
            && f32_same(self.scale_decay, other.scale_decay)
            && f32_same(self.erank_reg, other.erank_reg)
            && f32_same(self.erank_reg_s3, other.erank_reg_s3)
            && f32_same(
                self.scale_regularization_weight,
                other.scale_regularization_weight,
            )
            && f32_same(self.max_gauss_ratio, other.max_gauss_ratio)
            && f32_same(self.dc_reg, other.dc_reg)
            && f32_same(self.sh_reg, other.sh_reg)
            && f32_same(self.overexposure_reg, other.overexposure_reg)
            && f32_same(self.quat_norm_reg, other.quat_norm_reg)
            && self.use_bilateral_grid == other.use_bilateral_grid
            && self.bilagrid_type == other.bilagrid_type
            && self.bilagrid_shape == other.bilagrid_shape
            && f32_same(self.bilagrid_tv_loss_weight, other.bilagrid_tv_loss_weight)
            && f32_same(self.color_shift_reg_weight, other.color_shift_reg_weight)
            && self.color_shift_reg_ema_period == other.color_shift_reg_ema_period
            && self.use_bilateral_grid_for_geometry == other.use_bilateral_grid_for_geometry
            && self.bilagrid_shape_geometry == other.bilagrid_shape_geometry
            && f32_same(
                self.bilagrid_tv_loss_weight_geometry,
                other.bilagrid_tv_loss_weight_geometry,
            )
            && self.use_adagrad_bilagrid_optim == other.use_adagrad_bilagrid_optim
            && self.use_ppisp == other.use_ppisp
            && self.ppisp_param_type == other.ppisp_param_type
            && self.ppisp_exposure_from_exif == other.ppisp_exposure_from_exif
            && self.ppisp_exposure_arithmetic_mean == other.ppisp_exposure_arithmetic_mean
            && self.apply_ppisp_before_bilagrid == other.apply_ppisp_before_bilagrid
            && self.apply_ppisp_before_color_space == other.apply_ppisp_before_color_space
            && self.use_adagrad_ppisp_optim == other.use_adagrad_ppisp_optim
            && f32_same(self.ppisp_reg_exposure_mean, other.ppisp_reg_exposure_mean)
            && f32_same(self.ppisp_reg_color_mean, other.ppisp_reg_color_mean)
            && f32_same(self.ppisp_reg_vig_center, other.ppisp_reg_vig_center)
            && f32_same(self.ppisp_reg_vig_non_pos, other.ppisp_reg_vig_non_pos)
            && f32_same(
                self.ppisp_reg_vig_channel_var,
                other.ppisp_reg_vig_channel_var,
            )
            && f32_same(
                self.ppisp_reg_crf_channel_var,
                other.ppisp_reg_crf_channel_var,
            )
            && f32_same(self.bilagrid_adagrad_lr, other.bilagrid_adagrad_lr)
            && f32_same(
                self.bilagrid_adagrad_depth_lr,
                other.bilagrid_adagrad_depth_lr,
            )
            && f32_same(
                self.bilagrid_adagrad_normal_lr,
                other.bilagrid_adagrad_normal_lr,
            )
            && f32_same(self.ppisp_adagrad_lr, other.ppisp_adagrad_lr)
            && f32_same(self.bilagrid_lr, other.bilagrid_lr)
            && opt_f32_same(self.bilagrid_lr_final, other.bilagrid_lr_final)
            && self.bilagrid_lr_warmup == other.bilagrid_lr_warmup
            && f32_same(self.bilagrid_depth_lr, other.bilagrid_depth_lr)
            && opt_f32_same(self.bilagrid_depth_lr_final, other.bilagrid_depth_lr_final)
            && self.bilagrid_depth_lr_warmup == other.bilagrid_depth_lr_warmup
            && f32_same(self.bilagrid_normal_lr, other.bilagrid_normal_lr)
            && opt_f32_same(
                self.bilagrid_normal_lr_final,
                other.bilagrid_normal_lr_final,
            )
            && self.bilagrid_normal_lr_warmup == other.bilagrid_normal_lr_warmup
            && f32_same(self.ppisp_lr, other.ppisp_lr)
            && opt_f32_same(self.ppisp_lr_final, other.ppisp_lr_final)
            && self.ppisp_lr_warmup == other.ppisp_lr_warmup
            && self.image_color_is_linear == other.image_color_is_linear
            && self.image_color_transfer == other.image_color_transfer
            && self.image_color_gamut == other.image_color_gamut
            && self.splat_color_is_linear == other.splat_color_is_linear
            && self.splat_color_transfer == other.splat_color_transfer
            && self.splat_color_gamut == other.splat_color_gamut
            && self.point_color_is_linear == other.point_color_is_linear
            && self.point_color_transfer == other.point_color_transfer
            && self.point_color_gamut == other.point_color_gamut
            && self.cache_images == other.cache_images
            && self.max_batch_per_epoch == other.max_batch_per_epoch
            && f32_same(self.min_renders_per_refine, other.min_renders_per_refine)
            && f32_same(self.render_quantile, other.render_quantile)
            && self.max_train_batch_size == other.max_train_batch_size
            && self.split_batch == other.split_batch
            && self.use_fused_proj_bwd_optim == other.use_fused_proj_bwd_optim
            && self.packed == other.packed
            && self.quantization_level == other.quantization_level
            && self.bin_tile_size == other.bin_tile_size
            && self.preallocate_splat_tensors == other.preallocate_splat_tensors
            && self.optimizer_offload == other.optimizer_offload
            && self.use_bvh == other.use_bvh
            && f32_same(self.means_lr, other.means_lr)
            && opt_f32_same(self.means_lr_final, other.means_lr_final)
            && f32_same(self.scales_lr, other.scales_lr)
            && opt_f32_same(self.scales_lr_final, other.scales_lr_final)
            && f32_same(self.quats_lr, other.quats_lr)
            && f32_same(self.opacities_lr, other.opacities_lr)
            && f32_same(self.features_dc_lr, other.features_dc_lr)
            && f32_same(self.features_sh_lr, other.features_sh_lr)
            && f32_same(self.features_sb_lr, other.features_sb_lr)
            && f32_same(self.background_dc_lr, other.background_dc_lr)
            && f32_same(self.background_sh_lr, other.background_sh_lr)
            && self.max_steps == other.max_steps
            && self.use_scale_agnostic_mean == other.use_scale_agnostic_mean
            && self.use_per_splat_bias_correction == other.use_per_splat_bias_correction
    }
}

/// The reference table. Index `i` describes [`TrainConfig`]'s field in the same order.
pub const TRAIN_CONFIG_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "data",
        value_type: FieldType::String,
        section: "run",
        tier: "basic",
        choices: "<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "resume",
        value_type: FieldType::String,
        section: "run",
        tier: "advanced",
        choices: "<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "output_dir_prefix",
        value_type: FieldType::String,
        section: "run",
        tier: "basic",
        choices: "<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "output_dir_name",
        value_type: FieldType::String,
        section: "run",
        tier: "basic",
        choices: "none",
        choice_kind: ChoiceKind::FreeForm,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "num_iterations",
        value_type: FieldType::Int,
        section: "run",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "steps_per_save",
        value_type: FieldType::Int,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "save_only_latest_checkpoint",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "save_full_checkpoint",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "save_eval_images",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "eval_lpips",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "lpips_checkpoints",
        value_type: FieldType::String,
        section: "run",
        tier: "advanced",
        choices: "<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "log_performance",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "viewer_port",
        value_type: FieldType::Int,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "disable_viewer",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "keep_viewer_alive",
        value_type: FieldType::Bool,
        section: "run",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "data_format",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "colmap|nerfstudio|metashape|none",
        choice_kind: ChoiceKind::Closed(CLOSED_15),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "image_dir",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "mask_dir",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "load_masks",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "apply_loss_for_mask",
        value_type: FieldType::OptionalBool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "flip_mask",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "mask_boundary_offset",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "depth_dir",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "normal_dir",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "load_depths",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "load_normals",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "depth_unit_scale_factor",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "colmap_recon_dir",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<folder>",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "metashape_xml",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "<data>/<file>.xml",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "metashape_ply",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "<data>/<file>.ply",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "metashape_psx",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "<data>/<file>.psx",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "seed_pointcloud",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<data>/<file>.ply",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "init_ply",
        value_type: FieldType::String,
        section: "dataset",
        tier: "basic",
        choices: "<file>.ply",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "init_ply_add_points",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "partition",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "<file>.json",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "partition_part",
        value_type: FieldType::Int,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "roi_region",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "<data>/<file>.json",
        choice_kind: ChoiceKind::Path,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "roi_outside_weight",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "roi_outside_opacity_decay",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "roi_splat_samples",
        value_type: FieldType::Int,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "roi_mask_pixels",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "train_resolution_divisor",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "downscale_rounding_mode",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "floor|ceil|round",
        choice_kind: ChoiceKind::Closed(CLOSED_42),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_resolution",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_resolution_start",
        value_type: FieldType::Int,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_resolution_full_at",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_resolution_schedule",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "none",
        choice_kind: ChoiceKind::FreeForm,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_splat_budget",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "progressive_splat_budget_schedule",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "none",
        choice_kind: ChoiceKind::FreeForm,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "eval_mode",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "fraction|filename|interval|all",
        choice_kind: ChoiceKind::Closed(CLOSED_49),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "eval_interval",
        value_type: FieldType::Int,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "train_split_fraction",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "validation_fraction",
        value_type: FieldType::Float,
        section: "dataset",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "warp_to_pinhole",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "warp_spherical_to_pinhole",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "warp_face_fit",
        value_type: FieldType::String,
        section: "dataset",
        tier: "advanced",
        choices: "uniform|per-face",
        choice_kind: ChoiceKind::Closed(CLOSED_55),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "warp_back_face",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "deblur_training_images",
        value_type: FieldType::Bool,
        section: "dataset",
        tier: "stub",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "exif_orientation",
        value_type: FieldType::String,
        section: "scene",
        tier: "advanced",
        choices: "none|orient|apply",
        choice_kind: ChoiceKind::Closed(CLOSED_58),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "orientation_method",
        value_type: FieldType::String,
        section: "scene",
        tier: "expert",
        choices: "pca|up|vertical|none|gsplat",
        choice_kind: ChoiceKind::Closed(CLOSED_59),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "center_method",
        value_type: FieldType::String,
        section: "scene",
        tier: "expert",
        choices: "poses|focus|none|gsplat",
        choice_kind: ChoiceKind::Closed(CLOSED_60),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "auto_scale_poses",
        value_type: FieldType::Bool,
        section: "scene",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "outlier_threshold",
        value_type: FieldType::Float,
        section: "scene",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scene_center",
        value_type: FieldType::String,
        section: "scene",
        tier: "basic",
        choices: "auto|none|point-median|camera-median|camera-focus|point-mean|camera-mean",
        choice_kind: ChoiceKind::Closed(CLOSED_63),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scene_center_threshold",
        value_type: FieldType::Float,
        section: "scene",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "relative_scale",
        value_type: FieldType::OptionalFloat,
        section: "scene",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "train_frame",
        value_type: FieldType::String,
        section: "scene",
        tier: "expert",
        choices: "normalized|camera|points",
        choice_kind: ChoiceKind::Closed(CLOSED_66),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "primitive",
        value_type: FieldType::String,
        section: "splats",
        tier: "basic",
        choices: "3dgs|mip|3dgut",
        choice_kind: ChoiceKind::Closed(CLOSED_67),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "sh_degree",
        value_type: FieldType::Int,
        section: "splats",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "sb_lobes",
        value_type: FieldType::Int,
        section: "splats",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "sh_degree_warmup_every",
        value_type: FieldType::Int,
        section: "splats",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_mode",
        value_type: FieldType::String,
        section: "splats",
        tier: "basic",
        choices: "color|noise|pseudorandom|random|sh",
        choice_kind: ChoiceKind::Closed(CLOSED_71),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_color",
        value_type: FieldType::Vec3Float,
        section: "splats",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_sh_degree",
        value_type: FieldType::Int,
        section: "splats",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_noise_warmup",
        value_type: FieldType::Int,
        section: "splats",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_noise_pre_warmup",
        value_type: FieldType::Float,
        section: "splats",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_match_luminance",
        value_type: FieldType::Bool,
        section: "splats",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scale_init",
        value_type: FieldType::OptionalFloat,
        section: "splats",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "opacity_init",
        value_type: FieldType::OptionalFloat,
        section: "splats",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "suppress_initial_scales",
        value_type: FieldType::Bool,
        section: "splats",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init",
        value_type: FieldType::String,
        section: "splats",
        tier: "advanced",
        choices: "never|auto|always",
        choice_kind: ChoiceKind::Closed(CLOSED_80),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init_fraction",
        value_type: FieldType::Float,
        section: "splats",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init_distribution",
        value_type: FieldType::String,
        section: "splats",
        tier: "expert",
        choices: "isotropic-gaussian|anisotropic-gaussian|ellipsoid|box",
        choice_kind: ChoiceKind::Closed(CLOSED_82),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init_center",
        value_type: FieldType::String,
        section: "splats",
        tier: "expert",
        choices: "camera-median|camera-focus|camera-mean|origin",
        choice_kind: ChoiceKind::Closed(CLOSED_83),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init_spread",
        value_type: FieldType::String,
        section: "splats",
        tier: "expert",
        choices: "median|mean",
        choice_kind: ChoiceKind::Closed(CLOSED_84),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "random_init_std",
        value_type: FieldType::Float,
        section: "splats",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_camera_optimizer",
        value_type: FieldType::Bool,
        section: "splats",
        tier: "stub",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "quality",
        value_type: FieldType::String,
        section: "detail",
        tier: "basic",
        choices: "low|medium|high|ultra",
        choice_kind: ChoiceKind::Closed(CLOSED_87),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "cap_max",
        value_type: FieldType::Int,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "distraction_robustness",
        value_type: FieldType::String,
        section: "detail",
        tier: "basic",
        choices: "off|mild|strong",
        choice_kind: ChoiceKind::Closed(CLOSED_89),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "min_init_fraction",
        value_type: FieldType::Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "growth_factor",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "min_opacity",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "dead_criterion",
        value_type: FieldType::String,
        section: "detail",
        tier: "advanced",
        choices: "opacity|max_weight|either",
        choice_kind: ChoiceKind::Closed(CLOSED_93),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "refine_every",
        value_type: FieldType::Int,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "refine_start_iter",
        value_type: FieldType::Int,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "refine_stop_num_iter",
        value_type: FieldType::Int,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "refine_stop_iter",
        value_type: FieldType::Int,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "noise_lr",
        value_type: FieldType::Float,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "noise_lr_final",
        value_type: FieldType::Float,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_revised_densification",
        value_type: FieldType::Bool,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_score_mode",
        value_type: FieldType::String,
        section: "detail",
        tier: "basic",
        choices: "mean|max|median|geom",
        choice_kind: ChoiceKind::Closed(CLOSED_101),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_score_blend_world_grad",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_loss_map_mode",
        value_type: FieldType::String,
        section: "detail",
        tier: "basic",
        choices: "auto|none|loss_full|ssim_full|ssim_cs|ssim_structure|s_cielab|flip|flip_feature|edge_aware|robust_edge_aware|loss_full_nms|ssim_full_nms|ssim_cs_nms|ssim_structure_nms|s_cielab_nms|flip_nms|flip_feature_nms",
        choice_kind: ChoiceKind::Closed(CLOSED_103),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_robust_edge_aware_quantile",
        value_type: FieldType::Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_nms_falloff",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_loss_map_normalize",
        value_type: FieldType::Bool,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_loss_map_clip_quantile",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_loss_map_power",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_accum_mode",
        value_type: FieldType::String,
        section: "detail",
        tier: "basic",
        choices: "max|sum|avg",
        choice_kind: ChoiceKind::Closed(CLOSED_109),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_score_power",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_score_clip_quantile",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_final_score_power",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_learned_score",
        value_type: FieldType::Bool,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_oversize_split_fraction",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "densify_oversize_score_blend",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_long_axis_split",
        value_type: FieldType::Bool,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "long_axis_split_opacity_k",
        value_type: FieldType::Vec3Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_split_fraction",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "split_weight_by_renders",
        value_type: FieldType::Bool,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "dead_after_epochs",
        value_type: FieldType::Float,
        section: "detail",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_screen_size",
        value_type: FieldType::Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_screen_size_clip_hardness",
        value_type: FieldType::Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_screen_size_penalty",
        value_type: FieldType::Float,
        section: "detail",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_world_size",
        value_type: FieldType::Float,
        section: "detail",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ssim_lambda",
        value_type: FieldType::Float,
        section: "loss",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "perceptual_loss",
        value_type: FieldType::String,
        section: "loss",
        tier: "advanced",
        choices: "ssim|ssim_blur_tolerant|s_cielab|flip",
        choice_kind: ChoiceKind::Closed(CLOSED_126),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l1_weight",
        value_type: FieldType::Float,
        section: "loss",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l2_weight",
        value_type: FieldType::Float,
        section: "loss",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l1_weight_y",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l2_weight_y",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l2_weight_u",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "l2_weight_v",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "loss_scale_min_pixels",
        value_type: FieldType::Int,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "num_loss_scales",
        value_type: FieldType::Int,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "alpha_loss_weight",
        value_type: FieldType::Float,
        section: "loss",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "alpha_loss_weight_under",
        value_type: FieldType::Float,
        section: "loss",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "loss_saturation_threshold",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "loss_luminance_normalization",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "view_sampling",
        value_type: FieldType::String,
        section: "loss",
        tier: "basic",
        choices: "uniform|deficit",
        choice_kind: ChoiceKind::Closed(CLOSED_139),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "view_deficit_power",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "view_deficit_max_ratio",
        value_type: FieldType::Float,
        section: "loss",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "floater_suppression",
        value_type: FieldType::String,
        section: "geometry",
        tier: "basic",
        choices: "off|mild|strong",
        choice_kind: ChoiceKind::Closed(CLOSED_142),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "depth_distortion_reg",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "normal_distortion_reg",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "rgb_distortion_reg",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "distortion_reg_warmup",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "normal_reg_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "normal_reg_warmup",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "alpha_reg_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "alpha_reg_warmup",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "reg_warmup_length",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "depth_supervision_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "normal_supervision_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "input_depth_is_ray_depth",
        value_type: FieldType::OptionalBool,
        section: "geometry",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "supervision_warmup",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "mean_median_depth_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "median_depth_normal_reg_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "median_normal_supervision_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "median_render_normal_reg_weight",
        value_type: FieldType::Float,
        section: "geometry",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "median_warmup",
        value_type: FieldType::Int,
        section: "geometry",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "opacity_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "reg_rendered_only",
        value_type: FieldType::Bool,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scale_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "opacity_reg_decay_power",
        value_type: FieldType::Float,
        section: "shape",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scale_reg_decay_power",
        value_type: FieldType::Float,
        section: "shape",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "opacity_decay",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scale_decay",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "erank_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "erank_reg_s3",
        value_type: FieldType::Float,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scale_regularization_weight",
        value_type: FieldType::Float,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_gauss_ratio",
        value_type: FieldType::Float,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "dc_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "sh_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "overexposure_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "quat_norm_reg",
        value_type: FieldType::Float,
        section: "shape",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_bilateral_grid",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_type",
        value_type: FieldType::String,
        section: "correction",
        tier: "basic",
        choices: "affine|ppisp|loglinear",
        choice_kind: ChoiceKind::Closed(CLOSED_177),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_shape",
        value_type: FieldType::Vec3Int,
        section: "correction",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_tv_loss_weight",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "color_shift_reg_weight",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "color_shift_reg_ema_period",
        value_type: FieldType::Int,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_bilateral_grid_for_geometry",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_shape_geometry",
        value_type: FieldType::Vec3Int,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_tv_loss_weight_geometry",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_adagrad_bilagrid_optim",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_ppisp",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_param_type",
        value_type: FieldType::String,
        section: "correction",
        tier: "basic",
        choices: "original|rqs|no_crf|no_crf_clamp|no_crf_no_vig|no_crf_no_vig_clamp",
        choice_kind: ChoiceKind::Closed(CLOSED_187),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_exposure_from_exif",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_exposure_arithmetic_mean",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "apply_ppisp_before_bilagrid",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "apply_ppisp_before_color_space",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_adagrad_ppisp_optim",
        value_type: FieldType::Bool,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_exposure_mean",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_color_mean",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_vig_center",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_vig_non_pos",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_vig_channel_var",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_reg_crf_channel_var",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_adagrad_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_adagrad_depth_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_adagrad_normal_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_adagrad_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_lr_warmup",
        value_type: FieldType::Int,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_depth_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_depth_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_depth_lr_warmup",
        value_type: FieldType::Int,
        section: "correction",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_normal_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_normal_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bilagrid_normal_lr_warmup",
        value_type: FieldType::Int,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_lr",
        value_type: FieldType::Float,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "ppisp_lr_warmup",
        value_type: FieldType::Int,
        section: "correction",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "image_color_is_linear",
        value_type: FieldType::OptionalBool,
        section: "colorspace",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "image_color_transfer",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "advanced",
        choices: "srgb|srgb-clamped|aces|filmic|uncharted2|none",
        choice_kind: ChoiceKind::Closed(CLOSED_216),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "image_color_gamut",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "basic",
        choices: "Rec.709|ACES2065-1|ACEScg|Rec.2020|AdobeRGB|DCI-P3|none",
        choice_kind: ChoiceKind::Closed(CLOSED_217),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "splat_color_is_linear",
        value_type: FieldType::OptionalBool,
        section: "colorspace",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "splat_color_transfer",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "advanced",
        choices: "srgb|srgb-clamped|aces|filmic|uncharted2|none",
        choice_kind: ChoiceKind::Closed(CLOSED_219),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "splat_color_gamut",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "basic",
        choices: "Rec.709|ACES2065-1|ACEScg|Rec.2020|AdobeRGB|DCI-P3|none",
        choice_kind: ChoiceKind::Closed(CLOSED_220),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "point_color_is_linear",
        value_type: FieldType::OptionalBool,
        section: "colorspace",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "point_color_transfer",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "advanced",
        choices: "srgb|srgb-clamped|aces|filmic|uncharted2|none",
        choice_kind: ChoiceKind::Closed(CLOSED_222),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "point_color_gamut",
        value_type: FieldType::String,
        section: "colorspace",
        tier: "basic",
        choices: "Rec.709|ACES2065-1|ACEScg|Rec.2020|AdobeRGB|DCI-P3|none",
        choice_kind: ChoiceKind::Closed(CLOSED_223),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "cache_images",
        value_type: FieldType::String,
        section: "perf",
        tier: "basic",
        choices: "cpu|gpu|disk",
        choice_kind: ChoiceKind::Closed(CLOSED_224),
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_batch_per_epoch",
        value_type: FieldType::Int,
        section: "perf",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "min_renders_per_refine",
        value_type: FieldType::Float,
        section: "perf",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "render_quantile",
        value_type: FieldType::Float,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_train_batch_size",
        value_type: FieldType::Int,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "split_batch",
        value_type: FieldType::Bool,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_fused_proj_bwd_optim",
        value_type: FieldType::Bool,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "packed",
        value_type: FieldType::Bool,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "quantization_level",
        value_type: FieldType::Int,
        section: "perf",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "bin_tile_size",
        value_type: FieldType::Int,
        section: "perf",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "preallocate_splat_tensors",
        value_type: FieldType::Bool,
        section: "perf",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "optimizer_offload",
        value_type: FieldType::String,
        section: "perf",
        tier: "stub",
        choices: "sh|all|none",
        choice_kind: ChoiceKind::Closed(CLOSED_235),
        empty_is_unset: true,
    },
    FieldSpec {
        name: "use_bvh",
        value_type: FieldType::Bool,
        section: "perf",
        tier: "stub",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "means_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "means_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scales_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "scales_lr_final",
        value_type: FieldType::OptionalFloat,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "quats_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "opacities_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "features_dc_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "features_sh_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "features_sb_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "basic",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_dc_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "background_sh_lr",
        value_type: FieldType::Float,
        section: "rates",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "max_steps",
        value_type: FieldType::OptionalInt,
        section: "rates",
        tier: "advanced",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_scale_agnostic_mean",
        value_type: FieldType::Bool,
        section: "rates",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
    FieldSpec {
        name: "use_per_splat_bias_correction",
        value_type: FieldType::Bool,
        section: "rates",
        tier: "expert",
        choices: "",
        choice_kind: ChoiceKind::Free,
        empty_is_unset: false,
    },
];

/// Metadata for `name`, if it is one of the 251 flags.
pub fn field_spec(name: &str) -> Option<&'static FieldSpec> {
    TRAIN_CONFIG_FIELDS.iter().find(|spec| spec.name == name)
}

fn f32_same(left: f32, right: f32) -> bool {
    left == right || (left.is_nan() && right.is_nan())
}

fn opt_f32_same(left: Option<f32>, right: Option<f32>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => f32_same(left, right),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_reference_field() {
        assert_eq!(TRAIN_CONFIG_FIELDS.len(), TRAIN_CONFIG_FIELD_COUNT);
        assert_eq!(TRAIN_CONFIG_FIELD_COUNT, 251);
        let mut names = std::collections::BTreeSet::new();
        for spec in TRAIN_CONFIG_FIELDS {
            assert!(names.insert(spec.name), "duplicate {}", spec.name);
            assert!(
                TRAIN_CONFIG_SECTIONS.contains(&spec.section),
                "{}",
                spec.name
            );
            assert!(TRAIN_CONFIG_TIERS.contains(&spec.tier), "{}", spec.name);
            assert_eq!(
                field_spec(spec.name).map(|found| found.name),
                Some(spec.name)
            );
        }
        assert!(field_spec("preset").is_none());
        assert!(field_spec("steps").is_none());
        assert!(field_spec("schema_version").is_none());

        let config = TrainConfig::default();
        assert_eq!(config.num_iterations, 30000);
        assert_eq!(config.steps_per_save, 2000);
        assert!(config.save_only_latest_checkpoint);
        assert_eq!(config.output_dir_prefix, "outputs");
        assert_eq!(config.output_dir_name, "");
        assert_eq!(config.data, "");
        assert_eq!(config.scene_center, "auto");
        assert_eq!(config.primitive, "3dgs");
        assert!(
            config.outlier_threshold.is_infinite() && config.outlier_threshold.is_sign_positive()
        );
        assert!(config.max_world_size.is_infinite() && config.max_world_size.is_sign_positive());
        assert_eq!(config.background_color, [0.0, 0.0, 0.0]);
        assert_eq!(config.bilagrid_shape, [16, 16, 8]);
        assert_eq!(config.long_axis_split_opacity_k, [0.5, 0.55, 15000.0]);
        assert_eq!(config.scale_init, Some(0.35));
        assert_eq!(config.opacity_init, None);
        assert_eq!(config.apply_loss_for_mask, None);
        assert_eq!(config.max_steps, None);
        assert_eq!(config.means_lr_final, Some(3.2e-6));
        assert!(config.same_values(&TrainConfig::default()));

        let data = field_spec("data").unwrap();
        assert_eq!(data.choice_kind, ChoiceKind::Path);
        assert!(!data.empty_is_unset);
        let output_name = field_spec("output_dir_name").unwrap();
        assert_eq!(output_name.choice_kind, ChoiceKind::FreeForm);
        let scene_center = field_spec("scene_center").unwrap();
        assert!(matches!(
            scene_center.choice_kind,
            ChoiceKind::Closed(choices) if choices.contains(&"auto")
        ));
        assert!(!scene_center.empty_is_unset);
        let data_format = field_spec("data_format").unwrap();
        assert!(data_format.empty_is_unset);
        assert!(matches!(
            data_format.choice_kind,
            ChoiceKind::Closed(choices) if choices.contains(&"colmap")
        ));
        assert_eq!(
            field_spec("num_iterations").unwrap().choice_kind,
            ChoiceKind::Free
        );
        assert_eq!(
            field_spec("progressive_resolution_schedule")
                .unwrap()
                .choice_kind,
            ChoiceKind::FreeForm
        );
    }
}

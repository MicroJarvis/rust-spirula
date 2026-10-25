//! Assign and emit the 251 reference fields by name.

use crate::DomainError;
use crate::config::{FieldSpec, TrainConfig, field_spec};
use crate::config_value::{
    Take, canonical_bool, canonical_f32, canonical_i32, canonical_opt_bool, canonical_opt_f32,
    canonical_opt_i32, canonical_string, canonical_vec3_f, canonical_vec3_i, check_choice,
    take_bool, take_f32, take_i32, take_opt_bool, take_opt_f32, take_opt_i32, take_string,
    take_vec3_f, take_vec3_i,
};
use crate::json_doc::JsonValue;

pub(crate) fn assign_field(
    config: &mut TrainConfig,
    name: &str,
    value: &JsonValue,
) -> Result<bool, DomainError> {
    match name {
        "data" => {
            config.data = take_string("data", value)?;
            Ok(true)
        }
        "resume" => {
            config.resume = take_string("resume", value)?;
            Ok(true)
        }
        "output_dir_prefix" => {
            config.output_dir_prefix = take_string("output_dir_prefix", value)?;
            Ok(true)
        }
        "output_dir_name" => {
            config.output_dir_name = take_string("output_dir_name", value)?;
            Ok(true)
        }
        "num_iterations" => {
            if let Take::Set(parsed) = take_i32("num_iterations", value)? {
                config.num_iterations = parsed;
            }
            Ok(true)
        }
        "steps_per_save" => {
            if let Take::Set(parsed) = take_i32("steps_per_save", value)? {
                config.steps_per_save = parsed;
            }
            Ok(true)
        }
        "save_only_latest_checkpoint" => {
            if let Take::Set(parsed) = take_bool("save_only_latest_checkpoint", value)? {
                config.save_only_latest_checkpoint = parsed;
            }
            Ok(true)
        }
        "save_full_checkpoint" => {
            if let Take::Set(parsed) = take_bool("save_full_checkpoint", value)? {
                config.save_full_checkpoint = parsed;
            }
            Ok(true)
        }
        "save_eval_images" => {
            if let Take::Set(parsed) = take_bool("save_eval_images", value)? {
                config.save_eval_images = parsed;
            }
            Ok(true)
        }
        "eval_lpips" => {
            if let Take::Set(parsed) = take_bool("eval_lpips", value)? {
                config.eval_lpips = parsed;
            }
            Ok(true)
        }
        "lpips_checkpoints" => {
            config.lpips_checkpoints = take_string("lpips_checkpoints", value)?;
            Ok(true)
        }
        "log_performance" => {
            if let Take::Set(parsed) = take_bool("log_performance", value)? {
                config.log_performance = parsed;
            }
            Ok(true)
        }
        "viewer_port" => {
            if let Take::Set(parsed) = take_i32("viewer_port", value)? {
                config.viewer_port = parsed;
            }
            Ok(true)
        }
        "disable_viewer" => {
            if let Take::Set(parsed) = take_bool("disable_viewer", value)? {
                config.disable_viewer = parsed;
            }
            Ok(true)
        }
        "keep_viewer_alive" => {
            if let Take::Set(parsed) = take_bool("keep_viewer_alive", value)? {
                config.keep_viewer_alive = parsed;
            }
            Ok(true)
        }
        "data_format" => {
            config.data_format = take_string("data_format", value)?;
            Ok(true)
        }
        "image_dir" => {
            config.image_dir = take_string("image_dir", value)?;
            Ok(true)
        }
        "mask_dir" => {
            config.mask_dir = take_string("mask_dir", value)?;
            Ok(true)
        }
        "load_masks" => {
            if let Take::Set(parsed) = take_bool("load_masks", value)? {
                config.load_masks = parsed;
            }
            Ok(true)
        }
        "apply_loss_for_mask" => {
            config.apply_loss_for_mask = take_opt_bool("apply_loss_for_mask", value)?;
            Ok(true)
        }
        "flip_mask" => {
            if let Take::Set(parsed) = take_bool("flip_mask", value)? {
                config.flip_mask = parsed;
            }
            Ok(true)
        }
        "mask_boundary_offset" => {
            if let Take::Set(parsed) = take_f32("mask_boundary_offset", value)? {
                config.mask_boundary_offset = parsed;
            }
            Ok(true)
        }
        "depth_dir" => {
            config.depth_dir = take_string("depth_dir", value)?;
            Ok(true)
        }
        "normal_dir" => {
            config.normal_dir = take_string("normal_dir", value)?;
            Ok(true)
        }
        "load_depths" => {
            if let Take::Set(parsed) = take_bool("load_depths", value)? {
                config.load_depths = parsed;
            }
            Ok(true)
        }
        "load_normals" => {
            if let Take::Set(parsed) = take_bool("load_normals", value)? {
                config.load_normals = parsed;
            }
            Ok(true)
        }
        "depth_unit_scale_factor" => {
            if let Take::Set(parsed) = take_f32("depth_unit_scale_factor", value)? {
                config.depth_unit_scale_factor = parsed;
            }
            Ok(true)
        }
        "colmap_recon_dir" => {
            config.colmap_recon_dir = take_string("colmap_recon_dir", value)?;
            Ok(true)
        }
        "metashape_xml" => {
            config.metashape_xml = take_string("metashape_xml", value)?;
            Ok(true)
        }
        "metashape_ply" => {
            config.metashape_ply = take_string("metashape_ply", value)?;
            Ok(true)
        }
        "metashape_psx" => {
            config.metashape_psx = take_string("metashape_psx", value)?;
            Ok(true)
        }
        "seed_pointcloud" => {
            config.seed_pointcloud = take_string("seed_pointcloud", value)?;
            Ok(true)
        }
        "init_ply" => {
            config.init_ply = take_string("init_ply", value)?;
            Ok(true)
        }
        "init_ply_add_points" => {
            if let Take::Set(parsed) = take_bool("init_ply_add_points", value)? {
                config.init_ply_add_points = parsed;
            }
            Ok(true)
        }
        "partition" => {
            config.partition = take_string("partition", value)?;
            Ok(true)
        }
        "partition_part" => {
            if let Take::Set(parsed) = take_i32("partition_part", value)? {
                config.partition_part = parsed;
            }
            Ok(true)
        }
        "roi_region" => {
            config.roi_region = take_string("roi_region", value)?;
            Ok(true)
        }
        "roi_outside_weight" => {
            if let Take::Set(parsed) = take_f32("roi_outside_weight", value)? {
                config.roi_outside_weight = parsed;
            }
            Ok(true)
        }
        "roi_outside_opacity_decay" => {
            if let Take::Set(parsed) = take_f32("roi_outside_opacity_decay", value)? {
                config.roi_outside_opacity_decay = parsed;
            }
            Ok(true)
        }
        "roi_splat_samples" => {
            if let Take::Set(parsed) = take_i32("roi_splat_samples", value)? {
                config.roi_splat_samples = parsed;
            }
            Ok(true)
        }
        "roi_mask_pixels" => {
            if let Take::Set(parsed) = take_bool("roi_mask_pixels", value)? {
                config.roi_mask_pixels = parsed;
            }
            Ok(true)
        }
        "train_resolution_divisor" => {
            if let Take::Set(parsed) = take_f32("train_resolution_divisor", value)? {
                config.train_resolution_divisor = parsed;
            }
            Ok(true)
        }
        "downscale_rounding_mode" => {
            config.downscale_rounding_mode = take_string("downscale_rounding_mode", value)?;
            Ok(true)
        }
        "progressive_resolution" => {
            if let Take::Set(parsed) = take_bool("progressive_resolution", value)? {
                config.progressive_resolution = parsed;
            }
            Ok(true)
        }
        "progressive_resolution_start" => {
            if let Take::Set(parsed) = take_i32("progressive_resolution_start", value)? {
                config.progressive_resolution_start = parsed;
            }
            Ok(true)
        }
        "progressive_resolution_full_at" => {
            if let Take::Set(parsed) = take_f32("progressive_resolution_full_at", value)? {
                config.progressive_resolution_full_at = parsed;
            }
            Ok(true)
        }
        "progressive_resolution_schedule" => {
            config.progressive_resolution_schedule =
                take_string("progressive_resolution_schedule", value)?;
            Ok(true)
        }
        "progressive_splat_budget" => {
            if let Take::Set(parsed) = take_bool("progressive_splat_budget", value)? {
                config.progressive_splat_budget = parsed;
            }
            Ok(true)
        }
        "progressive_splat_budget_schedule" => {
            config.progressive_splat_budget_schedule =
                take_string("progressive_splat_budget_schedule", value)?;
            Ok(true)
        }
        "eval_mode" => {
            config.eval_mode = take_string("eval_mode", value)?;
            Ok(true)
        }
        "eval_interval" => {
            if let Take::Set(parsed) = take_i32("eval_interval", value)? {
                config.eval_interval = parsed;
            }
            Ok(true)
        }
        "train_split_fraction" => {
            if let Take::Set(parsed) = take_f32("train_split_fraction", value)? {
                config.train_split_fraction = parsed;
            }
            Ok(true)
        }
        "validation_fraction" => {
            if let Take::Set(parsed) = take_f32("validation_fraction", value)? {
                config.validation_fraction = parsed;
            }
            Ok(true)
        }
        "warp_to_pinhole" => {
            if let Take::Set(parsed) = take_bool("warp_to_pinhole", value)? {
                config.warp_to_pinhole = parsed;
            }
            Ok(true)
        }
        "warp_spherical_to_pinhole" => {
            if let Take::Set(parsed) = take_bool("warp_spherical_to_pinhole", value)? {
                config.warp_spherical_to_pinhole = parsed;
            }
            Ok(true)
        }
        "warp_face_fit" => {
            config.warp_face_fit = take_string("warp_face_fit", value)?;
            Ok(true)
        }
        "warp_back_face" => {
            if let Take::Set(parsed) = take_bool("warp_back_face", value)? {
                config.warp_back_face = parsed;
            }
            Ok(true)
        }
        "deblur_training_images" => {
            if let Take::Set(parsed) = take_bool("deblur_training_images", value)? {
                config.deblur_training_images = parsed;
            }
            Ok(true)
        }
        "exif_orientation" => {
            config.exif_orientation = take_string("exif_orientation", value)?;
            Ok(true)
        }
        "orientation_method" => {
            config.orientation_method = take_string("orientation_method", value)?;
            Ok(true)
        }
        "center_method" => {
            config.center_method = take_string("center_method", value)?;
            Ok(true)
        }
        "auto_scale_poses" => {
            if let Take::Set(parsed) = take_bool("auto_scale_poses", value)? {
                config.auto_scale_poses = parsed;
            }
            Ok(true)
        }
        "outlier_threshold" => {
            if let Take::Set(parsed) = take_f32("outlier_threshold", value)? {
                config.outlier_threshold = parsed;
            }
            Ok(true)
        }
        "scene_center" => {
            config.scene_center = take_string("scene_center", value)?;
            Ok(true)
        }
        "scene_center_threshold" => {
            if let Take::Set(parsed) = take_f32("scene_center_threshold", value)? {
                config.scene_center_threshold = parsed;
            }
            Ok(true)
        }
        "relative_scale" => {
            config.relative_scale = take_opt_f32("relative_scale", value)?;
            Ok(true)
        }
        "train_frame" => {
            config.train_frame = take_string("train_frame", value)?;
            Ok(true)
        }
        "primitive" => {
            config.primitive = take_string("primitive", value)?;
            Ok(true)
        }
        "sh_degree" => {
            if let Take::Set(parsed) = take_i32("sh_degree", value)? {
                config.sh_degree = parsed;
            }
            Ok(true)
        }
        "sb_lobes" => {
            if let Take::Set(parsed) = take_i32("sb_lobes", value)? {
                config.sb_lobes = parsed;
            }
            Ok(true)
        }
        "sh_degree_warmup_every" => {
            if let Take::Set(parsed) = take_i32("sh_degree_warmup_every", value)? {
                config.sh_degree_warmup_every = parsed;
            }
            Ok(true)
        }
        "background_mode" => {
            config.background_mode = take_string("background_mode", value)?;
            Ok(true)
        }
        "background_color" => {
            if let Take::Set(parsed) = take_vec3_f("background_color", value)? {
                config.background_color = parsed;
            }
            Ok(true)
        }
        "background_sh_degree" => {
            if let Take::Set(parsed) = take_i32("background_sh_degree", value)? {
                config.background_sh_degree = parsed;
            }
            Ok(true)
        }
        "background_noise_warmup" => {
            if let Take::Set(parsed) = take_i32("background_noise_warmup", value)? {
                config.background_noise_warmup = parsed;
            }
            Ok(true)
        }
        "background_noise_pre_warmup" => {
            if let Take::Set(parsed) = take_f32("background_noise_pre_warmup", value)? {
                config.background_noise_pre_warmup = parsed;
            }
            Ok(true)
        }
        "background_match_luminance" => {
            if let Take::Set(parsed) = take_bool("background_match_luminance", value)? {
                config.background_match_luminance = parsed;
            }
            Ok(true)
        }
        "scale_init" => {
            config.scale_init = take_opt_f32("scale_init", value)?;
            Ok(true)
        }
        "opacity_init" => {
            config.opacity_init = take_opt_f32("opacity_init", value)?;
            Ok(true)
        }
        "suppress_initial_scales" => {
            if let Take::Set(parsed) = take_bool("suppress_initial_scales", value)? {
                config.suppress_initial_scales = parsed;
            }
            Ok(true)
        }
        "random_init" => {
            config.random_init = take_string("random_init", value)?;
            Ok(true)
        }
        "random_init_fraction" => {
            if let Take::Set(parsed) = take_f32("random_init_fraction", value)? {
                config.random_init_fraction = parsed;
            }
            Ok(true)
        }
        "random_init_distribution" => {
            config.random_init_distribution = take_string("random_init_distribution", value)?;
            Ok(true)
        }
        "random_init_center" => {
            config.random_init_center = take_string("random_init_center", value)?;
            Ok(true)
        }
        "random_init_spread" => {
            config.random_init_spread = take_string("random_init_spread", value)?;
            Ok(true)
        }
        "random_init_std" => {
            if let Take::Set(parsed) = take_f32("random_init_std", value)? {
                config.random_init_std = parsed;
            }
            Ok(true)
        }
        "use_camera_optimizer" => {
            if let Take::Set(parsed) = take_bool("use_camera_optimizer", value)? {
                config.use_camera_optimizer = parsed;
            }
            Ok(true)
        }
        "quality" => {
            config.quality = take_string("quality", value)?;
            Ok(true)
        }
        "cap_max" => {
            if let Take::Set(parsed) = take_i32("cap_max", value)? {
                config.cap_max = parsed;
            }
            Ok(true)
        }
        "distraction_robustness" => {
            config.distraction_robustness = take_string("distraction_robustness", value)?;
            Ok(true)
        }
        "min_init_fraction" => {
            if let Take::Set(parsed) = take_f32("min_init_fraction", value)? {
                config.min_init_fraction = parsed;
            }
            Ok(true)
        }
        "growth_factor" => {
            if let Take::Set(parsed) = take_f32("growth_factor", value)? {
                config.growth_factor = parsed;
            }
            Ok(true)
        }
        "min_opacity" => {
            if let Take::Set(parsed) = take_f32("min_opacity", value)? {
                config.min_opacity = parsed;
            }
            Ok(true)
        }
        "dead_criterion" => {
            config.dead_criterion = take_string("dead_criterion", value)?;
            Ok(true)
        }
        "refine_every" => {
            if let Take::Set(parsed) = take_i32("refine_every", value)? {
                config.refine_every = parsed;
            }
            Ok(true)
        }
        "refine_start_iter" => {
            if let Take::Set(parsed) = take_i32("refine_start_iter", value)? {
                config.refine_start_iter = parsed;
            }
            Ok(true)
        }
        "refine_stop_num_iter" => {
            if let Take::Set(parsed) = take_i32("refine_stop_num_iter", value)? {
                config.refine_stop_num_iter = parsed;
            }
            Ok(true)
        }
        "refine_stop_iter" => {
            if let Take::Set(parsed) = take_i32("refine_stop_iter", value)? {
                config.refine_stop_iter = parsed;
            }
            Ok(true)
        }
        "noise_lr" => {
            if let Take::Set(parsed) = take_f32("noise_lr", value)? {
                config.noise_lr = parsed;
            }
            Ok(true)
        }
        "noise_lr_final" => {
            if let Take::Set(parsed) = take_f32("noise_lr_final", value)? {
                config.noise_lr_final = parsed;
            }
            Ok(true)
        }
        "use_revised_densification" => {
            if let Take::Set(parsed) = take_bool("use_revised_densification", value)? {
                config.use_revised_densification = parsed;
            }
            Ok(true)
        }
        "densify_score_mode" => {
            config.densify_score_mode = take_string("densify_score_mode", value)?;
            Ok(true)
        }
        "densify_score_blend_world_grad" => {
            if let Take::Set(parsed) = take_f32("densify_score_blend_world_grad", value)? {
                config.densify_score_blend_world_grad = parsed;
            }
            Ok(true)
        }
        "densify_loss_map_mode" => {
            config.densify_loss_map_mode = take_string("densify_loss_map_mode", value)?;
            Ok(true)
        }
        "densify_robust_edge_aware_quantile" => {
            if let Take::Set(parsed) = take_f32("densify_robust_edge_aware_quantile", value)? {
                config.densify_robust_edge_aware_quantile = parsed;
            }
            Ok(true)
        }
        "densify_nms_falloff" => {
            if let Take::Set(parsed) = take_f32("densify_nms_falloff", value)? {
                config.densify_nms_falloff = parsed;
            }
            Ok(true)
        }
        "densify_loss_map_normalize" => {
            if let Take::Set(parsed) = take_bool("densify_loss_map_normalize", value)? {
                config.densify_loss_map_normalize = parsed;
            }
            Ok(true)
        }
        "densify_loss_map_clip_quantile" => {
            if let Take::Set(parsed) = take_f32("densify_loss_map_clip_quantile", value)? {
                config.densify_loss_map_clip_quantile = parsed;
            }
            Ok(true)
        }
        "densify_loss_map_power" => {
            if let Take::Set(parsed) = take_f32("densify_loss_map_power", value)? {
                config.densify_loss_map_power = parsed;
            }
            Ok(true)
        }
        "densify_accum_mode" => {
            config.densify_accum_mode = take_string("densify_accum_mode", value)?;
            Ok(true)
        }
        "densify_score_power" => {
            if let Take::Set(parsed) = take_f32("densify_score_power", value)? {
                config.densify_score_power = parsed;
            }
            Ok(true)
        }
        "densify_score_clip_quantile" => {
            if let Take::Set(parsed) = take_f32("densify_score_clip_quantile", value)? {
                config.densify_score_clip_quantile = parsed;
            }
            Ok(true)
        }
        "densify_final_score_power" => {
            if let Take::Set(parsed) = take_f32("densify_final_score_power", value)? {
                config.densify_final_score_power = parsed;
            }
            Ok(true)
        }
        "densify_learned_score" => {
            if let Take::Set(parsed) = take_bool("densify_learned_score", value)? {
                config.densify_learned_score = parsed;
            }
            Ok(true)
        }
        "densify_oversize_split_fraction" => {
            if let Take::Set(parsed) = take_f32("densify_oversize_split_fraction", value)? {
                config.densify_oversize_split_fraction = parsed;
            }
            Ok(true)
        }
        "densify_oversize_score_blend" => {
            if let Take::Set(parsed) = take_f32("densify_oversize_score_blend", value)? {
                config.densify_oversize_score_blend = parsed;
            }
            Ok(true)
        }
        "use_long_axis_split" => {
            if let Take::Set(parsed) = take_bool("use_long_axis_split", value)? {
                config.use_long_axis_split = parsed;
            }
            Ok(true)
        }
        "long_axis_split_opacity_k" => {
            if let Take::Set(parsed) = take_vec3_f("long_axis_split_opacity_k", value)? {
                config.long_axis_split_opacity_k = parsed;
            }
            Ok(true)
        }
        "max_split_fraction" => {
            if let Take::Set(parsed) = take_f32("max_split_fraction", value)? {
                config.max_split_fraction = parsed;
            }
            Ok(true)
        }
        "split_weight_by_renders" => {
            if let Take::Set(parsed) = take_bool("split_weight_by_renders", value)? {
                config.split_weight_by_renders = parsed;
            }
            Ok(true)
        }
        "dead_after_epochs" => {
            if let Take::Set(parsed) = take_f32("dead_after_epochs", value)? {
                config.dead_after_epochs = parsed;
            }
            Ok(true)
        }
        "max_screen_size" => {
            if let Take::Set(parsed) = take_f32("max_screen_size", value)? {
                config.max_screen_size = parsed;
            }
            Ok(true)
        }
        "max_screen_size_clip_hardness" => {
            if let Take::Set(parsed) = take_f32("max_screen_size_clip_hardness", value)? {
                config.max_screen_size_clip_hardness = parsed;
            }
            Ok(true)
        }
        "max_screen_size_penalty" => {
            if let Take::Set(parsed) = take_f32("max_screen_size_penalty", value)? {
                config.max_screen_size_penalty = parsed;
            }
            Ok(true)
        }
        "max_world_size" => {
            if let Take::Set(parsed) = take_f32("max_world_size", value)? {
                config.max_world_size = parsed;
            }
            Ok(true)
        }
        "ssim_lambda" => {
            if let Take::Set(parsed) = take_f32("ssim_lambda", value)? {
                config.ssim_lambda = parsed;
            }
            Ok(true)
        }
        "perceptual_loss" => {
            config.perceptual_loss = take_string("perceptual_loss", value)?;
            Ok(true)
        }
        "l1_weight" => {
            if let Take::Set(parsed) = take_f32("l1_weight", value)? {
                config.l1_weight = parsed;
            }
            Ok(true)
        }
        "l2_weight" => {
            if let Take::Set(parsed) = take_f32("l2_weight", value)? {
                config.l2_weight = parsed;
            }
            Ok(true)
        }
        "l1_weight_y" => {
            if let Take::Set(parsed) = take_f32("l1_weight_y", value)? {
                config.l1_weight_y = parsed;
            }
            Ok(true)
        }
        "l2_weight_y" => {
            if let Take::Set(parsed) = take_f32("l2_weight_y", value)? {
                config.l2_weight_y = parsed;
            }
            Ok(true)
        }
        "l2_weight_u" => {
            if let Take::Set(parsed) = take_f32("l2_weight_u", value)? {
                config.l2_weight_u = parsed;
            }
            Ok(true)
        }
        "l2_weight_v" => {
            if let Take::Set(parsed) = take_f32("l2_weight_v", value)? {
                config.l2_weight_v = parsed;
            }
            Ok(true)
        }
        "loss_scale_min_pixels" => {
            if let Take::Set(parsed) = take_i32("loss_scale_min_pixels", value)? {
                config.loss_scale_min_pixels = parsed;
            }
            Ok(true)
        }
        "num_loss_scales" => {
            if let Take::Set(parsed) = take_i32("num_loss_scales", value)? {
                config.num_loss_scales = parsed;
            }
            Ok(true)
        }
        "alpha_loss_weight" => {
            if let Take::Set(parsed) = take_f32("alpha_loss_weight", value)? {
                config.alpha_loss_weight = parsed;
            }
            Ok(true)
        }
        "alpha_loss_weight_under" => {
            if let Take::Set(parsed) = take_f32("alpha_loss_weight_under", value)? {
                config.alpha_loss_weight_under = parsed;
            }
            Ok(true)
        }
        "loss_saturation_threshold" => {
            if let Take::Set(parsed) = take_f32("loss_saturation_threshold", value)? {
                config.loss_saturation_threshold = parsed;
            }
            Ok(true)
        }
        "loss_luminance_normalization" => {
            if let Take::Set(parsed) = take_f32("loss_luminance_normalization", value)? {
                config.loss_luminance_normalization = parsed;
            }
            Ok(true)
        }
        "view_sampling" => {
            config.view_sampling = take_string("view_sampling", value)?;
            Ok(true)
        }
        "view_deficit_power" => {
            if let Take::Set(parsed) = take_f32("view_deficit_power", value)? {
                config.view_deficit_power = parsed;
            }
            Ok(true)
        }
        "view_deficit_max_ratio" => {
            if let Take::Set(parsed) = take_f32("view_deficit_max_ratio", value)? {
                config.view_deficit_max_ratio = parsed;
            }
            Ok(true)
        }
        "floater_suppression" => {
            config.floater_suppression = take_string("floater_suppression", value)?;
            Ok(true)
        }
        "depth_distortion_reg" => {
            if let Take::Set(parsed) = take_f32("depth_distortion_reg", value)? {
                config.depth_distortion_reg = parsed;
            }
            Ok(true)
        }
        "normal_distortion_reg" => {
            if let Take::Set(parsed) = take_f32("normal_distortion_reg", value)? {
                config.normal_distortion_reg = parsed;
            }
            Ok(true)
        }
        "rgb_distortion_reg" => {
            if let Take::Set(parsed) = take_f32("rgb_distortion_reg", value)? {
                config.rgb_distortion_reg = parsed;
            }
            Ok(true)
        }
        "distortion_reg_warmup" => {
            if let Take::Set(parsed) = take_i32("distortion_reg_warmup", value)? {
                config.distortion_reg_warmup = parsed;
            }
            Ok(true)
        }
        "normal_reg_weight" => {
            if let Take::Set(parsed) = take_f32("normal_reg_weight", value)? {
                config.normal_reg_weight = parsed;
            }
            Ok(true)
        }
        "normal_reg_warmup" => {
            if let Take::Set(parsed) = take_i32("normal_reg_warmup", value)? {
                config.normal_reg_warmup = parsed;
            }
            Ok(true)
        }
        "alpha_reg_weight" => {
            if let Take::Set(parsed) = take_f32("alpha_reg_weight", value)? {
                config.alpha_reg_weight = parsed;
            }
            Ok(true)
        }
        "alpha_reg_warmup" => {
            if let Take::Set(parsed) = take_i32("alpha_reg_warmup", value)? {
                config.alpha_reg_warmup = parsed;
            }
            Ok(true)
        }
        "reg_warmup_length" => {
            if let Take::Set(parsed) = take_i32("reg_warmup_length", value)? {
                config.reg_warmup_length = parsed;
            }
            Ok(true)
        }
        "depth_supervision_weight" => {
            if let Take::Set(parsed) = take_f32("depth_supervision_weight", value)? {
                config.depth_supervision_weight = parsed;
            }
            Ok(true)
        }
        "normal_supervision_weight" => {
            if let Take::Set(parsed) = take_f32("normal_supervision_weight", value)? {
                config.normal_supervision_weight = parsed;
            }
            Ok(true)
        }
        "input_depth_is_ray_depth" => {
            config.input_depth_is_ray_depth = take_opt_bool("input_depth_is_ray_depth", value)?;
            Ok(true)
        }
        "supervision_warmup" => {
            if let Take::Set(parsed) = take_i32("supervision_warmup", value)? {
                config.supervision_warmup = parsed;
            }
            Ok(true)
        }
        "mean_median_depth_weight" => {
            if let Take::Set(parsed) = take_f32("mean_median_depth_weight", value)? {
                config.mean_median_depth_weight = parsed;
            }
            Ok(true)
        }
        "median_depth_normal_reg_weight" => {
            if let Take::Set(parsed) = take_f32("median_depth_normal_reg_weight", value)? {
                config.median_depth_normal_reg_weight = parsed;
            }
            Ok(true)
        }
        "median_normal_supervision_weight" => {
            if let Take::Set(parsed) = take_f32("median_normal_supervision_weight", value)? {
                config.median_normal_supervision_weight = parsed;
            }
            Ok(true)
        }
        "median_render_normal_reg_weight" => {
            if let Take::Set(parsed) = take_f32("median_render_normal_reg_weight", value)? {
                config.median_render_normal_reg_weight = parsed;
            }
            Ok(true)
        }
        "median_warmup" => {
            if let Take::Set(parsed) = take_i32("median_warmup", value)? {
                config.median_warmup = parsed;
            }
            Ok(true)
        }
        "opacity_reg" => {
            if let Take::Set(parsed) = take_f32("opacity_reg", value)? {
                config.opacity_reg = parsed;
            }
            Ok(true)
        }
        "reg_rendered_only" => {
            if let Take::Set(parsed) = take_bool("reg_rendered_only", value)? {
                config.reg_rendered_only = parsed;
            }
            Ok(true)
        }
        "scale_reg" => {
            if let Take::Set(parsed) = take_f32("scale_reg", value)? {
                config.scale_reg = parsed;
            }
            Ok(true)
        }
        "opacity_reg_decay_power" => {
            if let Take::Set(parsed) = take_f32("opacity_reg_decay_power", value)? {
                config.opacity_reg_decay_power = parsed;
            }
            Ok(true)
        }
        "scale_reg_decay_power" => {
            if let Take::Set(parsed) = take_f32("scale_reg_decay_power", value)? {
                config.scale_reg_decay_power = parsed;
            }
            Ok(true)
        }
        "opacity_decay" => {
            if let Take::Set(parsed) = take_f32("opacity_decay", value)? {
                config.opacity_decay = parsed;
            }
            Ok(true)
        }
        "scale_decay" => {
            if let Take::Set(parsed) = take_f32("scale_decay", value)? {
                config.scale_decay = parsed;
            }
            Ok(true)
        }
        "erank_reg" => {
            if let Take::Set(parsed) = take_f32("erank_reg", value)? {
                config.erank_reg = parsed;
            }
            Ok(true)
        }
        "erank_reg_s3" => {
            if let Take::Set(parsed) = take_f32("erank_reg_s3", value)? {
                config.erank_reg_s3 = parsed;
            }
            Ok(true)
        }
        "scale_regularization_weight" => {
            if let Take::Set(parsed) = take_f32("scale_regularization_weight", value)? {
                config.scale_regularization_weight = parsed;
            }
            Ok(true)
        }
        "max_gauss_ratio" => {
            if let Take::Set(parsed) = take_f32("max_gauss_ratio", value)? {
                config.max_gauss_ratio = parsed;
            }
            Ok(true)
        }
        "dc_reg" => {
            if let Take::Set(parsed) = take_f32("dc_reg", value)? {
                config.dc_reg = parsed;
            }
            Ok(true)
        }
        "sh_reg" => {
            if let Take::Set(parsed) = take_f32("sh_reg", value)? {
                config.sh_reg = parsed;
            }
            Ok(true)
        }
        "overexposure_reg" => {
            if let Take::Set(parsed) = take_f32("overexposure_reg", value)? {
                config.overexposure_reg = parsed;
            }
            Ok(true)
        }
        "quat_norm_reg" => {
            if let Take::Set(parsed) = take_f32("quat_norm_reg", value)? {
                config.quat_norm_reg = parsed;
            }
            Ok(true)
        }
        "use_bilateral_grid" => {
            if let Take::Set(parsed) = take_bool("use_bilateral_grid", value)? {
                config.use_bilateral_grid = parsed;
            }
            Ok(true)
        }
        "bilagrid_type" => {
            config.bilagrid_type = take_string("bilagrid_type", value)?;
            Ok(true)
        }
        "bilagrid_shape" => {
            if let Take::Set(parsed) = take_vec3_i("bilagrid_shape", value)? {
                config.bilagrid_shape = parsed;
            }
            Ok(true)
        }
        "bilagrid_tv_loss_weight" => {
            if let Take::Set(parsed) = take_f32("bilagrid_tv_loss_weight", value)? {
                config.bilagrid_tv_loss_weight = parsed;
            }
            Ok(true)
        }
        "color_shift_reg_weight" => {
            if let Take::Set(parsed) = take_f32("color_shift_reg_weight", value)? {
                config.color_shift_reg_weight = parsed;
            }
            Ok(true)
        }
        "color_shift_reg_ema_period" => {
            if let Take::Set(parsed) = take_i32("color_shift_reg_ema_period", value)? {
                config.color_shift_reg_ema_period = parsed;
            }
            Ok(true)
        }
        "use_bilateral_grid_for_geometry" => {
            if let Take::Set(parsed) = take_bool("use_bilateral_grid_for_geometry", value)? {
                config.use_bilateral_grid_for_geometry = parsed;
            }
            Ok(true)
        }
        "bilagrid_shape_geometry" => {
            if let Take::Set(parsed) = take_vec3_i("bilagrid_shape_geometry", value)? {
                config.bilagrid_shape_geometry = parsed;
            }
            Ok(true)
        }
        "bilagrid_tv_loss_weight_geometry" => {
            if let Take::Set(parsed) = take_f32("bilagrid_tv_loss_weight_geometry", value)? {
                config.bilagrid_tv_loss_weight_geometry = parsed;
            }
            Ok(true)
        }
        "use_adagrad_bilagrid_optim" => {
            if let Take::Set(parsed) = take_bool("use_adagrad_bilagrid_optim", value)? {
                config.use_adagrad_bilagrid_optim = parsed;
            }
            Ok(true)
        }
        "use_ppisp" => {
            if let Take::Set(parsed) = take_bool("use_ppisp", value)? {
                config.use_ppisp = parsed;
            }
            Ok(true)
        }
        "ppisp_param_type" => {
            config.ppisp_param_type = take_string("ppisp_param_type", value)?;
            Ok(true)
        }
        "ppisp_exposure_from_exif" => {
            if let Take::Set(parsed) = take_bool("ppisp_exposure_from_exif", value)? {
                config.ppisp_exposure_from_exif = parsed;
            }
            Ok(true)
        }
        "ppisp_exposure_arithmetic_mean" => {
            if let Take::Set(parsed) = take_bool("ppisp_exposure_arithmetic_mean", value)? {
                config.ppisp_exposure_arithmetic_mean = parsed;
            }
            Ok(true)
        }
        "apply_ppisp_before_bilagrid" => {
            if let Take::Set(parsed) = take_bool("apply_ppisp_before_bilagrid", value)? {
                config.apply_ppisp_before_bilagrid = parsed;
            }
            Ok(true)
        }
        "apply_ppisp_before_color_space" => {
            if let Take::Set(parsed) = take_bool("apply_ppisp_before_color_space", value)? {
                config.apply_ppisp_before_color_space = parsed;
            }
            Ok(true)
        }
        "use_adagrad_ppisp_optim" => {
            if let Take::Set(parsed) = take_bool("use_adagrad_ppisp_optim", value)? {
                config.use_adagrad_ppisp_optim = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_exposure_mean" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_exposure_mean", value)? {
                config.ppisp_reg_exposure_mean = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_color_mean" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_color_mean", value)? {
                config.ppisp_reg_color_mean = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_vig_center" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_vig_center", value)? {
                config.ppisp_reg_vig_center = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_vig_non_pos" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_vig_non_pos", value)? {
                config.ppisp_reg_vig_non_pos = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_vig_channel_var" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_vig_channel_var", value)? {
                config.ppisp_reg_vig_channel_var = parsed;
            }
            Ok(true)
        }
        "ppisp_reg_crf_channel_var" => {
            if let Take::Set(parsed) = take_f32("ppisp_reg_crf_channel_var", value)? {
                config.ppisp_reg_crf_channel_var = parsed;
            }
            Ok(true)
        }
        "bilagrid_adagrad_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_adagrad_lr", value)? {
                config.bilagrid_adagrad_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_adagrad_depth_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_adagrad_depth_lr", value)? {
                config.bilagrid_adagrad_depth_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_adagrad_normal_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_adagrad_normal_lr", value)? {
                config.bilagrid_adagrad_normal_lr = parsed;
            }
            Ok(true)
        }
        "ppisp_adagrad_lr" => {
            if let Take::Set(parsed) = take_f32("ppisp_adagrad_lr", value)? {
                config.ppisp_adagrad_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_lr", value)? {
                config.bilagrid_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_lr_final" => {
            config.bilagrid_lr_final = take_opt_f32("bilagrid_lr_final", value)?;
            Ok(true)
        }
        "bilagrid_lr_warmup" => {
            if let Take::Set(parsed) = take_i32("bilagrid_lr_warmup", value)? {
                config.bilagrid_lr_warmup = parsed;
            }
            Ok(true)
        }
        "bilagrid_depth_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_depth_lr", value)? {
                config.bilagrid_depth_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_depth_lr_final" => {
            config.bilagrid_depth_lr_final = take_opt_f32("bilagrid_depth_lr_final", value)?;
            Ok(true)
        }
        "bilagrid_depth_lr_warmup" => {
            if let Take::Set(parsed) = take_i32("bilagrid_depth_lr_warmup", value)? {
                config.bilagrid_depth_lr_warmup = parsed;
            }
            Ok(true)
        }
        "bilagrid_normal_lr" => {
            if let Take::Set(parsed) = take_f32("bilagrid_normal_lr", value)? {
                config.bilagrid_normal_lr = parsed;
            }
            Ok(true)
        }
        "bilagrid_normal_lr_final" => {
            config.bilagrid_normal_lr_final = take_opt_f32("bilagrid_normal_lr_final", value)?;
            Ok(true)
        }
        "bilagrid_normal_lr_warmup" => {
            if let Take::Set(parsed) = take_i32("bilagrid_normal_lr_warmup", value)? {
                config.bilagrid_normal_lr_warmup = parsed;
            }
            Ok(true)
        }
        "ppisp_lr" => {
            if let Take::Set(parsed) = take_f32("ppisp_lr", value)? {
                config.ppisp_lr = parsed;
            }
            Ok(true)
        }
        "ppisp_lr_final" => {
            config.ppisp_lr_final = take_opt_f32("ppisp_lr_final", value)?;
            Ok(true)
        }
        "ppisp_lr_warmup" => {
            if let Take::Set(parsed) = take_i32("ppisp_lr_warmup", value)? {
                config.ppisp_lr_warmup = parsed;
            }
            Ok(true)
        }
        "image_color_is_linear" => {
            config.image_color_is_linear = take_opt_bool("image_color_is_linear", value)?;
            Ok(true)
        }
        "image_color_transfer" => {
            config.image_color_transfer = take_string("image_color_transfer", value)?;
            Ok(true)
        }
        "image_color_gamut" => {
            config.image_color_gamut = take_string("image_color_gamut", value)?;
            Ok(true)
        }
        "splat_color_is_linear" => {
            config.splat_color_is_linear = take_opt_bool("splat_color_is_linear", value)?;
            Ok(true)
        }
        "splat_color_transfer" => {
            config.splat_color_transfer = take_string("splat_color_transfer", value)?;
            Ok(true)
        }
        "splat_color_gamut" => {
            config.splat_color_gamut = take_string("splat_color_gamut", value)?;
            Ok(true)
        }
        "point_color_is_linear" => {
            config.point_color_is_linear = take_opt_bool("point_color_is_linear", value)?;
            Ok(true)
        }
        "point_color_transfer" => {
            config.point_color_transfer = take_string("point_color_transfer", value)?;
            Ok(true)
        }
        "point_color_gamut" => {
            config.point_color_gamut = take_string("point_color_gamut", value)?;
            Ok(true)
        }
        "cache_images" => {
            config.cache_images = take_string("cache_images", value)?;
            Ok(true)
        }
        "max_batch_per_epoch" => {
            if let Take::Set(parsed) = take_i32("max_batch_per_epoch", value)? {
                config.max_batch_per_epoch = parsed;
            }
            Ok(true)
        }
        "min_renders_per_refine" => {
            if let Take::Set(parsed) = take_f32("min_renders_per_refine", value)? {
                config.min_renders_per_refine = parsed;
            }
            Ok(true)
        }
        "render_quantile" => {
            if let Take::Set(parsed) = take_f32("render_quantile", value)? {
                config.render_quantile = parsed;
            }
            Ok(true)
        }
        "max_train_batch_size" => {
            if let Take::Set(parsed) = take_i32("max_train_batch_size", value)? {
                config.max_train_batch_size = parsed;
            }
            Ok(true)
        }
        "split_batch" => {
            if let Take::Set(parsed) = take_bool("split_batch", value)? {
                config.split_batch = parsed;
            }
            Ok(true)
        }
        "use_fused_proj_bwd_optim" => {
            if let Take::Set(parsed) = take_bool("use_fused_proj_bwd_optim", value)? {
                config.use_fused_proj_bwd_optim = parsed;
            }
            Ok(true)
        }
        "packed" => {
            if let Take::Set(parsed) = take_bool("packed", value)? {
                config.packed = parsed;
            }
            Ok(true)
        }
        "quantization_level" => {
            if let Take::Set(parsed) = take_i32("quantization_level", value)? {
                config.quantization_level = parsed;
            }
            Ok(true)
        }
        "bin_tile_size" => {
            if let Take::Set(parsed) = take_i32("bin_tile_size", value)? {
                config.bin_tile_size = parsed;
            }
            Ok(true)
        }
        "preallocate_splat_tensors" => {
            if let Take::Set(parsed) = take_bool("preallocate_splat_tensors", value)? {
                config.preallocate_splat_tensors = parsed;
            }
            Ok(true)
        }
        "optimizer_offload" => {
            config.optimizer_offload = take_string("optimizer_offload", value)?;
            Ok(true)
        }
        "use_bvh" => {
            if let Take::Set(parsed) = take_bool("use_bvh", value)? {
                config.use_bvh = parsed;
            }
            Ok(true)
        }
        "means_lr" => {
            if let Take::Set(parsed) = take_f32("means_lr", value)? {
                config.means_lr = parsed;
            }
            Ok(true)
        }
        "means_lr_final" => {
            config.means_lr_final = take_opt_f32("means_lr_final", value)?;
            Ok(true)
        }
        "scales_lr" => {
            if let Take::Set(parsed) = take_f32("scales_lr", value)? {
                config.scales_lr = parsed;
            }
            Ok(true)
        }
        "scales_lr_final" => {
            config.scales_lr_final = take_opt_f32("scales_lr_final", value)?;
            Ok(true)
        }
        "quats_lr" => {
            if let Take::Set(parsed) = take_f32("quats_lr", value)? {
                config.quats_lr = parsed;
            }
            Ok(true)
        }
        "opacities_lr" => {
            if let Take::Set(parsed) = take_f32("opacities_lr", value)? {
                config.opacities_lr = parsed;
            }
            Ok(true)
        }
        "features_dc_lr" => {
            if let Take::Set(parsed) = take_f32("features_dc_lr", value)? {
                config.features_dc_lr = parsed;
            }
            Ok(true)
        }
        "features_sh_lr" => {
            if let Take::Set(parsed) = take_f32("features_sh_lr", value)? {
                config.features_sh_lr = parsed;
            }
            Ok(true)
        }
        "features_sb_lr" => {
            if let Take::Set(parsed) = take_f32("features_sb_lr", value)? {
                config.features_sb_lr = parsed;
            }
            Ok(true)
        }
        "background_dc_lr" => {
            if let Take::Set(parsed) = take_f32("background_dc_lr", value)? {
                config.background_dc_lr = parsed;
            }
            Ok(true)
        }
        "background_sh_lr" => {
            if let Take::Set(parsed) = take_f32("background_sh_lr", value)? {
                config.background_sh_lr = parsed;
            }
            Ok(true)
        }
        "max_steps" => {
            config.max_steps = take_opt_i32("max_steps", value)?;
            Ok(true)
        }
        "use_scale_agnostic_mean" => {
            if let Take::Set(parsed) = take_bool("use_scale_agnostic_mean", value)? {
                config.use_scale_agnostic_mean = parsed;
            }
            Ok(true)
        }
        "use_per_splat_bias_correction" => {
            if let Take::Set(parsed) = take_bool("use_per_splat_bias_correction", value)? {
                config.use_per_splat_bias_correction = parsed;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn canonical_field(config: &TrainConfig, name: &str) -> Option<String> {
    match name {
        "data" => Some(canonical_string(&config.data)),
        "resume" => Some(canonical_string(&config.resume)),
        "output_dir_prefix" => Some(canonical_string(&config.output_dir_prefix)),
        "output_dir_name" => Some(canonical_string(&config.output_dir_name)),
        "num_iterations" => Some(canonical_i32(config.num_iterations)),
        "steps_per_save" => Some(canonical_i32(config.steps_per_save)),
        "save_only_latest_checkpoint" => Some(canonical_bool(config.save_only_latest_checkpoint)),
        "save_full_checkpoint" => Some(canonical_bool(config.save_full_checkpoint)),
        "save_eval_images" => Some(canonical_bool(config.save_eval_images)),
        "eval_lpips" => Some(canonical_bool(config.eval_lpips)),
        "lpips_checkpoints" => Some(canonical_string(&config.lpips_checkpoints)),
        "log_performance" => Some(canonical_bool(config.log_performance)),
        "viewer_port" => Some(canonical_i32(config.viewer_port)),
        "disable_viewer" => Some(canonical_bool(config.disable_viewer)),
        "keep_viewer_alive" => Some(canonical_bool(config.keep_viewer_alive)),
        "data_format" => Some(canonical_string(&config.data_format)),
        "image_dir" => Some(canonical_string(&config.image_dir)),
        "mask_dir" => Some(canonical_string(&config.mask_dir)),
        "load_masks" => Some(canonical_bool(config.load_masks)),
        "apply_loss_for_mask" => Some(canonical_opt_bool(config.apply_loss_for_mask)),
        "flip_mask" => Some(canonical_bool(config.flip_mask)),
        "mask_boundary_offset" => Some(canonical_f32(config.mask_boundary_offset)),
        "depth_dir" => Some(canonical_string(&config.depth_dir)),
        "normal_dir" => Some(canonical_string(&config.normal_dir)),
        "load_depths" => Some(canonical_bool(config.load_depths)),
        "load_normals" => Some(canonical_bool(config.load_normals)),
        "depth_unit_scale_factor" => Some(canonical_f32(config.depth_unit_scale_factor)),
        "colmap_recon_dir" => Some(canonical_string(&config.colmap_recon_dir)),
        "metashape_xml" => Some(canonical_string(&config.metashape_xml)),
        "metashape_ply" => Some(canonical_string(&config.metashape_ply)),
        "metashape_psx" => Some(canonical_string(&config.metashape_psx)),
        "seed_pointcloud" => Some(canonical_string(&config.seed_pointcloud)),
        "init_ply" => Some(canonical_string(&config.init_ply)),
        "init_ply_add_points" => Some(canonical_bool(config.init_ply_add_points)),
        "partition" => Some(canonical_string(&config.partition)),
        "partition_part" => Some(canonical_i32(config.partition_part)),
        "roi_region" => Some(canonical_string(&config.roi_region)),
        "roi_outside_weight" => Some(canonical_f32(config.roi_outside_weight)),
        "roi_outside_opacity_decay" => Some(canonical_f32(config.roi_outside_opacity_decay)),
        "roi_splat_samples" => Some(canonical_i32(config.roi_splat_samples)),
        "roi_mask_pixels" => Some(canonical_bool(config.roi_mask_pixels)),
        "train_resolution_divisor" => Some(canonical_f32(config.train_resolution_divisor)),
        "downscale_rounding_mode" => Some(canonical_string(&config.downscale_rounding_mode)),
        "progressive_resolution" => Some(canonical_bool(config.progressive_resolution)),
        "progressive_resolution_start" => Some(canonical_i32(config.progressive_resolution_start)),
        "progressive_resolution_full_at" => {
            Some(canonical_f32(config.progressive_resolution_full_at))
        }
        "progressive_resolution_schedule" => {
            Some(canonical_string(&config.progressive_resolution_schedule))
        }
        "progressive_splat_budget" => Some(canonical_bool(config.progressive_splat_budget)),
        "progressive_splat_budget_schedule" => {
            Some(canonical_string(&config.progressive_splat_budget_schedule))
        }
        "eval_mode" => Some(canonical_string(&config.eval_mode)),
        "eval_interval" => Some(canonical_i32(config.eval_interval)),
        "train_split_fraction" => Some(canonical_f32(config.train_split_fraction)),
        "validation_fraction" => Some(canonical_f32(config.validation_fraction)),
        "warp_to_pinhole" => Some(canonical_bool(config.warp_to_pinhole)),
        "warp_spherical_to_pinhole" => Some(canonical_bool(config.warp_spherical_to_pinhole)),
        "warp_face_fit" => Some(canonical_string(&config.warp_face_fit)),
        "warp_back_face" => Some(canonical_bool(config.warp_back_face)),
        "deblur_training_images" => Some(canonical_bool(config.deblur_training_images)),
        "exif_orientation" => Some(canonical_string(&config.exif_orientation)),
        "orientation_method" => Some(canonical_string(&config.orientation_method)),
        "center_method" => Some(canonical_string(&config.center_method)),
        "auto_scale_poses" => Some(canonical_bool(config.auto_scale_poses)),
        "outlier_threshold" => Some(canonical_f32(config.outlier_threshold)),
        "scene_center" => Some(canonical_string(&config.scene_center)),
        "scene_center_threshold" => Some(canonical_f32(config.scene_center_threshold)),
        "relative_scale" => Some(canonical_opt_f32(config.relative_scale)),
        "train_frame" => Some(canonical_string(&config.train_frame)),
        "primitive" => Some(canonical_string(&config.primitive)),
        "sh_degree" => Some(canonical_i32(config.sh_degree)),
        "sb_lobes" => Some(canonical_i32(config.sb_lobes)),
        "sh_degree_warmup_every" => Some(canonical_i32(config.sh_degree_warmup_every)),
        "background_mode" => Some(canonical_string(&config.background_mode)),
        "background_color" => Some(canonical_vec3_f(&config.background_color)),
        "background_sh_degree" => Some(canonical_i32(config.background_sh_degree)),
        "background_noise_warmup" => Some(canonical_i32(config.background_noise_warmup)),
        "background_noise_pre_warmup" => Some(canonical_f32(config.background_noise_pre_warmup)),
        "background_match_luminance" => Some(canonical_bool(config.background_match_luminance)),
        "scale_init" => Some(canonical_opt_f32(config.scale_init)),
        "opacity_init" => Some(canonical_opt_f32(config.opacity_init)),
        "suppress_initial_scales" => Some(canonical_bool(config.suppress_initial_scales)),
        "random_init" => Some(canonical_string(&config.random_init)),
        "random_init_fraction" => Some(canonical_f32(config.random_init_fraction)),
        "random_init_distribution" => Some(canonical_string(&config.random_init_distribution)),
        "random_init_center" => Some(canonical_string(&config.random_init_center)),
        "random_init_spread" => Some(canonical_string(&config.random_init_spread)),
        "random_init_std" => Some(canonical_f32(config.random_init_std)),
        "use_camera_optimizer" => Some(canonical_bool(config.use_camera_optimizer)),
        "quality" => Some(canonical_string(&config.quality)),
        "cap_max" => Some(canonical_i32(config.cap_max)),
        "distraction_robustness" => Some(canonical_string(&config.distraction_robustness)),
        "min_init_fraction" => Some(canonical_f32(config.min_init_fraction)),
        "growth_factor" => Some(canonical_f32(config.growth_factor)),
        "min_opacity" => Some(canonical_f32(config.min_opacity)),
        "dead_criterion" => Some(canonical_string(&config.dead_criterion)),
        "refine_every" => Some(canonical_i32(config.refine_every)),
        "refine_start_iter" => Some(canonical_i32(config.refine_start_iter)),
        "refine_stop_num_iter" => Some(canonical_i32(config.refine_stop_num_iter)),
        "refine_stop_iter" => Some(canonical_i32(config.refine_stop_iter)),
        "noise_lr" => Some(canonical_f32(config.noise_lr)),
        "noise_lr_final" => Some(canonical_f32(config.noise_lr_final)),
        "use_revised_densification" => Some(canonical_bool(config.use_revised_densification)),
        "densify_score_mode" => Some(canonical_string(&config.densify_score_mode)),
        "densify_score_blend_world_grad" => {
            Some(canonical_f32(config.densify_score_blend_world_grad))
        }
        "densify_loss_map_mode" => Some(canonical_string(&config.densify_loss_map_mode)),
        "densify_robust_edge_aware_quantile" => {
            Some(canonical_f32(config.densify_robust_edge_aware_quantile))
        }
        "densify_nms_falloff" => Some(canonical_f32(config.densify_nms_falloff)),
        "densify_loss_map_normalize" => Some(canonical_bool(config.densify_loss_map_normalize)),
        "densify_loss_map_clip_quantile" => {
            Some(canonical_f32(config.densify_loss_map_clip_quantile))
        }
        "densify_loss_map_power" => Some(canonical_f32(config.densify_loss_map_power)),
        "densify_accum_mode" => Some(canonical_string(&config.densify_accum_mode)),
        "densify_score_power" => Some(canonical_f32(config.densify_score_power)),
        "densify_score_clip_quantile" => Some(canonical_f32(config.densify_score_clip_quantile)),
        "densify_final_score_power" => Some(canonical_f32(config.densify_final_score_power)),
        "densify_learned_score" => Some(canonical_bool(config.densify_learned_score)),
        "densify_oversize_split_fraction" => {
            Some(canonical_f32(config.densify_oversize_split_fraction))
        }
        "densify_oversize_score_blend" => Some(canonical_f32(config.densify_oversize_score_blend)),
        "use_long_axis_split" => Some(canonical_bool(config.use_long_axis_split)),
        "long_axis_split_opacity_k" => Some(canonical_vec3_f(&config.long_axis_split_opacity_k)),
        "max_split_fraction" => Some(canonical_f32(config.max_split_fraction)),
        "split_weight_by_renders" => Some(canonical_bool(config.split_weight_by_renders)),
        "dead_after_epochs" => Some(canonical_f32(config.dead_after_epochs)),
        "max_screen_size" => Some(canonical_f32(config.max_screen_size)),
        "max_screen_size_clip_hardness" => {
            Some(canonical_f32(config.max_screen_size_clip_hardness))
        }
        "max_screen_size_penalty" => Some(canonical_f32(config.max_screen_size_penalty)),
        "max_world_size" => Some(canonical_f32(config.max_world_size)),
        "ssim_lambda" => Some(canonical_f32(config.ssim_lambda)),
        "perceptual_loss" => Some(canonical_string(&config.perceptual_loss)),
        "l1_weight" => Some(canonical_f32(config.l1_weight)),
        "l2_weight" => Some(canonical_f32(config.l2_weight)),
        "l1_weight_y" => Some(canonical_f32(config.l1_weight_y)),
        "l2_weight_y" => Some(canonical_f32(config.l2_weight_y)),
        "l2_weight_u" => Some(canonical_f32(config.l2_weight_u)),
        "l2_weight_v" => Some(canonical_f32(config.l2_weight_v)),
        "loss_scale_min_pixels" => Some(canonical_i32(config.loss_scale_min_pixels)),
        "num_loss_scales" => Some(canonical_i32(config.num_loss_scales)),
        "alpha_loss_weight" => Some(canonical_f32(config.alpha_loss_weight)),
        "alpha_loss_weight_under" => Some(canonical_f32(config.alpha_loss_weight_under)),
        "loss_saturation_threshold" => Some(canonical_f32(config.loss_saturation_threshold)),
        "loss_luminance_normalization" => Some(canonical_f32(config.loss_luminance_normalization)),
        "view_sampling" => Some(canonical_string(&config.view_sampling)),
        "view_deficit_power" => Some(canonical_f32(config.view_deficit_power)),
        "view_deficit_max_ratio" => Some(canonical_f32(config.view_deficit_max_ratio)),
        "floater_suppression" => Some(canonical_string(&config.floater_suppression)),
        "depth_distortion_reg" => Some(canonical_f32(config.depth_distortion_reg)),
        "normal_distortion_reg" => Some(canonical_f32(config.normal_distortion_reg)),
        "rgb_distortion_reg" => Some(canonical_f32(config.rgb_distortion_reg)),
        "distortion_reg_warmup" => Some(canonical_i32(config.distortion_reg_warmup)),
        "normal_reg_weight" => Some(canonical_f32(config.normal_reg_weight)),
        "normal_reg_warmup" => Some(canonical_i32(config.normal_reg_warmup)),
        "alpha_reg_weight" => Some(canonical_f32(config.alpha_reg_weight)),
        "alpha_reg_warmup" => Some(canonical_i32(config.alpha_reg_warmup)),
        "reg_warmup_length" => Some(canonical_i32(config.reg_warmup_length)),
        "depth_supervision_weight" => Some(canonical_f32(config.depth_supervision_weight)),
        "normal_supervision_weight" => Some(canonical_f32(config.normal_supervision_weight)),
        "input_depth_is_ray_depth" => Some(canonical_opt_bool(config.input_depth_is_ray_depth)),
        "supervision_warmup" => Some(canonical_i32(config.supervision_warmup)),
        "mean_median_depth_weight" => Some(canonical_f32(config.mean_median_depth_weight)),
        "median_depth_normal_reg_weight" => {
            Some(canonical_f32(config.median_depth_normal_reg_weight))
        }
        "median_normal_supervision_weight" => {
            Some(canonical_f32(config.median_normal_supervision_weight))
        }
        "median_render_normal_reg_weight" => {
            Some(canonical_f32(config.median_render_normal_reg_weight))
        }
        "median_warmup" => Some(canonical_i32(config.median_warmup)),
        "opacity_reg" => Some(canonical_f32(config.opacity_reg)),
        "reg_rendered_only" => Some(canonical_bool(config.reg_rendered_only)),
        "scale_reg" => Some(canonical_f32(config.scale_reg)),
        "opacity_reg_decay_power" => Some(canonical_f32(config.opacity_reg_decay_power)),
        "scale_reg_decay_power" => Some(canonical_f32(config.scale_reg_decay_power)),
        "opacity_decay" => Some(canonical_f32(config.opacity_decay)),
        "scale_decay" => Some(canonical_f32(config.scale_decay)),
        "erank_reg" => Some(canonical_f32(config.erank_reg)),
        "erank_reg_s3" => Some(canonical_f32(config.erank_reg_s3)),
        "scale_regularization_weight" => Some(canonical_f32(config.scale_regularization_weight)),
        "max_gauss_ratio" => Some(canonical_f32(config.max_gauss_ratio)),
        "dc_reg" => Some(canonical_f32(config.dc_reg)),
        "sh_reg" => Some(canonical_f32(config.sh_reg)),
        "overexposure_reg" => Some(canonical_f32(config.overexposure_reg)),
        "quat_norm_reg" => Some(canonical_f32(config.quat_norm_reg)),
        "use_bilateral_grid" => Some(canonical_bool(config.use_bilateral_grid)),
        "bilagrid_type" => Some(canonical_string(&config.bilagrid_type)),
        "bilagrid_shape" => Some(canonical_vec3_i(&config.bilagrid_shape)),
        "bilagrid_tv_loss_weight" => Some(canonical_f32(config.bilagrid_tv_loss_weight)),
        "color_shift_reg_weight" => Some(canonical_f32(config.color_shift_reg_weight)),
        "color_shift_reg_ema_period" => Some(canonical_i32(config.color_shift_reg_ema_period)),
        "use_bilateral_grid_for_geometry" => {
            Some(canonical_bool(config.use_bilateral_grid_for_geometry))
        }
        "bilagrid_shape_geometry" => Some(canonical_vec3_i(&config.bilagrid_shape_geometry)),
        "bilagrid_tv_loss_weight_geometry" => {
            Some(canonical_f32(config.bilagrid_tv_loss_weight_geometry))
        }
        "use_adagrad_bilagrid_optim" => Some(canonical_bool(config.use_adagrad_bilagrid_optim)),
        "use_ppisp" => Some(canonical_bool(config.use_ppisp)),
        "ppisp_param_type" => Some(canonical_string(&config.ppisp_param_type)),
        "ppisp_exposure_from_exif" => Some(canonical_bool(config.ppisp_exposure_from_exif)),
        "ppisp_exposure_arithmetic_mean" => {
            Some(canonical_bool(config.ppisp_exposure_arithmetic_mean))
        }
        "apply_ppisp_before_bilagrid" => Some(canonical_bool(config.apply_ppisp_before_bilagrid)),
        "apply_ppisp_before_color_space" => {
            Some(canonical_bool(config.apply_ppisp_before_color_space))
        }
        "use_adagrad_ppisp_optim" => Some(canonical_bool(config.use_adagrad_ppisp_optim)),
        "ppisp_reg_exposure_mean" => Some(canonical_f32(config.ppisp_reg_exposure_mean)),
        "ppisp_reg_color_mean" => Some(canonical_f32(config.ppisp_reg_color_mean)),
        "ppisp_reg_vig_center" => Some(canonical_f32(config.ppisp_reg_vig_center)),
        "ppisp_reg_vig_non_pos" => Some(canonical_f32(config.ppisp_reg_vig_non_pos)),
        "ppisp_reg_vig_channel_var" => Some(canonical_f32(config.ppisp_reg_vig_channel_var)),
        "ppisp_reg_crf_channel_var" => Some(canonical_f32(config.ppisp_reg_crf_channel_var)),
        "bilagrid_adagrad_lr" => Some(canonical_f32(config.bilagrid_adagrad_lr)),
        "bilagrid_adagrad_depth_lr" => Some(canonical_f32(config.bilagrid_adagrad_depth_lr)),
        "bilagrid_adagrad_normal_lr" => Some(canonical_f32(config.bilagrid_adagrad_normal_lr)),
        "ppisp_adagrad_lr" => Some(canonical_f32(config.ppisp_adagrad_lr)),
        "bilagrid_lr" => Some(canonical_f32(config.bilagrid_lr)),
        "bilagrid_lr_final" => Some(canonical_opt_f32(config.bilagrid_lr_final)),
        "bilagrid_lr_warmup" => Some(canonical_i32(config.bilagrid_lr_warmup)),
        "bilagrid_depth_lr" => Some(canonical_f32(config.bilagrid_depth_lr)),
        "bilagrid_depth_lr_final" => Some(canonical_opt_f32(config.bilagrid_depth_lr_final)),
        "bilagrid_depth_lr_warmup" => Some(canonical_i32(config.bilagrid_depth_lr_warmup)),
        "bilagrid_normal_lr" => Some(canonical_f32(config.bilagrid_normal_lr)),
        "bilagrid_normal_lr_final" => Some(canonical_opt_f32(config.bilagrid_normal_lr_final)),
        "bilagrid_normal_lr_warmup" => Some(canonical_i32(config.bilagrid_normal_lr_warmup)),
        "ppisp_lr" => Some(canonical_f32(config.ppisp_lr)),
        "ppisp_lr_final" => Some(canonical_opt_f32(config.ppisp_lr_final)),
        "ppisp_lr_warmup" => Some(canonical_i32(config.ppisp_lr_warmup)),
        "image_color_is_linear" => Some(canonical_opt_bool(config.image_color_is_linear)),
        "image_color_transfer" => Some(canonical_string(&config.image_color_transfer)),
        "image_color_gamut" => Some(canonical_string(&config.image_color_gamut)),
        "splat_color_is_linear" => Some(canonical_opt_bool(config.splat_color_is_linear)),
        "splat_color_transfer" => Some(canonical_string(&config.splat_color_transfer)),
        "splat_color_gamut" => Some(canonical_string(&config.splat_color_gamut)),
        "point_color_is_linear" => Some(canonical_opt_bool(config.point_color_is_linear)),
        "point_color_transfer" => Some(canonical_string(&config.point_color_transfer)),
        "point_color_gamut" => Some(canonical_string(&config.point_color_gamut)),
        "cache_images" => Some(canonical_string(&config.cache_images)),
        "max_batch_per_epoch" => Some(canonical_i32(config.max_batch_per_epoch)),
        "min_renders_per_refine" => Some(canonical_f32(config.min_renders_per_refine)),
        "render_quantile" => Some(canonical_f32(config.render_quantile)),
        "max_train_batch_size" => Some(canonical_i32(config.max_train_batch_size)),
        "split_batch" => Some(canonical_bool(config.split_batch)),
        "use_fused_proj_bwd_optim" => Some(canonical_bool(config.use_fused_proj_bwd_optim)),
        "packed" => Some(canonical_bool(config.packed)),
        "quantization_level" => Some(canonical_i32(config.quantization_level)),
        "bin_tile_size" => Some(canonical_i32(config.bin_tile_size)),
        "preallocate_splat_tensors" => Some(canonical_bool(config.preallocate_splat_tensors)),
        "optimizer_offload" => Some(canonical_string(&config.optimizer_offload)),
        "use_bvh" => Some(canonical_bool(config.use_bvh)),
        "means_lr" => Some(canonical_f32(config.means_lr)),
        "means_lr_final" => Some(canonical_opt_f32(config.means_lr_final)),
        "scales_lr" => Some(canonical_f32(config.scales_lr)),
        "scales_lr_final" => Some(canonical_opt_f32(config.scales_lr_final)),
        "quats_lr" => Some(canonical_f32(config.quats_lr)),
        "opacities_lr" => Some(canonical_f32(config.opacities_lr)),
        "features_dc_lr" => Some(canonical_f32(config.features_dc_lr)),
        "features_sh_lr" => Some(canonical_f32(config.features_sh_lr)),
        "features_sb_lr" => Some(canonical_f32(config.features_sb_lr)),
        "background_dc_lr" => Some(canonical_f32(config.background_dc_lr)),
        "background_sh_lr" => Some(canonical_f32(config.background_sh_lr)),
        "max_steps" => Some(canonical_opt_i32(config.max_steps)),
        "use_scale_agnostic_mean" => Some(canonical_bool(config.use_scale_agnostic_mean)),
        "use_per_splat_bias_correction" => {
            Some(canonical_bool(config.use_per_splat_bias_correction))
        }
        _ => None,
    }
}

fn check_writable_string(spec: &FieldSpec, value: &str) -> Result<(), DomainError> {
    if value.is_empty() {
        return Ok(());
    }
    check_choice(spec, value)
}

pub(crate) fn validate_fields(config: &TrainConfig) -> Result<(), DomainError> {
    check_writable_string(field_spec("data").expect("data"), &config.data)?;
    check_writable_string(field_spec("resume").expect("resume"), &config.resume)?;
    check_writable_string(
        field_spec("output_dir_prefix").expect("output_dir_prefix"),
        &config.output_dir_prefix,
    )?;
    check_writable_string(
        field_spec("output_dir_name").expect("output_dir_name"),
        &config.output_dir_name,
    )?;
    check_writable_string(
        field_spec("lpips_checkpoints").expect("lpips_checkpoints"),
        &config.lpips_checkpoints,
    )?;
    check_writable_string(
        field_spec("data_format").expect("data_format"),
        &config.data_format,
    )?;
    check_writable_string(
        field_spec("image_dir").expect("image_dir"),
        &config.image_dir,
    )?;
    check_writable_string(field_spec("mask_dir").expect("mask_dir"), &config.mask_dir)?;
    check_writable_string(
        field_spec("depth_dir").expect("depth_dir"),
        &config.depth_dir,
    )?;
    check_writable_string(
        field_spec("normal_dir").expect("normal_dir"),
        &config.normal_dir,
    )?;
    check_writable_string(
        field_spec("colmap_recon_dir").expect("colmap_recon_dir"),
        &config.colmap_recon_dir,
    )?;
    check_writable_string(
        field_spec("metashape_xml").expect("metashape_xml"),
        &config.metashape_xml,
    )?;
    check_writable_string(
        field_spec("metashape_ply").expect("metashape_ply"),
        &config.metashape_ply,
    )?;
    check_writable_string(
        field_spec("metashape_psx").expect("metashape_psx"),
        &config.metashape_psx,
    )?;
    check_writable_string(
        field_spec("seed_pointcloud").expect("seed_pointcloud"),
        &config.seed_pointcloud,
    )?;
    check_writable_string(field_spec("init_ply").expect("init_ply"), &config.init_ply)?;
    check_writable_string(
        field_spec("partition").expect("partition"),
        &config.partition,
    )?;
    check_writable_string(
        field_spec("roi_region").expect("roi_region"),
        &config.roi_region,
    )?;
    check_writable_string(
        field_spec("downscale_rounding_mode").expect("downscale_rounding_mode"),
        &config.downscale_rounding_mode,
    )?;
    check_writable_string(
        field_spec("progressive_resolution_schedule").expect("progressive_resolution_schedule"),
        &config.progressive_resolution_schedule,
    )?;
    check_writable_string(
        field_spec("progressive_splat_budget_schedule").expect("progressive_splat_budget_schedule"),
        &config.progressive_splat_budget_schedule,
    )?;
    check_writable_string(
        field_spec("eval_mode").expect("eval_mode"),
        &config.eval_mode,
    )?;
    check_writable_string(
        field_spec("warp_face_fit").expect("warp_face_fit"),
        &config.warp_face_fit,
    )?;
    check_writable_string(
        field_spec("exif_orientation").expect("exif_orientation"),
        &config.exif_orientation,
    )?;
    check_writable_string(
        field_spec("orientation_method").expect("orientation_method"),
        &config.orientation_method,
    )?;
    check_writable_string(
        field_spec("center_method").expect("center_method"),
        &config.center_method,
    )?;
    check_writable_string(
        field_spec("scene_center").expect("scene_center"),
        &config.scene_center,
    )?;
    check_writable_string(
        field_spec("train_frame").expect("train_frame"),
        &config.train_frame,
    )?;
    check_writable_string(
        field_spec("primitive").expect("primitive"),
        &config.primitive,
    )?;
    check_writable_string(
        field_spec("background_mode").expect("background_mode"),
        &config.background_mode,
    )?;
    check_writable_string(
        field_spec("random_init").expect("random_init"),
        &config.random_init,
    )?;
    check_writable_string(
        field_spec("random_init_distribution").expect("random_init_distribution"),
        &config.random_init_distribution,
    )?;
    check_writable_string(
        field_spec("random_init_center").expect("random_init_center"),
        &config.random_init_center,
    )?;
    check_writable_string(
        field_spec("random_init_spread").expect("random_init_spread"),
        &config.random_init_spread,
    )?;
    check_writable_string(field_spec("quality").expect("quality"), &config.quality)?;
    check_writable_string(
        field_spec("distraction_robustness").expect("distraction_robustness"),
        &config.distraction_robustness,
    )?;
    check_writable_string(
        field_spec("dead_criterion").expect("dead_criterion"),
        &config.dead_criterion,
    )?;
    check_writable_string(
        field_spec("densify_score_mode").expect("densify_score_mode"),
        &config.densify_score_mode,
    )?;
    check_writable_string(
        field_spec("densify_loss_map_mode").expect("densify_loss_map_mode"),
        &config.densify_loss_map_mode,
    )?;
    check_writable_string(
        field_spec("densify_accum_mode").expect("densify_accum_mode"),
        &config.densify_accum_mode,
    )?;
    check_writable_string(
        field_spec("perceptual_loss").expect("perceptual_loss"),
        &config.perceptual_loss,
    )?;
    check_writable_string(
        field_spec("view_sampling").expect("view_sampling"),
        &config.view_sampling,
    )?;
    check_writable_string(
        field_spec("floater_suppression").expect("floater_suppression"),
        &config.floater_suppression,
    )?;
    check_writable_string(
        field_spec("bilagrid_type").expect("bilagrid_type"),
        &config.bilagrid_type,
    )?;
    check_writable_string(
        field_spec("ppisp_param_type").expect("ppisp_param_type"),
        &config.ppisp_param_type,
    )?;
    check_writable_string(
        field_spec("image_color_transfer").expect("image_color_transfer"),
        &config.image_color_transfer,
    )?;
    check_writable_string(
        field_spec("image_color_gamut").expect("image_color_gamut"),
        &config.image_color_gamut,
    )?;
    check_writable_string(
        field_spec("splat_color_transfer").expect("splat_color_transfer"),
        &config.splat_color_transfer,
    )?;
    check_writable_string(
        field_spec("splat_color_gamut").expect("splat_color_gamut"),
        &config.splat_color_gamut,
    )?;
    check_writable_string(
        field_spec("point_color_transfer").expect("point_color_transfer"),
        &config.point_color_transfer,
    )?;
    check_writable_string(
        field_spec("point_color_gamut").expect("point_color_gamut"),
        &config.point_color_gamut,
    )?;
    check_writable_string(
        field_spec("cache_images").expect("cache_images"),
        &config.cache_images,
    )?;
    check_writable_string(
        field_spec("optimizer_offload").expect("optimizer_offload"),
        &config.optimizer_offload,
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn fill_probe(config: &mut TrainConfig) {
    config.data = probe_string("data", &config.data);
    config.resume = probe_string("resume", &config.resume);
    config.output_dir_prefix = probe_string("output_dir_prefix", &config.output_dir_prefix);
    config.output_dir_name = probe_string("output_dir_name", &config.output_dir_name);
    config.num_iterations = probe_i32(config.num_iterations);
    config.steps_per_save = probe_i32(config.steps_per_save);
    config.save_only_latest_checkpoint = !config.save_only_latest_checkpoint;
    config.save_full_checkpoint = !config.save_full_checkpoint;
    config.save_eval_images = !config.save_eval_images;
    config.eval_lpips = !config.eval_lpips;
    config.lpips_checkpoints = probe_string("lpips_checkpoints", &config.lpips_checkpoints);
    config.log_performance = !config.log_performance;
    config.viewer_port = probe_i32(config.viewer_port);
    config.disable_viewer = !config.disable_viewer;
    config.keep_viewer_alive = !config.keep_viewer_alive;
    config.data_format = probe_string("data_format", &config.data_format);
    config.image_dir = probe_string("image_dir", &config.image_dir);
    config.mask_dir = probe_string("mask_dir", &config.mask_dir);
    config.load_masks = !config.load_masks;
    config.apply_loss_for_mask = Some(!config.apply_loss_for_mask.unwrap_or(false));
    config.flip_mask = !config.flip_mask;
    config.mask_boundary_offset = probe_f32(config.mask_boundary_offset);
    config.depth_dir = probe_string("depth_dir", &config.depth_dir);
    config.normal_dir = probe_string("normal_dir", &config.normal_dir);
    config.load_depths = !config.load_depths;
    config.load_normals = !config.load_normals;
    config.depth_unit_scale_factor = probe_f32(config.depth_unit_scale_factor);
    config.colmap_recon_dir = probe_string("colmap_recon_dir", &config.colmap_recon_dir);
    config.metashape_xml = probe_string("metashape_xml", &config.metashape_xml);
    config.metashape_ply = probe_string("metashape_ply", &config.metashape_ply);
    config.metashape_psx = probe_string("metashape_psx", &config.metashape_psx);
    config.seed_pointcloud = probe_string("seed_pointcloud", &config.seed_pointcloud);
    config.init_ply = probe_string("init_ply", &config.init_ply);
    config.init_ply_add_points = !config.init_ply_add_points;
    config.partition = probe_string("partition", &config.partition);
    config.partition_part = probe_i32(config.partition_part);
    config.roi_region = probe_string("roi_region", &config.roi_region);
    config.roi_outside_weight = probe_f32(config.roi_outside_weight);
    config.roi_outside_opacity_decay = probe_f32(config.roi_outside_opacity_decay);
    config.roi_splat_samples = probe_i32(config.roi_splat_samples);
    config.roi_mask_pixels = !config.roi_mask_pixels;
    config.train_resolution_divisor = probe_f32(config.train_resolution_divisor);
    config.downscale_rounding_mode =
        probe_string("downscale_rounding_mode", &config.downscale_rounding_mode);
    config.progressive_resolution = !config.progressive_resolution;
    config.progressive_resolution_start = probe_i32(config.progressive_resolution_start);
    config.progressive_resolution_full_at = probe_f32(config.progressive_resolution_full_at);
    config.progressive_resolution_schedule = probe_string(
        "progressive_resolution_schedule",
        &config.progressive_resolution_schedule,
    );
    config.progressive_splat_budget = !config.progressive_splat_budget;
    config.progressive_splat_budget_schedule = probe_string(
        "progressive_splat_budget_schedule",
        &config.progressive_splat_budget_schedule,
    );
    config.eval_mode = probe_string("eval_mode", &config.eval_mode);
    config.eval_interval = probe_i32(config.eval_interval);
    config.train_split_fraction = probe_f32(config.train_split_fraction);
    config.validation_fraction = probe_f32(config.validation_fraction);
    config.warp_to_pinhole = !config.warp_to_pinhole;
    config.warp_spherical_to_pinhole = !config.warp_spherical_to_pinhole;
    config.warp_face_fit = probe_string("warp_face_fit", &config.warp_face_fit);
    config.warp_back_face = !config.warp_back_face;
    config.deblur_training_images = !config.deblur_training_images;
    config.exif_orientation = probe_string("exif_orientation", &config.exif_orientation);
    config.orientation_method = probe_string("orientation_method", &config.orientation_method);
    config.center_method = probe_string("center_method", &config.center_method);
    config.auto_scale_poses = !config.auto_scale_poses;
    config.outlier_threshold = probe_f32(config.outlier_threshold);
    config.scene_center = probe_string("scene_center", &config.scene_center);
    config.scene_center_threshold = probe_f32(config.scene_center_threshold);
    config.relative_scale = Some(probe_f32(config.relative_scale.unwrap_or(0.0)));
    config.train_frame = probe_string("train_frame", &config.train_frame);
    config.primitive = probe_string("primitive", &config.primitive);
    config.sh_degree = probe_i32(config.sh_degree);
    config.sb_lobes = probe_i32(config.sb_lobes);
    config.sh_degree_warmup_every = probe_i32(config.sh_degree_warmup_every);
    config.background_mode = probe_string("background_mode", &config.background_mode);
    config.background_color[0] = probe_f32(config.background_color[0]);
    config.background_sh_degree = probe_i32(config.background_sh_degree);
    config.background_noise_warmup = probe_i32(config.background_noise_warmup);
    config.background_noise_pre_warmup = probe_f32(config.background_noise_pre_warmup);
    config.background_match_luminance = !config.background_match_luminance;
    config.scale_init = Some(probe_f32(config.scale_init.unwrap_or(0.0)));
    config.opacity_init = Some(probe_f32(config.opacity_init.unwrap_or(0.0)));
    config.suppress_initial_scales = !config.suppress_initial_scales;
    config.random_init = probe_string("random_init", &config.random_init);
    config.random_init_fraction = probe_f32(config.random_init_fraction);
    config.random_init_distribution =
        probe_string("random_init_distribution", &config.random_init_distribution);
    config.random_init_center = probe_string("random_init_center", &config.random_init_center);
    config.random_init_spread = probe_string("random_init_spread", &config.random_init_spread);
    config.random_init_std = probe_f32(config.random_init_std);
    config.use_camera_optimizer = !config.use_camera_optimizer;
    config.quality = probe_string("quality", &config.quality);
    config.cap_max = probe_i32(config.cap_max);
    config.distraction_robustness =
        probe_string("distraction_robustness", &config.distraction_robustness);
    config.min_init_fraction = probe_f32(config.min_init_fraction);
    config.growth_factor = probe_f32(config.growth_factor);
    config.min_opacity = probe_f32(config.min_opacity);
    config.dead_criterion = probe_string("dead_criterion", &config.dead_criterion);
    config.refine_every = probe_i32(config.refine_every);
    config.refine_start_iter = probe_i32(config.refine_start_iter);
    config.refine_stop_num_iter = probe_i32(config.refine_stop_num_iter);
    config.refine_stop_iter = probe_i32(config.refine_stop_iter);
    config.noise_lr = probe_f32(config.noise_lr);
    config.noise_lr_final = probe_f32(config.noise_lr_final);
    config.use_revised_densification = !config.use_revised_densification;
    config.densify_score_mode = probe_string("densify_score_mode", &config.densify_score_mode);
    config.densify_score_blend_world_grad = probe_f32(config.densify_score_blend_world_grad);
    config.densify_loss_map_mode =
        probe_string("densify_loss_map_mode", &config.densify_loss_map_mode);
    config.densify_robust_edge_aware_quantile =
        probe_f32(config.densify_robust_edge_aware_quantile);
    config.densify_nms_falloff = probe_f32(config.densify_nms_falloff);
    config.densify_loss_map_normalize = !config.densify_loss_map_normalize;
    config.densify_loss_map_clip_quantile = probe_f32(config.densify_loss_map_clip_quantile);
    config.densify_loss_map_power = probe_f32(config.densify_loss_map_power);
    config.densify_accum_mode = probe_string("densify_accum_mode", &config.densify_accum_mode);
    config.densify_score_power = probe_f32(config.densify_score_power);
    config.densify_score_clip_quantile = probe_f32(config.densify_score_clip_quantile);
    config.densify_final_score_power = probe_f32(config.densify_final_score_power);
    config.densify_learned_score = !config.densify_learned_score;
    config.densify_oversize_split_fraction = probe_f32(config.densify_oversize_split_fraction);
    config.densify_oversize_score_blend = probe_f32(config.densify_oversize_score_blend);
    config.use_long_axis_split = !config.use_long_axis_split;
    config.long_axis_split_opacity_k[0] = probe_f32(config.long_axis_split_opacity_k[0]);
    config.max_split_fraction = probe_f32(config.max_split_fraction);
    config.split_weight_by_renders = !config.split_weight_by_renders;
    config.dead_after_epochs = probe_f32(config.dead_after_epochs);
    config.max_screen_size = probe_f32(config.max_screen_size);
    config.max_screen_size_clip_hardness = probe_f32(config.max_screen_size_clip_hardness);
    config.max_screen_size_penalty = probe_f32(config.max_screen_size_penalty);
    config.max_world_size = probe_f32(config.max_world_size);
    config.ssim_lambda = probe_f32(config.ssim_lambda);
    config.perceptual_loss = probe_string("perceptual_loss", &config.perceptual_loss);
    config.l1_weight = probe_f32(config.l1_weight);
    config.l2_weight = probe_f32(config.l2_weight);
    config.l1_weight_y = probe_f32(config.l1_weight_y);
    config.l2_weight_y = probe_f32(config.l2_weight_y);
    config.l2_weight_u = probe_f32(config.l2_weight_u);
    config.l2_weight_v = probe_f32(config.l2_weight_v);
    config.loss_scale_min_pixels = probe_i32(config.loss_scale_min_pixels);
    config.num_loss_scales = probe_i32(config.num_loss_scales);
    config.alpha_loss_weight = probe_f32(config.alpha_loss_weight);
    config.alpha_loss_weight_under = probe_f32(config.alpha_loss_weight_under);
    config.loss_saturation_threshold = probe_f32(config.loss_saturation_threshold);
    config.loss_luminance_normalization = probe_f32(config.loss_luminance_normalization);
    config.view_sampling = probe_string("view_sampling", &config.view_sampling);
    config.view_deficit_power = probe_f32(config.view_deficit_power);
    config.view_deficit_max_ratio = probe_f32(config.view_deficit_max_ratio);
    config.floater_suppression = probe_string("floater_suppression", &config.floater_suppression);
    config.depth_distortion_reg = probe_f32(config.depth_distortion_reg);
    config.normal_distortion_reg = probe_f32(config.normal_distortion_reg);
    config.rgb_distortion_reg = probe_f32(config.rgb_distortion_reg);
    config.distortion_reg_warmup = probe_i32(config.distortion_reg_warmup);
    config.normal_reg_weight = probe_f32(config.normal_reg_weight);
    config.normal_reg_warmup = probe_i32(config.normal_reg_warmup);
    config.alpha_reg_weight = probe_f32(config.alpha_reg_weight);
    config.alpha_reg_warmup = probe_i32(config.alpha_reg_warmup);
    config.reg_warmup_length = probe_i32(config.reg_warmup_length);
    config.depth_supervision_weight = probe_f32(config.depth_supervision_weight);
    config.normal_supervision_weight = probe_f32(config.normal_supervision_weight);
    config.input_depth_is_ray_depth = Some(!config.input_depth_is_ray_depth.unwrap_or(false));
    config.supervision_warmup = probe_i32(config.supervision_warmup);
    config.mean_median_depth_weight = probe_f32(config.mean_median_depth_weight);
    config.median_depth_normal_reg_weight = probe_f32(config.median_depth_normal_reg_weight);
    config.median_normal_supervision_weight = probe_f32(config.median_normal_supervision_weight);
    config.median_render_normal_reg_weight = probe_f32(config.median_render_normal_reg_weight);
    config.median_warmup = probe_i32(config.median_warmup);
    config.opacity_reg = probe_f32(config.opacity_reg);
    config.reg_rendered_only = !config.reg_rendered_only;
    config.scale_reg = probe_f32(config.scale_reg);
    config.opacity_reg_decay_power = probe_f32(config.opacity_reg_decay_power);
    config.scale_reg_decay_power = probe_f32(config.scale_reg_decay_power);
    config.opacity_decay = probe_f32(config.opacity_decay);
    config.scale_decay = probe_f32(config.scale_decay);
    config.erank_reg = probe_f32(config.erank_reg);
    config.erank_reg_s3 = probe_f32(config.erank_reg_s3);
    config.scale_regularization_weight = probe_f32(config.scale_regularization_weight);
    config.max_gauss_ratio = probe_f32(config.max_gauss_ratio);
    config.dc_reg = probe_f32(config.dc_reg);
    config.sh_reg = probe_f32(config.sh_reg);
    config.overexposure_reg = probe_f32(config.overexposure_reg);
    config.quat_norm_reg = probe_f32(config.quat_norm_reg);
    config.use_bilateral_grid = !config.use_bilateral_grid;
    config.bilagrid_type = probe_string("bilagrid_type", &config.bilagrid_type);
    config.bilagrid_shape[0] = probe_i32(config.bilagrid_shape[0]);
    config.bilagrid_tv_loss_weight = probe_f32(config.bilagrid_tv_loss_weight);
    config.color_shift_reg_weight = probe_f32(config.color_shift_reg_weight);
    config.color_shift_reg_ema_period = probe_i32(config.color_shift_reg_ema_period);
    config.use_bilateral_grid_for_geometry = !config.use_bilateral_grid_for_geometry;
    config.bilagrid_shape_geometry[0] = probe_i32(config.bilagrid_shape_geometry[0]);
    config.bilagrid_tv_loss_weight_geometry = probe_f32(config.bilagrid_tv_loss_weight_geometry);
    config.use_adagrad_bilagrid_optim = !config.use_adagrad_bilagrid_optim;
    config.use_ppisp = !config.use_ppisp;
    config.ppisp_param_type = probe_string("ppisp_param_type", &config.ppisp_param_type);
    config.ppisp_exposure_from_exif = !config.ppisp_exposure_from_exif;
    config.ppisp_exposure_arithmetic_mean = !config.ppisp_exposure_arithmetic_mean;
    config.apply_ppisp_before_bilagrid = !config.apply_ppisp_before_bilagrid;
    config.apply_ppisp_before_color_space = !config.apply_ppisp_before_color_space;
    config.use_adagrad_ppisp_optim = !config.use_adagrad_ppisp_optim;
    config.ppisp_reg_exposure_mean = probe_f32(config.ppisp_reg_exposure_mean);
    config.ppisp_reg_color_mean = probe_f32(config.ppisp_reg_color_mean);
    config.ppisp_reg_vig_center = probe_f32(config.ppisp_reg_vig_center);
    config.ppisp_reg_vig_non_pos = probe_f32(config.ppisp_reg_vig_non_pos);
    config.ppisp_reg_vig_channel_var = probe_f32(config.ppisp_reg_vig_channel_var);
    config.ppisp_reg_crf_channel_var = probe_f32(config.ppisp_reg_crf_channel_var);
    config.bilagrid_adagrad_lr = probe_f32(config.bilagrid_adagrad_lr);
    config.bilagrid_adagrad_depth_lr = probe_f32(config.bilagrid_adagrad_depth_lr);
    config.bilagrid_adagrad_normal_lr = probe_f32(config.bilagrid_adagrad_normal_lr);
    config.ppisp_adagrad_lr = probe_f32(config.ppisp_adagrad_lr);
    config.bilagrid_lr = probe_f32(config.bilagrid_lr);
    config.bilagrid_lr_final = Some(probe_f32(config.bilagrid_lr_final.unwrap_or(0.0)));
    config.bilagrid_lr_warmup = probe_i32(config.bilagrid_lr_warmup);
    config.bilagrid_depth_lr = probe_f32(config.bilagrid_depth_lr);
    config.bilagrid_depth_lr_final = Some(probe_f32(config.bilagrid_depth_lr_final.unwrap_or(0.0)));
    config.bilagrid_depth_lr_warmup = probe_i32(config.bilagrid_depth_lr_warmup);
    config.bilagrid_normal_lr = probe_f32(config.bilagrid_normal_lr);
    config.bilagrid_normal_lr_final =
        Some(probe_f32(config.bilagrid_normal_lr_final.unwrap_or(0.0)));
    config.bilagrid_normal_lr_warmup = probe_i32(config.bilagrid_normal_lr_warmup);
    config.ppisp_lr = probe_f32(config.ppisp_lr);
    config.ppisp_lr_final = Some(probe_f32(config.ppisp_lr_final.unwrap_or(0.0)));
    config.ppisp_lr_warmup = probe_i32(config.ppisp_lr_warmup);
    config.image_color_is_linear = Some(!config.image_color_is_linear.unwrap_or(false));
    config.image_color_transfer =
        probe_string("image_color_transfer", &config.image_color_transfer);
    config.image_color_gamut = probe_string("image_color_gamut", &config.image_color_gamut);
    config.splat_color_is_linear = Some(!config.splat_color_is_linear.unwrap_or(false));
    config.splat_color_transfer =
        probe_string("splat_color_transfer", &config.splat_color_transfer);
    config.splat_color_gamut = probe_string("splat_color_gamut", &config.splat_color_gamut);
    config.point_color_is_linear = Some(!config.point_color_is_linear.unwrap_or(false));
    config.point_color_transfer =
        probe_string("point_color_transfer", &config.point_color_transfer);
    config.point_color_gamut = probe_string("point_color_gamut", &config.point_color_gamut);
    config.cache_images = probe_string("cache_images", &config.cache_images);
    config.max_batch_per_epoch = probe_i32(config.max_batch_per_epoch);
    config.min_renders_per_refine = probe_f32(config.min_renders_per_refine);
    config.render_quantile = probe_f32(config.render_quantile);
    config.max_train_batch_size = probe_i32(config.max_train_batch_size);
    config.split_batch = !config.split_batch;
    config.use_fused_proj_bwd_optim = !config.use_fused_proj_bwd_optim;
    config.packed = !config.packed;
    config.quantization_level = probe_i32(config.quantization_level);
    config.bin_tile_size = probe_i32(config.bin_tile_size);
    config.preallocate_splat_tensors = !config.preallocate_splat_tensors;
    config.optimizer_offload = probe_string("optimizer_offload", &config.optimizer_offload);
    config.use_bvh = !config.use_bvh;
    config.means_lr = probe_f32(config.means_lr);
    config.means_lr_final = Some(probe_f32(config.means_lr_final.unwrap_or(0.0)));
    config.scales_lr = probe_f32(config.scales_lr);
    config.scales_lr_final = Some(probe_f32(config.scales_lr_final.unwrap_or(0.0)));
    config.quats_lr = probe_f32(config.quats_lr);
    config.opacities_lr = probe_f32(config.opacities_lr);
    config.features_dc_lr = probe_f32(config.features_dc_lr);
    config.features_sh_lr = probe_f32(config.features_sh_lr);
    config.features_sb_lr = probe_f32(config.features_sb_lr);
    config.background_dc_lr = probe_f32(config.background_dc_lr);
    config.background_sh_lr = probe_f32(config.background_sh_lr);
    config.max_steps = Some(probe_i32(config.max_steps.unwrap_or(0)));
    config.use_scale_agnostic_mean = !config.use_scale_agnostic_mean;
    config.use_per_splat_bias_correction = !config.use_per_splat_bias_correction;
}

#[cfg(test)]
fn probe_string(name: &str, current: &str) -> String {
    let spec = field_spec(name).expect("known field");
    match spec.choice_kind {
        crate::config::ChoiceKind::Closed(choices) => choices
            .iter()
            .copied()
            .find(|choice| *choice != current)
            .expect("closed set has another token")
            .to_owned(),
        crate::config::ChoiceKind::Free
        | crate::config::ChoiceKind::FreeForm
        | crate::config::ChoiceKind::Path => {
            if current == "probe" {
                "probe-2".to_owned()
            } else {
                "probe".to_owned()
            }
        }
    }
}

#[cfg(test)]
fn probe_i32(value: i32) -> i32 {
    if value == i32::MAX {
        value - 1
    } else {
        value + 1
    }
}

#[cfg(test)]
fn probe_f32(value: f32) -> f32 {
    if value.is_finite() && value != 1.5 {
        1.5
    } else {
        2.5
    }
}

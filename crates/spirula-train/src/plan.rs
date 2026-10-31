//! Capability-validated executable training plan (P5-F08).
//!
//! The plan is a business-layer value. It consumes the compatibility config,
//! a dataset summary, and portable GPU capability values; no `wgpu` type is
//! exposed here. Compatibility fields stay in the canonical payload, while
//! only explicitly resolved fields are executable behavior.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use spirula_core::{
    ContextKey, DomainError, ErrorCode, JsonKind, JsonNumber, LoadedTrainConfig, TrainConfig,
    UnknownField, parse_json,
};
use spirula_gpu::{
    AdapterCapabilities, AdapterClass, AdapterIdentity, AdapterLimits, BackendKind, CapabilityId,
    CapabilityRequest, FeatureBits, FieldAvailability, LimitFloor, negotiate,
};

pub const TRAIN_PLAN_SCHEMA_VERSION: u64 = 1;
pub const TRAIN_PLAN_DIGEST_VERSION: &str = "sha256-canonical-json-v1";

fn error(code: ErrorCode, field: &str, message: impl Into<String>) -> DomainError {
    DomainError::try_new(code, message)
        .expect("nonempty train plan error")
        .with_context(ContextKey::Field, field)
        .expect("field context")
}

fn no_gpu(message: impl Into<String>) -> DomainError {
    DomainError::try_new(ErrorCode::NoAdapter, message)
        .expect("nonempty gpu error")
        .with_context(ContextKey::Field, "gpu")
        .expect("gpu field")
}

/// Dataset facts needed by planning. It intentionally contains no parsed IO
/// object or image buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetSummary {
    pub dataset_id: String,
    pub train_images: u64,
    pub eval_images: u64,
    pub validation_images: u64,
    pub point_count: u64,
    pub max_width: u32,
    pub max_height: u32,
}

impl DatasetSummary {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.dataset_id.trim().is_empty() {
            return Err(error(
                ErrorCode::InvalidArgument,
                "dataset_id",
                "dataset id is empty",
            ));
        }
        if self.train_images == 0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "train_images",
                "train split is empty",
            ));
        }
        if self.max_width == 0 || self.max_height == 0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "image_size",
                "dataset dimensions must be positive",
            ));
        }
        Ok(())
    }
}

/// Portable GPU facts copied from the narrow `spirula-gpu` capability surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuCapabilitySnapshot {
    pub adapter_name: String,
    pub backend: BackendKind,
    pub features: FeatureBits,
    pub limits: AdapterLimits,
}

impl GpuCapabilitySnapshot {
    pub fn from_adapter(adapter: &AdapterCapabilities) -> Self {
        Self {
            adapter_name: adapter.identity.name.clone(),
            backend: adapter.identity.backend,
            features: adapter.features,
            limits: adapter.limits,
        }
    }

    pub fn host_test() -> Self {
        Self {
            adapter_name: "host-test".into(),
            backend: BackendKind::Noop,
            features: FeatureBits::default(),
            limits: AdapterLimits {
                max_buffer_size: 1 << 30,
                max_storage_buffers_per_shader_stage: 8,
                max_compute_workgroup_storage_size: 16_384,
            },
        }
    }

    fn as_adapter(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            identity: AdapterIdentity {
                index: 0,
                name: self.adapter_name.clone(),
                backend: self.backend,
                class: AdapterClass::Other,
                vendor: 0,
                device: 0,
                driver: String::new(),
                driver_info: String::new(),
                dedicated_vram_bytes: FieldAvailability::Unavailable {
                    reason: "plan snapshot",
                },
                uuid: FieldAvailability::Unavailable {
                    reason: "plan snapshot",
                },
            },
            features: self.features,
            limits: self.limits,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanBudget {
    pub host_bytes: u64,
    pub device_bytes: u64,
}

impl Default for PlanBudget {
    fn default() -> Self {
        Self {
            host_bytes: 2 << 30,
            device_bytes: 1 << 30,
        }
    }
}

impl PlanBudget {
    fn validate(&self) -> Result<(), DomainError> {
        if self.host_bytes == 0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "host_bytes",
                "host budget must be greater than zero",
            ));
        }
        if self.device_bytes == 0 {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                "device_bytes",
                "device budget must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TrainPlanRequest {
    pub config: TrainConfig,
    pub unknown_fields: Vec<UnknownField>,
    pub dataset: DatasetSummary,
    pub gpu: Option<GpuCapabilitySnapshot>,
    pub budget: PlanBudget,
}

impl TrainPlanRequest {
    pub fn new(
        config: TrainConfig,
        dataset: DatasetSummary,
        gpu: Option<GpuCapabilitySnapshot>,
    ) -> Self {
        Self {
            config,
            unknown_fields: Vec::new(),
            dataset,
            gpu,
            budget: PlanBudget::default(),
        }
    }

    /// Build a request from the compatibility document, retaining unknown
    /// root fields in the plan payload and digest.
    pub fn from_loaded_config(
        loaded: &LoadedTrainConfig,
        dataset: DatasetSummary,
        gpu: Option<GpuCapabilitySnapshot>,
    ) -> Self {
        Self {
            config: loaded.config.clone(),
            unknown_fields: loaded.unknown_fields(),
            dataset,
            gpu,
            budget: PlanBudget::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTrainOptions {
    pub primitive: String,
    pub sh_degree: u32,
    pub iterations: u64,
    pub eval_interval: u64,
    pub train_split_percent: u32,
    pub validation_split_percent: u32,
    pub max_splats: u64,
    pub max_train_batch_size: u64,
    pub loss: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedGpuPlan {
    pub adapter_name: String,
    pub backend: String,
    pub subgroup: bool,
    pub shader_f16: bool,
    pub timestamp_query: bool,
    pub float32_atomic_add: bool,
    pub max_buffer_size: u64,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_compute_workgroup_storage_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrainPlan {
    pub schema_version: u64,
    pub digest_version: String,
    pub config: String,
    pub unknown_fields: Vec<UnknownField>,
    pub dataset: DatasetSummary,
    pub resolved: ResolvedTrainOptions,
    pub gpu: ResolvedGpuPlan,
    pub budget: PlanBudget,
    pub required_device_bytes: u64,
    pub digest: String,
}

/// Resolve compatibility config into an executable, capability-checked plan.
pub fn resolve_train_plan(request: &TrainPlanRequest) -> Result<TrainPlan, DomainError> {
    request.dataset.validate()?;
    // Resolve plan-owned unsupported combinations before the compatibility
    // writer reports a generic closed-choice error for the same field.
    validate_numeric_and_combinations(&request.config)?;
    request.config.validate()?;
    validate_nonfinite_config(&request.config)?;
    request.budget.validate()?;
    validate_unknown_fields(&request.unknown_fields)?;
    let resolved = resolve_options(&request.config)?;
    let required_host = required_host_bytes(&request.dataset)?;
    let required_device = required_device_bytes(&request.dataset, &resolved)?;
    if required_host > request.budget.host_bytes {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "host_bytes",
            format!(
                "estimated host requirement {required_host} exceeds budget {}",
                request.budget.host_bytes
            ),
        ));
    }
    if required_device > request.budget.device_bytes {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "device_bytes",
            format!(
                "estimated device requirement {required_device} exceeds budget {}",
                request.budget.device_bytes
            ),
        ));
    }
    let gpu = request
        .gpu
        .as_ref()
        .ok_or_else(|| no_gpu("no GPU capability snapshot was supplied"))?;
    let required = LimitFloor {
        max_buffer_size: Some(required_device),
        max_storage_buffers_per_shader_stage: Some(4),
        max_compute_workgroup_storage_size: None,
    };
    let capability_request = CapabilityRequest {
        required_features: Vec::<CapabilityId>::new(),
        optional_features: CapabilityId::ALL.to_vec(),
        required_min_limits: required,
        optional_desired_limits: LimitFloor::default(),
        prefer_staged_reduce_fallback: true,
    };
    let negotiated = negotiate(&gpu.as_adapter(), &capability_request)?;
    let resolved_gpu = ResolvedGpuPlan {
        adapter_name: gpu.adapter_name.clone(),
        backend: gpu.backend.as_str().to_owned(),
        subgroup: matches!(
            negotiated.subgroup,
            spirula_gpu::SubgroupPlan::AvailableDeclared
        ),
        shader_f16: matches!(
            negotiated.shader_f16,
            spirula_gpu::F16Plan::AvailableDeclared
        ),
        timestamp_query: matches!(
            negotiated.timestamp,
            spirula_gpu::TimestampPlan::AvailableDeclared
        ),
        float32_atomic_add: matches!(
            negotiated.float_atomic,
            spirula_gpu::FloatAtomicPlan::AvailableDeclared
        ),
        max_buffer_size: negotiated.limits.max_buffer_size.effective,
        max_storage_buffers_per_shader_stage: negotiated
            .limits
            .max_storage_buffers_per_shader_stage
            .effective,
        max_compute_workgroup_storage_size: negotiated
            .limits
            .max_compute_workgroup_storage_size
            .effective,
    };
    let config = request.config.to_current_json()?;
    let mut plan = TrainPlan {
        schema_version: TRAIN_PLAN_SCHEMA_VERSION,
        digest_version: TRAIN_PLAN_DIGEST_VERSION.to_owned(),
        config,
        unknown_fields: request.unknown_fields.clone(),
        dataset: request.dataset.clone(),
        resolved,
        gpu: resolved_gpu,
        budget: request.budget.clone(),
        required_device_bytes: required_device,
        digest: String::new(),
    };
    plan.digest = plan.compute_digest()?;
    Ok(plan)
}

pub fn resolve_train_plan_from_config(
    config: TrainConfig,
    dataset: DatasetSummary,
    gpu: Option<GpuCapabilitySnapshot>,
) -> Result<TrainPlan, DomainError> {
    resolve_train_plan(&TrainPlanRequest::new(config, dataset, gpu))
}

impl TrainPlan {
    pub fn resolve(request: &TrainPlanRequest) -> Result<Self, DomainError> {
        resolve_train_plan(request)
    }

    pub fn compute_digest(&self) -> Result<String, DomainError> {
        validate_digest_version(&self.digest_version)?;
        let bytes = serde_json::to_vec(&self.payload_value()).map_err(|_| {
            error(
                ErrorCode::Internal,
                "digest",
                "canonical plan serialization failed",
            )
        })?;
        Ok(hex_digest(&bytes))
    }

    pub fn verify_digest(&self) -> Result<(), DomainError> {
        let expected = self.compute_digest()?;
        if expected != self.digest {
            return Err(error(
                ErrorCode::IntegrityMismatch,
                "digest",
                "train plan digest does not match canonical payload",
            ));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, DomainError> {
        self.verify_digest()?;
        let mut object = self
            .payload_value()
            .as_object()
            .cloned()
            .expect("plan object");
        object.insert("digest".into(), Value::String(self.digest.clone()));
        serde_json::to_string(&Value::Object(object))
            .map_err(|_| error(ErrorCode::Internal, "plan", "plan serialization failed"))
    }

    pub fn from_json(text: &str) -> Result<Self, DomainError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|_| error(ErrorCode::MalformedRecord, "plan", "plan JSON is malformed"))?;
        let object = value.as_object().ok_or_else(|| {
            error(
                ErrorCode::MalformedRecord,
                "plan",
                "plan JSON must be an object",
            )
        })?;
        let digest = object
            .get("digest")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    ErrorCode::MalformedRecord,
                    "digest",
                    "plan digest is missing",
                )
            })?
            .to_owned();
        let schema_version = object
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                error(
                    ErrorCode::MalformedRecord,
                    "schema_version",
                    "schema version missing",
                )
            })?;
        if schema_version != TRAIN_PLAN_SCHEMA_VERSION {
            return Err(error(
                ErrorCode::UnsupportedVersion,
                "schema_version",
                "unsupported train plan version",
            ));
        }
        // Rebuilding the typed view is intentionally strict; the canonical
        // resolver remains the authority for creating new plans.
        let config = object
            .get("config")
            .and_then(Value::as_str)
            .ok_or_else(|| error(ErrorCode::MalformedRecord, "config", "config missing"))?
            .to_owned();
        let dataset = parse_dataset(object.get("dataset"))?;
        let resolved = parse_resolved(object.get("resolved"))?;
        let gpu = parse_gpu(object.get("gpu"))?;
        let budget = parse_budget(object.get("budget"))?;
        let required_device_bytes = object
            .get("required_device_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                error(
                    ErrorCode::MalformedRecord,
                    "required_device_bytes",
                    "required device bytes missing",
                )
            })?;
        let unknown_fields = parse_unknown(object.get("unknown_fields"))?;
        let digest_version = object
            .get("digest_version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    ErrorCode::MalformedRecord,
                    "digest_version",
                    "digest version missing",
                )
            })?
            .to_owned();
        validate_digest_version(&digest_version)?;
        let plan = Self {
            schema_version,
            digest_version,
            config,
            unknown_fields,
            dataset,
            resolved,
            gpu,
            budget,
            required_device_bytes,
            digest: digest.clone(),
        };
        validate_loaded_plan(&plan)?;
        if plan.compute_digest()? != digest {
            return Err(error(
                ErrorCode::IntegrityMismatch,
                "digest",
                "train plan digest does not match canonical payload",
            ));
        }
        Ok(plan)
    }

    fn payload_value(&self) -> Value {
        let mut unknown_fields = self.unknown_fields.clone();
        unknown_fields
            .sort_by(|left, right| left.name.cmp(&right.name).then(left.raw.cmp(&right.raw)));
        let unknown = unknown_fields
            .iter()
            .map(|field| json!({"name": field.name, "raw": field.raw}))
            .collect::<Vec<_>>();
        json!({
            "schema_version": self.schema_version,
            "digest_version": self.digest_version,
            "config": self.config,
            "unknown_fields": unknown,
            "dataset": {"dataset_id": self.dataset.dataset_id, "train_images": self.dataset.train_images, "eval_images": self.dataset.eval_images, "validation_images": self.dataset.validation_images, "point_count": self.dataset.point_count, "max_width": self.dataset.max_width, "max_height": self.dataset.max_height},
            "resolved": {"primitive": self.resolved.primitive, "sh_degree": self.resolved.sh_degree, "iterations": self.resolved.iterations, "eval_interval": self.resolved.eval_interval, "train_split_percent": self.resolved.train_split_percent, "validation_split_percent": self.resolved.validation_split_percent, "max_splats": self.resolved.max_splats, "max_train_batch_size": self.resolved.max_train_batch_size, "loss": self.resolved.loss},
            "gpu": {"adapter_name": self.gpu.adapter_name, "backend": self.gpu.backend, "subgroup": self.gpu.subgroup, "shader_f16": self.gpu.shader_f16, "timestamp_query": self.gpu.timestamp_query, "float32_atomic_add": self.gpu.float32_atomic_add, "max_buffer_size": self.gpu.max_buffer_size, "max_storage_buffers_per_shader_stage": self.gpu.max_storage_buffers_per_shader_stage, "max_compute_workgroup_storage_size": self.gpu.max_compute_workgroup_storage_size},
            "budget": {"host_bytes": self.budget.host_bytes, "device_bytes": self.budget.device_bytes},
            "required_device_bytes": self.required_device_bytes,
        })
    }
}

fn validate_numeric_and_combinations(config: &TrainConfig) -> Result<(), DomainError> {
    for (field, value) in [
        ("train_split_fraction", config.train_split_fraction),
        ("validation_fraction", config.validation_fraction),
        ("scene_center_threshold", config.scene_center_threshold),
        (
            "progressive_resolution_full_at",
            config.progressive_resolution_full_at,
        ),
    ] {
        if !value.is_finite() {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                field,
                "value must be finite",
            ));
        }
    }
    if !(0.0..=1.0).contains(&config.train_split_fraction) {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "train_split_fraction",
            "must be in [0,1]",
        ));
    }
    if !(0.0..=1.0).contains(&config.validation_fraction) {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "validation_fraction",
            "must be in [0,1]",
        ));
    }
    if config.train_split_fraction + config.validation_fraction > 1.0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "validation_fraction",
            "train and validation fractions must sum to at most one",
        ));
    }
    if config.num_iterations <= 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "num_iterations",
            "must be positive",
        ));
    }
    if config.eval_interval <= 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "eval_interval",
            "must be positive",
        ));
    }
    if !(0..=3).contains(&config.sh_degree) {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "sh_degree",
            "must be in [0,3]",
        ));
    }
    if config.cap_max <= 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "cap_max",
            "splat cap must be positive",
        ));
    }
    if config.primitive != "3dgs" {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "primitive",
            "only 3dgs is executable in this plan",
        ));
    }
    if config.max_train_batch_size == 0 || config.max_train_batch_size < -1 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "max_train_batch_size",
            "must be -1 or positive",
        ));
    }
    if config.max_steps.is_some_and(|steps| steps <= 0) {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "max_steps",
            "must be positive when supplied",
        ));
    }
    if !config.perceptual_loss.is_empty() && config.perceptual_loss != "ssim_blur_tolerant" {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "perceptual_loss",
            "loss is retained for compatibility but not executable",
        ));
    }
    if !config.optimizer_offload.is_empty() && config.optimizer_offload != "cpu" {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "optimizer_offload",
            "optimizer offload mode is not executable",
        ));
    }
    Ok(())
}

fn validate_digest_version(version: &str) -> Result<(), DomainError> {
    if version != TRAIN_PLAN_DIGEST_VERSION {
        return Err(error(
            ErrorCode::UnsupportedVersion,
            "digest_version",
            format!("unsupported digest version `{version}`"),
        ));
    }
    Ok(())
}

fn validate_unknown_fields(fields: &[UnknownField]) -> Result<(), DomainError> {
    let mut names = std::collections::BTreeSet::new();
    for field in fields {
        if field.name.trim().is_empty() {
            return Err(error(
                ErrorCode::MalformedRecord,
                "unknown_fields",
                "unknown field name is empty",
            ));
        }
        if !names.insert(&field.name) {
            return Err(error(
                ErrorCode::MalformedRecord,
                "unknown_fields",
                format!("duplicate unknown field `{}`", field.name),
            ));
        }
        parse_json(&field.raw).map_err(|_| {
            error(
                ErrorCode::MalformedRecord,
                "unknown_fields",
                format!("unknown field `{}` has invalid JSON", field.name),
            )
        })?;
    }
    Ok(())
}

fn validate_nonfinite_config(config: &TrainConfig) -> Result<(), DomainError> {
    let text = config.to_current_json()?;
    let value = parse_json(&text).map_err(|_| {
        error(
            ErrorCode::MalformedRecord,
            "config",
            "canonical config could not be parsed",
        )
    })?;
    let JsonKind::Object(fields) = value.kind else {
        return Err(error(
            ErrorCode::MalformedRecord,
            "config",
            "canonical config is not an object",
        ));
    };
    for (name, value) in fields {
        if let Some(path) = first_disallowed_nonfinite(&value, &name) {
            return Err(error(
                ErrorCode::ValueOutOfRange,
                &path,
                "non-finite value is not executable",
            ));
        }
    }
    Ok(())
}

fn first_disallowed_nonfinite(value: &spirula_core::JsonValue, path: &str) -> Option<String> {
    match &value.kind {
        JsonKind::Number(number) if !matches!(number, JsonNumber::Finite(_)) => {
            let allowed = matches!(
                (path, number),
                ("outlier_threshold", JsonNumber::Infinity)
                    | ("max_world_size", JsonNumber::Infinity)
            );
            (!allowed).then(|| path.to_owned())
        }
        JsonKind::Array(values) => values.iter().enumerate().find_map(|(index, child)| {
            first_disallowed_nonfinite(child, &format!("{path}[{index}]"))
        }),
        JsonKind::Object(fields) => fields
            .iter()
            .find_map(|(name, child)| first_disallowed_nonfinite(child, &format!("{path}.{name}"))),
        _ => None,
    }
}

fn validate_loaded_plan(plan: &TrainPlan) -> Result<(), DomainError> {
    validate_digest_version(&plan.digest_version)?;
    plan.dataset.validate()?;
    plan.budget.validate()?;
    validate_unknown_fields(&plan.unknown_fields)?;
    if plan.gpu.adapter_name.trim().is_empty() || plan.gpu.backend.trim().is_empty() {
        return Err(error(
            ErrorCode::MalformedRecord,
            "gpu",
            "resolved GPU identity is incomplete",
        ));
    }
    if plan.gpu.max_buffer_size == 0 || plan.gpu.max_storage_buffers_per_shader_stage < 4 {
        return Err(no_gpu("resolved GPU limits are insufficient"));
    }
    validate_resolved_options(&plan.resolved)?;
    let required_host = required_host_bytes(&plan.dataset)?;
    if required_host > plan.budget.host_bytes {
        return Err(error(
            ErrorCode::IntegrityMismatch,
            "host_bytes",
            "resolved host budget does not match the plan",
        ));
    }
    let required_device = required_device_bytes(&plan.dataset, &plan.resolved)?;
    if required_device != plan.required_device_bytes
        || required_device > plan.budget.device_bytes
        || required_device > plan.gpu.max_buffer_size
    {
        return Err(error(
            ErrorCode::IntegrityMismatch,
            "required_device_bytes",
            "resolved device budget does not match the plan",
        ));
    }
    let loaded = LoadedTrainConfig::parse(&plan.config)?;
    loaded.config.validate()?;
    validate_nonfinite_config(&loaded.config)?;
    validate_numeric_and_combinations(&loaded.config)?;
    let expected_resolved = resolve_options(&loaded.config)?;
    if expected_resolved != plan.resolved {
        return Err(error(
            ErrorCode::IntegrityMismatch,
            "resolved",
            "resolved options do not match the compatibility config",
        ));
    }
    Ok(())
}

fn validate_resolved_options(options: &ResolvedTrainOptions) -> Result<(), DomainError> {
    if options.primitive != "3dgs" {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "primitive",
            "only 3dgs is executable in this plan",
        ));
    }
    if options.sh_degree > 3 || options.iterations == 0 || options.eval_interval == 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "resolved",
            "resolved numeric option is out of range",
        ));
    }
    if options.train_split_percent + options.validation_split_percent > 100
        || options.max_splats == 0
        || options.max_train_batch_size == 0
    {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "resolved",
            "resolved split or resource option is out of range",
        ));
    }
    if options.loss != "none" && options.loss != "ssim_blur_tolerant" {
        return Err(error(
            ErrorCode::UnimplementedOption,
            "loss",
            "resolved loss is not executable",
        ));
    }
    Ok(())
}

fn resolve_options(config: &TrainConfig) -> Result<ResolvedTrainOptions, DomainError> {
    let iterations = config.max_steps.unwrap_or(config.num_iterations);
    if iterations <= 0 {
        return Err(error(
            ErrorCode::ValueOutOfRange,
            "max_steps",
            "effective iterations must be positive",
        ));
    }
    Ok(ResolvedTrainOptions {
        primitive: config.primitive.clone(),
        sh_degree: config.sh_degree as u32,
        iterations: iterations as u64,
        eval_interval: config.eval_interval as u64,
        train_split_percent: (config.train_split_fraction * 100.0).round() as u32,
        validation_split_percent: (config.validation_fraction * 100.0).round() as u32,
        max_splats: config.cap_max as u64,
        max_train_batch_size: if config.max_train_batch_size < 0 {
            1
        } else {
            config.max_train_batch_size as u64
        },
        loss: if config.perceptual_loss.is_empty() {
            "none".into()
        } else {
            config.perceptual_loss.clone()
        },
    })
}

fn required_host_bytes(dataset: &DatasetSummary) -> Result<u64, DomainError> {
    let pixels = u64::from(dataset.max_width)
        .checked_mul(u64::from(dataset.max_height))
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "image_size",
                "pixel count overflow",
            )
        })?;
    dataset
        .train_images
        .checked_mul(pixels)
        .and_then(|n| n.checked_mul(3))
        .and_then(|n| n.checked_add(dataset.point_count.checked_mul(32)?))
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "host_bytes",
                "host byte estimate overflow",
            )
        })
}

fn required_device_bytes(
    dataset: &DatasetSummary,
    resolved: &ResolvedTrainOptions,
) -> Result<u64, DomainError> {
    let splat = resolved.max_splats.checked_mul(64).ok_or_else(|| {
        error(
            ErrorCode::ValueOutOfRange,
            "max_splats",
            "device byte estimate overflow",
        )
    })?;
    let pixels = u64::from(dataset.max_width)
        .checked_mul(u64::from(dataset.max_height))
        .and_then(|p| dataset.train_images.checked_mul(p))
        .and_then(|p| p.checked_mul(16))
        .ok_or_else(|| {
            error(
                ErrorCode::ValueOutOfRange,
                "device_bytes",
                "device byte estimate overflow",
            )
        })?;
    splat.checked_add(pixels).ok_or_else(|| {
        error(
            ErrorCode::ValueOutOfRange,
            "device_bytes",
            "device byte estimate overflow",
        )
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parse_dataset(value: Option<&Value>) -> Result<DatasetSummary, DomainError> {
    let object = value.and_then(Value::as_object).ok_or_else(|| {
        error(
            ErrorCode::MalformedRecord,
            "dataset",
            "dataset section missing",
        )
    })?;
    Ok(DatasetSummary {
        dataset_id: required_str(object, "dataset_id")?,
        train_images: required_u64(object, "train_images")?,
        eval_images: required_u64(object, "eval_images")?,
        validation_images: required_u64(object, "validation_images")?,
        point_count: required_u64(object, "point_count")?,
        max_width: u32::try_from(required_u64(object, "max_width")?)
            .map_err(|_| error(ErrorCode::ValueOutOfRange, "max_width", "width exceeds u32"))?,
        max_height: u32::try_from(required_u64(object, "max_height")?).map_err(|_| {
            error(
                ErrorCode::ValueOutOfRange,
                "max_height",
                "height exceeds u32",
            )
        })?,
    })
}
fn parse_resolved(value: Option<&Value>) -> Result<ResolvedTrainOptions, DomainError> {
    let o = value.and_then(Value::as_object).ok_or_else(|| {
        error(
            ErrorCode::MalformedRecord,
            "resolved",
            "resolved section missing",
        )
    })?;
    Ok(ResolvedTrainOptions {
        primitive: required_str(o, "primitive")?,
        sh_degree: u32::try_from(required_u64(o, "sh_degree")?)
            .map_err(|_| error(ErrorCode::ValueOutOfRange, "sh_degree", "invalid sh degree"))?,
        iterations: required_u64(o, "iterations")?,
        eval_interval: required_u64(o, "eval_interval")?,
        train_split_percent: u32::try_from(required_u64(o, "train_split_percent")?).map_err(
            |_| {
                error(
                    ErrorCode::ValueOutOfRange,
                    "train_split_percent",
                    "invalid split",
                )
            },
        )?,
        validation_split_percent: u32::try_from(required_u64(o, "validation_split_percent")?)
            .map_err(|_| {
                error(
                    ErrorCode::ValueOutOfRange,
                    "validation_split_percent",
                    "invalid split",
                )
            })?,
        max_splats: required_u64(o, "max_splats")?,
        max_train_batch_size: required_u64(o, "max_train_batch_size")?,
        loss: required_str(o, "loss")?,
    })
}
fn parse_gpu(value: Option<&Value>) -> Result<ResolvedGpuPlan, DomainError> {
    let o = value
        .and_then(Value::as_object)
        .ok_or_else(|| error(ErrorCode::MalformedRecord, "gpu", "gpu section missing"))?;
    Ok(ResolvedGpuPlan {
        adapter_name: required_str(o, "adapter_name")?,
        backend: required_str(o, "backend")?,
        subgroup: required_bool(o, "subgroup")?,
        shader_f16: required_bool(o, "shader_f16")?,
        timestamp_query: required_bool(o, "timestamp_query")?,
        float32_atomic_add: required_bool(o, "float32_atomic_add")?,
        max_buffer_size: required_u64(o, "max_buffer_size")?,
        max_storage_buffers_per_shader_stage: u32::try_from(required_u64(
            o,
            "max_storage_buffers_per_shader_stage",
        )?)
        .map_err(|_| {
            error(
                ErrorCode::ValueOutOfRange,
                "max_storage_buffers_per_shader_stage",
                "invalid limit",
            )
        })?,
        max_compute_workgroup_storage_size: u32::try_from(required_u64(
            o,
            "max_compute_workgroup_storage_size",
        )?)
        .map_err(|_| {
            error(
                ErrorCode::ValueOutOfRange,
                "max_compute_workgroup_storage_size",
                "invalid limit",
            )
        })?,
    })
}
fn parse_budget(value: Option<&Value>) -> Result<PlanBudget, DomainError> {
    let o = value.and_then(Value::as_object).ok_or_else(|| {
        error(
            ErrorCode::MalformedRecord,
            "budget",
            "budget section missing",
        )
    })?;
    Ok(PlanBudget {
        host_bytes: required_u64(o, "host_bytes")?,
        device_bytes: required_u64(o, "device_bytes")?,
    })
}
fn parse_unknown(value: Option<&Value>) -> Result<Vec<UnknownField>, DomainError> {
    let a = value.and_then(Value::as_array).ok_or_else(|| {
        error(
            ErrorCode::MalformedRecord,
            "unknown_fields",
            "unknown fields missing",
        )
    })?;
    a.iter()
        .map(|v| {
            let o = v.as_object().ok_or_else(|| {
                error(
                    ErrorCode::MalformedRecord,
                    "unknown_fields",
                    "unknown field must be object",
                )
            })?;
            Ok(UnknownField {
                name: required_str(o, "name")?,
                raw: required_str(o, "raw")?,
            })
        })
        .collect()
}
fn required_str(o: &Map<String, Value>, key: &str) -> Result<String, DomainError> {
    o.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(ErrorCode::MalformedRecord, key, "string field missing"))
}
fn required_u64(o: &Map<String, Value>, key: &str) -> Result<u64, DomainError> {
    o.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| error(ErrorCode::MalformedRecord, key, "integer field missing"))
}
fn required_bool(o: &Map<String, Value>, key: &str) -> Result<bool, DomainError> {
    o.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| error(ErrorCode::MalformedRecord, key, "boolean field missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dataset() -> DatasetSummary {
        DatasetSummary {
            dataset_id: "tiny-colmap".into(),
            train_images: 1,
            eval_images: 0,
            validation_images: 0,
            point_count: 4,
            max_width: 64,
            max_height: 48,
        }
    }
    #[test]
    fn default_resolves_and_roundtrips_digest() {
        let plan = resolve_train_plan_from_config(
            TrainConfig::default(),
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap();
        assert_eq!(plan.schema_version, 1);
        plan.verify_digest().unwrap();
        let text = plan.to_json().unwrap();
        let restored = TrainPlan::from_json(&text).unwrap();
        assert_eq!(restored.digest, plan.digest);
        assert_eq!(restored.to_json().unwrap(), text);
    }
    #[test]
    fn nonfinite_and_range_are_rejected() {
        let config = TrainConfig {
            train_split_fraction: f32::NAN,
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
        let config = TrainConfig {
            sh_degree: 4,
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

        let config = TrainConfig {
            means_lr: f32::NAN,
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

        let config = TrainConfig {
            cap_max: -1,
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);

        let config = TrainConfig {
            train_split_fraction: 0.8,
            validation_fraction: 0.3,
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ValueOutOfRange);
    }
    #[test]
    fn unsupported_combo_and_gpu_gap_are_explicit() {
        let config = TrainConfig {
            primitive: "2dgs".into(),
            ..TrainConfig::default()
        };
        let err = resolve_train_plan_from_config(
            config,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnimplementedOption);
        let err =
            resolve_train_plan_from_config(TrainConfig::default(), dataset(), None).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NoAdapter);
    }
    #[test]
    fn unknown_fields_change_digest_but_not_resolved_behavior() {
        let mut req = TrainPlanRequest::new(
            TrainConfig::default(),
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        );
        let a = resolve_train_plan(&req).unwrap();
        req.unknown_fields.push(UnknownField {
            name: "future_option".into(),
            raw: "true".into(),
        });
        let b = resolve_train_plan(&req).unwrap();
        assert_ne!(a.digest, b.digest);
        assert_eq!(a.resolved, b.resolved);
    }

    #[test]
    fn loaded_unknown_fields_are_preserved_and_canonicalized() {
        let loaded = LoadedTrainConfig::parse(
            r#"{"schema_version":1,"primitive":"3dgs","future_option":{"enabled":true}}"#,
        )
        .unwrap();
        let request = TrainPlanRequest::from_loaded_config(
            &loaded,
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        );
        let plan = resolve_train_plan(&request).unwrap();
        assert_eq!(plan.unknown_fields.len(), 1);
        let text = plan.to_json().unwrap();
        assert!(text.contains("future_option"));
        assert_eq!(TrainPlan::from_json(&text).unwrap().digest, plan.digest);

        let mut reordered = request;
        reordered.unknown_fields = vec![
            UnknownField {
                name: "zeta".into(),
                raw: "1".into(),
            },
            UnknownField {
                name: "alpha".into(),
                raw: "2".into(),
            },
        ];
        let mut sorted = reordered.clone();
        sorted.unknown_fields.reverse();
        assert_eq!(
            resolve_train_plan(&reordered).unwrap().digest,
            resolve_train_plan(&sorted).unwrap().digest
        );
    }

    #[test]
    fn saved_plan_rejects_digest_version_and_semantic_tampering() {
        let plan = resolve_train_plan_from_config(
            TrainConfig::default(),
            dataset(),
            Some(GpuCapabilitySnapshot::host_test()),
        )
        .unwrap();
        let mut value: Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
        value["digest_version"] = Value::String("sha256-other-v1".into());
        value["digest"] = Value::String(hex_digest(
            &serde_json::to_vec(&Value::Object(
                value
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(key, _)| key.as_str() != "digest")
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ))
            .unwrap(),
        ));
        let err = TrainPlan::from_json(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedVersion);

        let mut value: Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
        value["required_device_bytes"] = Value::from(1_u64);
        let err = TrainPlan::from_json(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::IntegrityMismatch);

        let mut value: Value = serde_json::from_str(&plan.to_json().unwrap()).unwrap();
        value["resolved"]["iterations"] = Value::from(1_u64);
        let payload = Value::Object(
            value
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.as_str() != "digest")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        value["digest"] = Value::String(hex_digest(&serde_json::to_vec(&payload).unwrap()));
        let err = TrainPlan::from_json(&serde_json::to_string(&value).unwrap()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::IntegrityMismatch);
    }
}

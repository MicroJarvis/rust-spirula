# P5-F08：可执行 TrainPlan 与能力校验

P5-F08 的 owner 是 `spirula-train`。它建立了一个 host 侧业务计划边界：
`TrainPlanRequest` 接收 `TrainConfig`、保留的未知配置字段、`DatasetSummary`、
`GpuCapabilitySnapshot` 和 `PlanBudget`，`resolve_train_plan` 返回
`TrainPlan`。计划不持有数据集对象、文件句柄或 `wgpu` 类型。

## 解析契约

`TrainPlanRequest::from_loaded_config` 从 `LoadedTrainConfig` 复制 251 字段
配置和未知 root 字段。计划会展开当前配置的规范化 JSON，并把未知字段按名称
排序后纳入 digest；未知字段不会自动变成已实现的训练行为。

当前可执行字段明确限制为 `3dgs`、SH degree 0–3、正的迭代次数和评估间隔、
合法 train/validation split、正的 splat 上限和 batch size，以及
`none`/`ssim_blur_tolerant` loss。`max_steps` 覆盖默认迭代次数，`-1` batch
表示由计划选用一个有界默认值。其他兼容字段仍保留在 config payload，待对应
训练功能实现后再接入。

计划在解析前检查可执行数字的有限性、范围和组合关系；配置中的非有限值会被
拒绝，兼容格式定义的 `outlier_threshold` 与 `max_world_size` 正无穷哨兵保留。
资源估算使用 checked arithmetic，host/device 预算不足返回 `SP-INP-003`。
GPU 能力通过 `spirula-gpu::negotiate` 的窄类型检查，缺少 required limit 返回
`SP-GPU-001`；不支持的训练选项返回 `SP-INP-004`。

## 持久化与恢复

`TrainPlan::to_json` 输出 schema version 1、digest version
`sha256-canonical-json-v1`、规范化 payload 和 SHA-256 digest。
`TrainPlan::from_json` 验证 schema/digest 版本、payload digest、未知字段 JSON、
预算、resolved 选项、资源估算和配置可读性；修改 payload 或版本会稳定失败。
重新读取后再次 `to_json` 会得到相同的 digest 和规范化文本。

## 证据

`crates/spirula-train/src/plan.rs` 的 headless 测试覆盖默认解析与 round-trip、
未知字段保留和顺序规范化、非有限/范围/组合拒绝、不支持选项、GPU 缺失、预算
和保存计划完整性校验。完整 workspace 门禁：
`cargo run -p spirula-xtask -- verify`（8/8）。训练 worker、渲染、checkpoint、
application/session 和 CLI 接入属于后续功能。

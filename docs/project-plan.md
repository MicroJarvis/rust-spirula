# Spirula Studio Rust 重构项目计划

## 1. 项目目标

将 `/Users/tfjiang/Projects/spirula-studio` 逐步重构为 Rust 项目：

- 桌面 UI 使用 Slint；
- GPU 资源、管线和计算使用 `wgpu`；
- GPU shader 使用 WGSL；
- CLI 与桌面 UI 共用同一套应用会话和业务服务；
- 保留原项目的数据格式、相机约定、checkpoint 和导出兼容性；
- 每个功能按独立垂直切片实现，完成完整测试后再提交 Git。

原 C++ 项目是行为参考。重构期间先建立架构和兼容性基线，再逐项接入功能，避免一次性重写导致行为无法核对。

## 2. 目标架构

```text
Slint UI ───────┐
CLI ────────────┼──> application/session
                │        ├──> train ──> render ──> gpu
                │        ├──> io ───────> core
                │        └──> core
```

工作区 crate 的职责如下：

| crate | 职责 |
|---|---|
| `spirula-core` | 领域类型、相机、变换、场景、splat、配置、错误、验证和纯算法 |
| `spirula-io` | 数据集、图像、点云、checkpoint、缓存、导入导出和原子持久化 |
| `spirula-gpu` | `wgpu` instance、adapter、device、queue、资源、shader、pipeline 和 readback |
| `spirula-render` | 渲染场景、投影、tile/bin、排序、光栅化、合成、深度和预览 |
| `spirula-train` | 训练会话、loss、优化器、densification、评估、停止、保存和恢复 |
| `spirula-ui` | Slint 组件、视图状态、命令绑定、验证反馈和事件订阅 |
| `spirula-cli` | 命令行参数、进程入口和共享应用服务的 CLI 展示层 |
| `spirula-application`（后续加入） | 统一会话状态机、命令、取消、进度和 UI/CLI 编排 |

核心层不得依赖 UI、GPU 或操作系统；GPU 细节停留在 `spirula-gpu` 边界；UI 不直接解析数据集、创建 GPU pipeline 或实现训练规则。

详细依赖边界见 [`docs/architecture.md`](architecture.md)。

## 3. 实施阶段

### Phase 0：基线与架构

1. 固定 C++ 参考仓库 commit、构建方式和测试入口。
2. 盘点数据集、相机模型、checkpoint、CLI 命令、UI 流程和 GPU 后端。
3. 建立能力矩阵，记录每项能力的源码位置、输入格式、输出和验证方式。
4. 准备相机投影、数据集解析、场景变换、splat、checkpoint 和微型渲染场景 fixture。
5. 固化错误模型、事件模型、会话状态机、GPU 边界、shader 约定和测试策略。

验收：Rust 工作区可构建，架构文档和能力矩阵完成，每项能力都有明确参考来源或 fixture。

### Phase 1：Rust 基础层

实现共享错误、ID、日志、配置、取消、序列化版本和原子文件操作；补齐 `spirula-core` 的数学、相机和场景基础类型；建立确定性的 fixture 布局及本地检查命令。

验收：干净 checkout 通过格式化、检查、测试和 clippy，CLI 中没有隐藏业务实现。

### Phase 2：wgpu 基础层

实现 adapter 选择、device/queue 创建、能力协商、资源标签、error scope、device lost、buffer/texture、staging/readback、uniform layout、shader 加载和 pipeline key。添加无窗口 compute smoke test 与最小 surface smoke test。

验收：开发机可以运行 headless GPU 测试；不支持的 adapter 返回可操作的错误。

### Phase 3：数据与场景兼容

按顺序迁移相机投影、坐标系、畸变、全景、EXIF 方向、COLMAP、Nerfstudio、Metashape、图像、mask、点云、场景变换、区域、splat 和 checkpoint。

验收：参考 fixture 可加载和 round-trip；非法输入错误稳定；旧 checkpoint 元数据可识别。

### Phase 4：渲染 MVP

实现渲染场景和相机接口，逐项迁移上传、投影、tile/bin、排序、光栅化、合成、深度和 readback。保留小型 CPU reference renderer，用于 GPU 正确性对照。

验收：fixture 能通过 headless API 和应用会话生成稳定预览，并通过图像容差测试。

### Phase 5：训练核心

实现训练会话、step 调度、loss、优化器、densification、剪枝、评估、进度事件、停止、取消、保存、崩溃恢复和 resume。训练必须复用渲染接口。

验收：微型场景能达到确定 step，可从 checkpoint 继续，指标和输出差异在约定范围内。

### Phase 6：CLI 与 Slint 应用壳

将 CLI 占位入口替换为命令解析；加入 Slint 窗口、导航、日志、校验、进度、取消、数据集预览、训练 start/stop/resume、渲染和导出流程。

验收：CLI 与桌面 UI 使用同一会话服务；用户可以加载 fixture、预览、运行微型训练、停止保存并恢复。

### Phase 7：编辑、网格与导出

迁移选择、mask、区域、场景变换、稀疏编辑、网格生成、网格导入导出、图像/视频导出和 web viewer 资源。文档格式版本化并采用原子保存。

验收：编辑后的场景可重新打开且状态完整，导出可被现有 viewer/reference 工具消费。

### Phase 8：辅助管线

分别迁移 SfM、稠密重建、深度/法线、分割、特征匹配、视频解码和模型管理。模型、许可证、codec 和平台能力必须显式检查。

验收：每条管线有隔离集成测试、失败信息和可用性矩阵。

### Phase 9：跨平台、性能与发布

在 macOS、Linux、Windows 的可用硬件上运行 adapter 测试；比较渲染质量、训练收敛、内存、启动时间和 checkpoint 大小；正确性通过后再加入 profiling、shader/pipeline 缓存、打包、崩溃报告和发布文档。

验收：发布清单可从干净 checkout 重现，能力开关明确，已知差异有记录。

## 4. 首批功能顺序

1. core 错误、ID、数学、相机契约和 fixture harness；
2. `wgpu` device/context headless smoke test；
3. 数据集解析和相机兼容；
4. splat 场景上传和预览渲染；
5. checkpoint 读写；
6. 微型训练循环和 resume；
7. CLI 命令；
8. Slint 壳和共享会话事件；
9. 编辑、导出和网格；
10. SfM、稠密重建、AI mask、深度/法线和视频。

## 5. 单个功能的完成标准

每个功能在提交前必须满足适用项：

- 明确归属 crate、公共 API 和依赖方向；
- 记录 C++ 行为、格式兼容假设和迁移说明；
- 覆盖正常输入、非法输入、取消和持久化恢复；
- GPU 功能有 CPU reference 或确定性 fixture，并记录数值容差；
- headless 测试不依赖可见窗口；
- Slint 行为与应用服务分开测试；
- 通过 `cargo fmt --all -- --check`；
- 通过 `cargo check --workspace`；
- 通过 `cargo test --workspace`；
- 通过 `cargo clippy --workspace --all-targets --all-features -- -D warnings`；
- 文档、fixture 和迁移记录同步更新；
- Git commit 只包含一个连贯功能，且可从干净 checkout 重现。

如果硬件、模型或 fixture 缺失，必须记录具体命令、环境和原因，不能把未测试功能标记为完成。

## 6. Git 提交策略

每个垂直切片都执行：

1. 盘点参考实现和兼容性；
2. 编写 Rust 契约及失败行为；
3. 实现功能和测试；
4. 运行局部测试及完整 workspace 检查；
5. 检查 `git diff` 和生成文件；
6. 使用一个结果明确的 Conventional Commit。

示例：

```text
feat(core): add camera projection contracts
feat(gpu): add headless wgpu device smoke test
feat(data): parse colmap sparse datasets
feat(render): render fixture gaussian scene
feat(train): save and resume tiny training run
feat(ui): add dataset preview screen
```

当前仓库的下一步是完成 Phase 0：记录参考仓库基线、能力矩阵和第一批 compatibility fixture，然后再开始 Phase 1 的代码实现。

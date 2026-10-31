# Spirula Studio Rust 重构项目计划

## 1. 项目目标

将 C++ 参考仓库（`SPIRULA_REFERENCE_REPO`，默认 `../spirula-studio`）逐步重构为 Rust 项目：

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
| `spirula-application`（P1-F07 加入） | 统一会话状态机、命令、取消、进度和 UI/CLI 编排 |

核心层不得依赖 UI、GPU 或操作系统；GPU 细节停留在 `spirula-gpu` 边界；UI 不直接解析数据集、创建 GPU pipeline 或实现训练规则。

详细依赖边界见 [`docs/architecture.md`](architecture.md)。
实施细节、资源预算和恢复门禁按需读取 [development-gates.md](development-gates.md)；
外部技术来源与采纳范围见 [OOOSplat / Brush 审查](research/ooosplat-brush-review.md)。

## 3. 功能点编号与状态

每个可独立开发、测试和合并的交付单元都必须有全局唯一编号，格式为
`P<phase>-F<feature>`，例如 `P0-F01`。编号一经分配不得复用；代码、测试、
fixture、迁移记录和提交说明都使用同一编号。状态只允许使用 `当前`、`未开始`、
`进行中`、`已完成`、`阻塞`，并在每次开发完成后同步更新。

每个功能点都要明确：归属 crate、前置功能、输入输出或公共 API、正常和
失败行为、测试/fixture、文档变更以及完成门禁。“细分范围”列是实现清单，
每个分号分隔项都必须在代码、测试或记录中留下证据。下表是当前基线；`当前` 表示
下一项工作，完成后要把下一项改为 `当前`。Phase 为归属分组，执行遵循前置
依赖和第 4 节；允许提前执行后续 Phase 的验证切片，既有编号不得重排或复用。

### Phase 0：基线与架构

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P0-F01 | 固定 C++ 参考基线 | 文档/工具 | — | 记录仓库地址和 commit；固定构建工具链与依赖版本；保存构建、测试和失败日志入口 | 记录 commit、构建方式、测试入口、依赖版本和可复现命令 | 已完成 |
| P0-F02 | 建立子系统行为清单 | 文档 | P0-F01 | 列出 C++ 主入口和调用链；标注数据读写者与格式；记录正常输出、错误和取消路径 | 记录数据集、相机、checkpoint、CLI、UI、GPU 入口、读写者和可观察输出 | 已完成 |
| P0-F03 | 建立能力矩阵 | 文档 | P0-F02 | 按数据、渲染、训练、UI、CLI 分类；为每项标注源码位置和 owner；定义验证方式与当前状态 | 每项能力有参考源码、输入格式、输出、兼容假设、验证方式和负责人 | 已完成 |
| P0-F04 | 建立最小兼容 fixtures | `spirula-core`/`spirula-io` | P0-F02, P0-F03 | 制作单相机/小图像样本；制作小 splat/点云/checkpoint；保存 golden 元数据、图像和校验值 | 提供相机、数据集、场景变换、splat、checkpoint、微型渲染样本及 golden 结果 | 已完成 |
| P0-F05 | 固化运行时契约 | `spirula-core`/架构 | P0-F02, P0-F03 | 定义错误类别和稳定字段；定义命令、事件、状态转换和取消；记录 GPU 资源与 shader 边界 | 定义错误、事件、session 状态、取消、GPU 边界、shader 约定和版本策略 | 已完成 |
| P0-F06 | 建立验证入口 | workspace | P0-F01–P0-F05 | 提供一条命令运行 fixture 检查；提供文档/状态一致性检查；记录 Phase 0 退出报告 | 提供 fixture 检查、文档状态检查和 Phase 0 验收命令；干净 checkout 可复现 | 已完成 |

Phase 0 退出条件：P0-F01 至 P0-F06 均有记录，工作区可构建，架构/能力矩阵
和第一批 fixtures 可被其他功能点直接引用。

### Phase 1：Rust 基础层

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P1-F01 | 领域错误与稳定 ID | `spirula-core` | P0-F05 | 定义领域错误枚举；分配稳定错误码；保留来源、字段和用户可读信息 | 定义可匹配错误类别、稳定 ID 和显示信息；覆盖非法输入测试 | 已完成 |
| P1-F02 | 配置与版本化序列化 | `spirula-core` | P0-F05, P1-F01 | 定义配置字段和默认值；加入 schema/version；测试缺失、未知和非法字段 | 配置有默认值、校验、版本字段和兼容读取失败语义 | 已完成 |
| P1-F03 | 取消与进度原语 | `spirula-core` | P0-F05, P1-F01 | 定义取消 token 和检查点；定义进度值与事件载荷；测试重复取消、终止和错误终态 | 定义合作式取消、进度值和事件载荷；测试重复取消和终态 | 已完成 |
| P1-F04 | 数学、相机与场景基础类型 | `spirula-core` | P0-F04, P1-F01 | 实现向量、矩阵和四元数；实现相机投影与坐标变换；定义场景和 splat 不变量 | 实现变换、投影、坐标约定和场景不变量；与 P0 fixtures 对照 | 已完成 |
| P1-F05 | 原子文件与 fixture harness | `spirula-io` | P0-F04, P1-F01 | 实现临时文件命名与原子替换；处理损坏和中断写入；提供可复用临时目录 fixture harness | 临时文件加原子替换；损坏、并发失败和恢复路径有测试 | 已完成 |
| P1-F06 | workspace 基础检查 | workspace | P0-F06 | 固定 fmt/check/test/clippy 命令；固定 lint 与 feature 组合；检查 CLI 仅作入口适配 | 固化 lint、格式化、测试和本地检查命令；CLI 不含隐藏业务逻辑 | 已完成 |
| P1-F07 | 最小 headless application/session | `spirula-application`（新增） | P1-F03, P1-F06 | 唯一应用 owner；有界命令和可恢复控制事件历史；快照/游标、取消控制通道和 fake worker；区分 run identity 与迟到事件 | 无训练/GPU 依赖的状态机测试；满队列与慢订阅不阻塞 worker；每个接受命令恰有一个终态 | 已完成 |
| P1-F08 | 资源与性能记录基础设施 | workspace | P1-F06 | 定义版本化测量结果、fixture/配置 hash 和环境字段；记录分配字节、阶段耗时、采样来源和预算；后续 GPU 指标声明不可用原因 | CPU 微型样本可生成和回读结果；已核算字节与系统峰值分开；形成后续逐功能基线模板 | 已完成 |
| P1-F09 | 最小 CLI 会话入口 | `spirula-cli`/application | P1-F07 | help/能力与已接入命令；共享 command/event 映射；SIGINT 与退出码；fake worker 验证，真实预览/训练分别由 P4-F06/P5-F07 接入 | 子进程测试命令拒绝、取消、终态和错误码；未接入命令显式不可用；入口无业务编排 | 已完成 |

Phase 1 退出条件：P1-F01 至 P1-F09 通过 workspace 检查，具备无 GPU 的
共享会话/CLI 骨架和测量入口；真实渲染、训练和完整 UI 仍由后续功能接入。
该退出条件已满足（host-only `spirula-xtask verify`；无真实训练/预览/GPU/UI
能力声明）。记录见 [`docs/phase-1/workspace-checks.md`](phase-1/workspace-checks.md)、
[`docs/phase-1/cli.md`](phase-1/cli.md) 与
[`docs/phase-1/exit-report.md`](phase-1/exit-report.md)。

### Phase 2：wgpu 基础层

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P2-F01 | adapter 选择与能力协商 | `spirula-gpu` | P1-F06 | 枚举 adapter/backend；区分 required/optional feature/limit；声明 subgroup、f16、timestamp 与梯度累加能力和回退 | 记录真实设备/driver/limits；缺少必需能力有稳定错误；可选能力回退有测试 | 已完成 |
| P2-F02 | device、queue 与上下文生命周期 | `spirula-gpu` | P2-F01, P1-F07 | 创建 instance/adapter/device/queue；资源所有权、context identity/generation；由应用 owner 决定 device lost 后重建 | 创建/关闭/失败可观察；旧 generation handle 拒绝复用；应用状态只由同一 owner 更新 | 已完成 |
| P2-F03 | buffer、texture 与 readback | `spirula-gpu` | P2-F02, P1-F08 | 窄类型封装与资源标签；staging/同步/字节预算；真实 upload/readback，compute 在 P2-F04 接入；跨 context 和 in-flight drop | round-trip 正确；超预算拒绝；释放和峰值字节可核算；不预建通用 wgpu 镜像 API | 已完成 |
| P2-F04 | shader 与 pipeline 契约 | `spirula-gpu` | P2-F03 | WGSL 版本和 bind/uniform layout；pipeline key/错误；以真实 compute 验证窄资源封装与 shader 接口 | layout/对齐及缓存失效可测；编译失败带位置；与 P2-F03 共同证明 GPU 边界可执行 | 已完成 |
| P2-F05 | GPU 错误与诊断事件 | `spirula-gpu` | P2-F02–P2-F04 | 包裹 error scope 和提交错误；把 device lost 转成 typed error；发布带资源标签的诊断事件 | error scope、设备丢失和提交失败转为 typed error/event | 已完成 |
| P2-F06 | headless GPU smoke test | `spirula-gpu` | P2-F01–P2-F05 | 编写无窗口 compute smoke test；记录设备、backend 和跳过原因；surface 只保留诚实 unsupported 门禁（不伪造窗口），present 验证移交 P2-F08 | 无窗口 compute 测试在支持设备上可复现；surface 不得伪造 present 成功 | 已完成 |
| P2-F07 | 最小 CLI 包与隔离启动 | workspace/`spirula-xtask` | P2-F06, P1-F09 | 从干净 checkout 构建可携带 CLI 包；记录版本/hash/运行库；在无源码隔离目录运行 help/能力诊断及微型 GPU smoke | 包内入口可启动；真实设备 smoke 或明确硬件缺失；P4-F06/P5-F07 继续加入真实业务样本 | 已完成 |
| P2-F08 | wgpu surface present 验证 | `spirula-gpu` | P2-F01–P2-F05 | 最小真实窗口 surface；`Surface::get_capabilities(&adapter)` 协商 format/present mode；`get_current_texture`→`CurrentSurfaceTexture`（Success/Suboptimal/Timeout/Occluded/Outdated/Lost/Validation）；`Queue::present` 提交；分层证据：像素校验 ≠ present 调用 ≠ 真实窗口可见性；可见性须可重复已知图案窗口证据或受支持平台合成器反馈（写明限度）；texture readback 不能关闭本编号 | 真实窗口下 Layer C 可见性成立才可关闭本编号；主机无法 present/无法提供可见性证据时必须按平台精确记录 unsupported（命令/环境/原因），但 **unsupported 不得冒充本编号已完成，也不得自动降低 Phase 2 出口标准**；不得用 software paint 或 readback 冒充 present；Phase 2 出口仍以本编号的可见性证据或另行书面决策为准 | 未开始 |

Phase 2 出口条件：P2-F01 至 P2-F08 有真实设备或不可用证据，目标开发机
compute smoke 通过、窄资源封装与最小 CLI 包可运行；不支持的 adapter、
shader 错误和 device lost 均有稳定错误；**surface present 由 P2-F08 判定**
（P2-F06 的 unsupported 门禁不计为通过）。随后提前执行 P6-F07 显示路径验证。

### Phase 3：数据与场景兼容

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P3-F01 | 相机投影与坐标约定 | `spirula-core`/`spirula-io` | P0-F04, P1-F04 | 实现透视、全景和畸变参数；固定坐标系和单位；对比参考投影数值与边界相机。正交在参考 `CameraModelType` 中不存在 → 按 AD-P3-F01-01 显式 unsupported，不发明无参考契约的模型 | 迁移投影、畸变、全景和坐标系；fixture 数值与参考一致；正交返回 `SP-INP-004` 而非静默丢弃 | 已完成 |
| P3-F02 | 图像方向与 mask | `spirula-io` | P1-F02 | 读取图像尺寸和 EXIF 方向；规范化 mask 尺寸与通道；测试缺失、旋转和非法元数据 | 处理图像元数据、EXIF 方向和 mask；非法元数据有稳定错误 | 已完成 |
| P3-F03 | COLMAP 数据集读取 | `spirula-io` | P0-F02, P0-F04 | 解析 COLMAP 相机和图像表；解析点云与关联关系；用 golden fixture 验证单位和 round-trip | 读取相机、图像和点云；round-trip 或 golden fixture 通过 | 已完成 |
| P3-F04 | Nerfstudio/Metashape 适配 | `spirula-io` | P3-F03 | 识别 Nerfstudio/Metashape 版本；映射相机和图像字段；记录缺失字段、默认值和格式差异 | 兼容读取并记录格式差异、缺失字段和迁移策略 | 已完成 |
| P3-F05 | 点云、场景变换与区域 | `spirula-core`/`spirula-io` | P1-F04, P3-F01 | 读取点云属性；应用场景平移/旋转/缩放；验证区域边界、序列化和非法输入不改原文件 | 变换、区域和不变量可序列化；非法输入不改变原文件 | 已完成 |
| P3-F06 | splat 场景格式 | `spirula-core`/`spirula-io` | P0-F04, P1-F04 | 定义 splat 属性布局和精度；加入版本与校验；验证空场景、极值和兼容读取 | 定义 splat 属性、版本、校验和兼容读取；与最小 fixture 对照 | 已完成 |
| P3-F07 | checkpoint 元数据读取 | `spirula-io` | P1-F02, P1-F05 | 读取 header/schema；校验块长度/hash及恢复前提；区分模型初始化、完整训练恢复与损坏；保留旧格式读取 | 旧版本及缺块错误稳定；模型 PLY 不误报为完整 resume；P5-F05 补齐新 bundle 读取 | 已完成 |
| P3-F08 | 有界训练数据加载与缓存 | `spirula-io` | P3-F02, P3-F03, P1-F08 | 解码并发/在途字节/packed cache/预取预算；分辨率感知解码；定义采样顺序、取消和预取排空/重建 | 高分辨率与缓存满样本不越预算；失败不 panic；相同采样状态可重建顺序；记录吞吐和内存 | 已完成 |

Phase 3 退出条件：P3-F01 至 P3-F08 的参考 fixtures 可加载，兼容格式有明确
版本策略，非法输入和旧 checkpoint 行为稳定。

Phase 3 执行要点（P3-F01 至 P3-F08 均已合入 main；当前为 P4-F01）：

- **P3-F01 已完成（独立 READY 后落地）**：代码 tip
  `37b7a734fe0f7126a51fa86bd9ec5d2ece6dd758`；独立审查 READY
  `/tmp/spirula-p3-f01-review/REVIEW-37b7a73.md`；集成 tip `a35a384`；契约
  [phase-3/camera.md](phase-3/camera.md)。`fixtures/camera/cpp-golden/camera_math.json`
  已连同字节数与 SHA-256 登记进 `fixtures/manifest.json`。
- **P3-F02 已完成（独立 READY 后落地）**：代码 tip
  `6f2b79ca5da9af5d8b14f5b2e2cd9b16cdacbc90`；merge
  `da0ea365f1a77d820c26e25390b6111aaf7587f2`；契约
  [phase-3/image-orient-mask.md](phase-3/image-orient-mask.md)。C-DATA-07..10；
  admitted `PrepareOrientedImage` / `PrepareNormalizedMask`；focused/e2e、
  after-start cancel、wide probe、golden cmp、xtask verify 8/8 与
  `git diff --check` 通过。第三方 `image` 解码 mid-call 不可抢占为诚实
  Non-claim/boundary，不阻塞 READY。
- **P3-F03 已完成（独立 READY）**：代码 tip
  `813729f8238f6730ad0235416371197fc4bd985a`；契约
  [phase-3/colmap.md](phase-3/colmap.md)。C-DATA-01..03 / C-DATA-06；text+LE
  binary COLMAP；18-model bake + P3-F01 fit；admitted `LoadDataset` /
  CLI `load-dataset`；有界 IO / 取消 / 累计 load 内存预算。剩余诚实边界：dense
  `fit_model` 单次 LM 不可抢占。计划编号总数仍为 **75**（**P2-F08** 未开始）。
- **P3-F04 已完成（独立 READY）**：代码 tip
  `4c7519ae717c6f501da7fa07650f2993b69d93bb`（`git rev-parse` 校验；链上含
  `548ce6d…` / `442b695…` / `4c7519a…`）；契约
  [phase-3/nerfstudio-metashape.md](phase-3/nerfstudio-metashape.md)。
  C-DATA-01 扩展 + C-DATA-04/05；Nerfstudio / PLY / Metashape XML；`.psx`
  typed unsupported；lens / 预算 / BOM+CDATA harness 与 focused 29/29、
  `dataset_load_e2e` 11/11、xtask verify 8/8。诚实 Non-claim：splat（P3-F06）、
  训练 cache 的 application/session 接入、`.psx` zip 运行时。
- **P3-F05 已完成**（独立审查通过并合入主线）：契约
  [phase-3/scene-transform-region.md](phase-3/scene-transform-region.md)。
  C-DATA-15/16 + Region/AABB；`spirula-scene-transform` JSON、六模式中心化、
  `applied_transform` undo、normalize 元数据、相似变换应用和区域原子写入。
  下一项为 P3-F06。
- **P3-F06 已完成**（独立 worktree `codex/p3-f06`，workspace 门禁通过）：契约
  [phase-3/splat-format.md](phase-3/splat-format.md)。C-DATA-18；复用
  `GaussianSplat`/`SplatSet`，支持参考 17 属性集的 ASCII/LE 读取、float32 LE
  原子写入、checked 输出预算、截断/尾随/非有限/取消错误。下一项为 P3-F07。
- **P3-F07 已完成**（独立 worktree `codex/p3-f07`）：契约
  [phase-3/checkpoint-metadata.md](phase-3/checkpoint-metadata.md)。C-DATA-19/21；
  legacy ustar + NPY v1-v3 读取、state schema/version 检查、模型-only/full-resume
  分类、稳定缺失/损坏/不支持错误、路径/重复/长度/有限性/计数/取消/预算门禁。
  新 bundle writer 与新进程恢复保留给 P5-F05/P5-F07。P3-F08 已完成，下一项为 P5-F08。
- **P3-F03 是数据侧关键路径**（已交付）：P3-F04（已交付）、P3-F08、P5-F08、
  P4-F01 以及 P5/P6 的大部分真实流程都经由它。
- P3-F01 只交付 CPU `f64` 投影与相机数学；GPU/WGSL 投影和可微投影 Jacobian
  分别属于 P4-F01/P4-F09，不得用本功能的结果声称渲染或训练已就绪。

### Phase 4：渲染与可微内核

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | `spirula-render` | P2-F06, P3-F01, P3-F06, P1-F04 | scene/camera/render request 和 CPU reference；固定参数化/坐标/色彩/SH/阈值；校验渲染能力与有效选项 | 版本化数值契约和 GPU f32 容差；不支持项显式拒绝；CPU f64 数学门禁单独保留 | 已完成 |
| P4-F02 | splat 上传与资源绑定 | `spirula-render`/`spirula-gpu` | P4-F01, P2-F03, P2-F04 | 上传布局/对齐和 bind group；验证 render/train 共享资源的窄接口；空场景、重用、失效 handle 和释放 | 布局有中间 buffer 对照；共享资源无隐藏 host copy；资源生命周期和分配字节可诊断 | 已完成 |
| P4-F03 | 投影与 tile/bin 构建 | `spirula-render` | P4-F01, P4-F02 | 投影/裁剪和 tile 覆盖；交点数溢出/预算预检；scan 与 CPU 中间 buffer 比较；大 footprint 压力样本 | 数值在容差内；空输入/边界/超预算有稳定结果；记录交点峰值、scratch 和同步耗时 | 当前 |
| P4-F04 | 排序与光栅化 | `spirula-render` | P4-F03 | 深度键和稳定 radix 排序；非整块/重复键/空输入；光栅覆盖、scratch 和高重叠压力；能力回退 | 同深度顺序和边界明确；CPU/确定性样本对照通过；记录分阶段耗时和资源峰值 | 未开始 |
| P4-F05 | 合成与深度 | `spirula-render` | P4-F04 | 实现颜色/透明度合成；实现深度写入和清除；用 golden 图像与误差容限验收 | 颜色、透明度、深度和异常输入有 golden 图像或数值验收 | 未开始 |
| P4-F06 | readback 与预览输出 | `spirula-render`/application/CLI | P4-F05, P2-F03, P1-F09 | readback/图像封装；经共享 application 接入 CLI 真实微型预览；取消/错误/资源回收；更新最小 CLI 包样本 | headless CLI 预览稳定；可取消且无资源泄漏；记录端到端时间和拷贝成本；隔离包可复现 | 未开始 |
| P4-F07 | 训练前向状态与索引契约 | `spirula-render` | P4-F05, P4-F02 | 浮点训练输出；保存/重算前向状态；visible/full 参数映射；状态版本/生命周期和字节预算 | 训练与预览前向对照；空/裁剪场景索引一致；保存状态峰值和释放有证据 | 未开始 |
| P4-F08 | 光栅化与合成反向传播 | `spirula-render` | P4-F07 | 像素到投影参数 VJP；透明度/遮挡/early-out/cutoff 导数；CPU 微型梯度参考 | 平滑区域有限差分通过；生产硬阈值单独测试；记录 eps/容差和失败样本 | 未开始 |
| P4-F09 | 投影与 SH 反向传播 | `spirula-render` | P4-F07 | 协方差/投影 Jacobian；quat normalize/log-scale/opacity 链；SH viewdir→mean；当前支持相机模型 | 每类可训练参数有数值梯度对照；极值/非有限输入有定义；未支持模型不得进入训练 | 未开始 |
| P4-F10 | 梯度聚合与可微渲染验收 | `spirula-render`/`spirula-gpu` | P4-F08, P4-F09, P2-F01 | 跨 tile 梯度聚合及 visible→full gather；WGSL CAS/分阶段归约能力策略；refinement 统计；高重叠压力 | 完整前后向有限差分通过；梯度有限；记录非确定性/原子竞争/峰值；作为 P5-F02 前置 | 未开始 |

P4-F01 已完成：`spirula-render` 提供版本化 `RenderRequest`、固定 degree-0
Gaussian/linear-RGB 参数化、共享阈值契约和 `CpuReference::project`。focused
headless tests 覆盖 OpenGL 相机中心/深度、非有限阈值、不支持 primitive/SH、
相机背面输入和 contract version。完整契约见
[docs/phase-4/render-scene-camera.md](phase-4/render-scene-camera.md)。上传、GPU
投影/排序、光栅化、合成、readback、训练前向和 UI 仍属于后续功能。

P4-F02 已完成：`SplatGpuLayout`/`PackedSplatData`/`GpuSplatBuffer` 固定 64-byte
GPU record，接入 P2-F03 buffer/staging accounting、预算预检、submit/wait 和
context generation 校验；契约见 [phase-4/splat-buffer.md](phase-4/splat-buffer.md)。
P4-F03 负责下一步 projection/tile/bin。

预览里程碑：P4-F01 至 P4-F06 经早期 application/CLI 生成稳定预览，
CPU/GPU 和图像容差通过。Phase 4 退出条件另需 P4-F07 至 P4-F10 的
保存状态、手写 VJP、梯度聚合和数值验证；完成后才能验收训练一步更新。

### Phase 5：训练核心

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P5-F01 | 训练 worker 与 step 调度 | `spirula-train` | P1-F07, P4-F06, P5-F08, P3-F08 | 消费 TrainPlan 和有界 batch；step/schedule、工作状态和安全边界；向唯一应用 owner 报告事件 | 不复制应用状态机；fake/真实 worker 接口一致；step 边界、取消与迟到事件有测试 | 未开始 |
| P5-F02 | loss 与优化器 | `spirula-train` | P5-F01, P4-F10 | 实现已支持 loss 及导数；Adam moments/计数/逐参数 LR 与 schedule；串联前向/VJP/更新；记录资源峰值 | 微型 CPU 一步更新对照；参数/梯度/状态有限；容差与设备记录；不支持 loss 显式拒绝 | 未开始 |
| P5-F03 | densification 与剪枝 | `spirula-train` | P5-F02 | 触发统计/增长/剪枝/上限；同步重映射参数、Adam 和统计；增长前资源预检；RNG/采样状态 | 可复现样本及 split/prune 状态对照；资源不足不发布半更新；空结果/重复执行有定义 | 未开始 |
| P5-F04 | 评估与进度事件 | `spirula-train` | P5-F02 | 固定 eval split/指标/采样；事件频率与实际 step；异步快照；GPU 时间和 host 时间分开 | 慢 UI/CLI 不阻塞训练；估算进度与实际迭代分开；质量及时间样本归档 | 未开始 |
| P5-F05 | checkpoint 保存与 resume | `spirula-train`/`spirula-io` | P1-F05, P3-F07, P5-F02, P5-F03 | 模型/Adam/step/schedule/RNG/sampler/refinement/数据及计划 digest；generation/manifest/marker/指针提交；版本与恢复模式 | 各发布边界故障注入；旧完整版本保留；新进程状态恢复；区分模型重启/容差恢复/位级恢复 | 未开始 |
| P5-F06 | stop、cancel 与部分失败 | `spirula-train` | P5-F01, P5-F05 | 实现合作式 stop/cancel；区分用户取消、可重试和不可恢复错误；验证已保存结果和清理 | 合作式取消、重试、终止状态和已保存结果有测试 | 未开始 |
| P5-F07 | 训练端到端验收 | application/CLI/`spirula-train` | P5-F01, P5-F02, P5-F03, P5-F04, P5-F05, P5-F06 | 真实微型数据集经 CLI 加载/预览/训练/保存；新进程 N→保存→M 对比连续 N+M；更新隔离 CLI 包 | 参数/Adam/采样/指标按声明容差比较；真实取消/损坏/资源失败；完整工作流与测量证据 | 未开始 |
| P5-F08 | 可执行 TrainPlan 与能力校验 | `spirula-train`/application | P1-F02, P2-F01, P3-F03 | 兼容配置+数据摘要+GPU 能力→生效训练计划；数值/能力/组合/预算验证；规范化序列化和版本化 digest | 251 字段兼容读取不变；不支持项/非有限值/冲突显式拒绝；resolved plan 可保存并重算 digest | 已完成 |

P5-F08 已完成：`spirula-train` 提供 `TrainPlanRequest`、`TrainPlan`、
`DatasetSummary`、GPU 能力快照和预算校验；规范化 JSON 与
`sha256-canonical-json-v1` digest 可保存/重建。focused plan tests 覆盖未知字段保留、
非有限/范围/组合拒绝、GPU/预算门禁和 payload 完整性。训练 worker、渲染、checkpoint
和 application/session 接入仍属于后续功能，契约见
[docs/phase-5/train-plan.md](phase-5/train-plan.md)。

Phase 5 退出条件：P5-F01 至 P5-F08 全部有 session 事件和 checkpoint 证据，
训练复用渲染接口且可从失败或取消状态安全恢复。

### Phase 6：CLI 与 Slint 应用壳

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P6-F01 | application/session 完整业务集成 | `spirula-application` | P1-F07, P5-F07 | 扩展早期唯一 owner 接入已实现加载/训练/恢复/评估；为 P7/P8 留 capability 挂接入口；统一 UI/CLI 状态和终态 | 真实服务的迟到事件/失败/订阅恢复通过；保留 P1-F07 背压与取消保证 | 未开始 |
| P6-F02 | CLI 命令与文本事件 | `spirula-cli` | P1-F09, P6-F01 | 完善早期 CLI 的加载/预览/训练/停止/恢复/导出；稳定错误和退出码；完整 help 与事件输出 | 各命令通过共享业务服务；配置和 capability 一致；子进程生命周期测试通过 | 未开始 |
| P6-F03 | Slint 窗口与导航 | `spirula-ui` | P6-F01, P6-F07 | 窗口/导航和视图状态；绑定日志/校验/错误；配置项来自已实现 capability；沿用验证后的呈现接口 | 不直接 IO/GPU；未实现配置不可启动；显示路径有拷贝/延迟/释放记录 | 未开始 |
| P6-F04 | 数据集加载与预览 | `spirula-ui`/application | P4-F06, P6-F01, P6-F03 | 绑定数据集选择和加载命令；显示相机/场景摘要；在共享 session 上展示 P3/P4 fixture 预览 | 通过共享 session 加载 P3 fixture 并显示稳定预览 | 未开始 |
| P6-F05 | 训练 start/stop/resume | `spirula-ui`/application | P5-F05, P6-F01, P6-F03 | 绑定 start/stop/resume；显示 step、指标和错误；验证取消后可保存并恢复 | UI 命令映射到同一 session，进度、取消和失败可见 | 未开始 |
| P6-F06 | CLI/UI 共享事件验收 | application/CLI/UI | P6-F02–P6-F05 | 用同一 fixture 运行 CLI 和 UI；比较命令、状态、事件和输出；覆盖连接断开和 session 失败 | 同一 fixture 下两入口状态、错误和恢复结果一致 | 未开始 |
| P6-F07 | Slint 与 wgpu 显示路径验证 | `spirula-ui`/application/`spirula-gpu` | P2-F06, P1-F07, P1-F08 | 在完整 UI 前验证最小图像/不透明呈现接口；renderer/backend/版本、线程和纹理所有权；共享或 readback 回退 | 记录每帧复制字节/同步/延迟；尺寸变化和关闭释放正确；无证据不宣称零复制；不暴露 wgpu 给 UI | 已完成 |

Phase 6 退出条件：P6-F01 至 P6-F07 通过 headless application 测试和 Slint
行为测试；CLI 与 UI 使用同一 session 服务。尚待 P7/P8 的导出和辅助命令
在本阶段仅验收协议及明确不可用结果，真实行为随对应功能接入并补充测试。

### Phase 7：编辑、网格与导出

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P7-F01 | 选择、mask 与区域编辑 | `spirula-core`/`spirula-ui` | P3-F05, P6-F01 | 定义选择和 mask 数据结构；实现区域编辑命令；测试边界、撤销/失败和持久化 | 编辑命令、边界和撤销/失败语义明确；状态可持久化 | 未开始 |
| P7-F02 | 场景变换与稀疏编辑 | `spirula-core`/`spirula-io` | P7-F01 | 实现选中 splat 的变换；保留未选数据和版本字段；验证重开后状态一致 | 变换后场景可重开，未选数据不变，版本字段可识别 | 未开始 |
| P7-F03 | 网格生成 | `spirula-render`/`spirula-io` | P4-F06, P6-F01 | 定义网格生成输入和参数；处理空结果、内存上限和取消；保存中间错误与进度 | 生成过程、空结果和资源限制有测试与错误信息 | 未开始 |
| P7-F04 | 网格导入导出 | `spirula-io` | P7-F03, P1-F05 | 实现网格格式读写；写入版本、校验和元数据；用 reference viewer 做 round-trip 和原子保存验证 | 格式版本化、校验和原子保存；可被 reference 工具读取 | 未开始 |
| P7-F05 | 图像与视频导出 | `spirula-io`/application | P7-F04, P6-F01 | 实现图像帧/视频导出；报告进度并支持取消/重试；清理部分文件并验证可播放结果 | 导出进度、取消、重试和部分文件清理有测试 | 未开始 |
| P7-F06 | web viewer 资源 | `spirula-io`/`spirula-ui` | P7-F04 | 打包 viewer 资源和场景索引；固定 URL/文件布局；用现有 viewer 打开导出结果 | 生成的资源与现有 viewer 消费契约一致 | 未开始 |
| P7-F07 | 编辑模型视图与分块 splat 导出 | `spirula-core`/`spirula-io`/application | P7-F02, P3-F06, P1-F05 | 选择/裁剪/变换统一模型视图；分块 PLY 写出；点数/坐标/尺度/SH 独立回读；取消和预算 | 原文件不变；编辑结果 round-trip 和图像对照；无需全量中间 PLY；压缩格式资源需求另行声明 | 未开始 |

Phase 7 退出条件：P7-F01 至 P7-F07 的编辑状态可重新打开，导出失败可见且
不会留下不可识别的半成品。

### Phase 8：辅助管线

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P8-F01 | SfM 管线适配 | `spirula-io`/application | P3-F03, P6-F01, P8-F07 | 在技术栈约束内探测 SfM 工具/version/capability；隔离输入输出；统一进程取消、产物验证和状态恢复 | 成功/超时/失败和阶段复用集成测试；输出经格式校验才发布；依赖例外须先记录架构决策 | 未开始 |
| P8-F02 | 稠密重建管线 | `spirula-io`/application | P8-F01 | 定义稠密重建适配接口；检查模型/许可证/平台；用最小样本验证输出和资源限制 | 版本、模型和平台要求明确；最小样本可复现 | 未开始 |
| P8-F03 | 深度与法线管线 | `spirula-io`/application | P2-F06, P6-F01 | 加载深度/法线模型；检查设备和精度能力；验证输出尺寸、格式和不可用错误 | 模型加载、设备能力、输出格式和不可用结果可诊断 | 未开始 |
| P8-F04 | 分割与特征匹配 | `spirula-io`/application | P8-F03 | 接入分割和特征匹配模型；记录许可证和版本；测试空结果、重试和结果格式 | 许可证、模型版本、失败重试和结果格式有记录 | 未开始 |
| P8-F05 | 视频解码 | `spirula-io` | P3-F02, P6-F01, P8-F07 | 探测 codec/平台解码能力；统一工具版本与进程取消；时间戳/方向；损坏视频和帧顺序 | 能力和依赖显式检查；取消终止后代进程；部分产物清理；资源预算和错误可测 | 未开始 |
| P8-F06 | 模型管理 | `spirula-io`/application | P8-F01–P8-F05 | 实现模型下载和本地缓存；校验 hash/版本；支持取消、失败清理和迁移 | 模型下载、缓存、校验、取消和版本迁移语义明确 | 未开始 |
| P8-F07 | 外部工具锁定与进程适配基础 | `spirula-io`/application | P1-F05, P1-F07, P1-F08 | 固定 source/target/URL/hash/version/help/许可证；安全解压/离线校验/安装锁/回滚；有界日志和进程树取消 | 伪工具隔离测试损坏包/路径穿越/安装失败/取消；真实工具按平台单独验收；不改变现有运行时约束 | 未开始 |

Phase 8 退出条件：P8-F01 至 P8-F07 各有隔离测试、能力矩阵和失败信息，
缺少外部模型或 codec 时不会被误报为已完成。

### Phase 9：跨平台、性能与发布

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P9-F01 | 跨平台 adapter 验证 | `spirula-gpu`/CI | P2-F06 | 在三平台枚举 adapter；记录 backend、limits 和 feature；归档不可用设备原因 | macOS、Linux、Windows 的设备能力和不可用结果有记录 | 未开始 |
| P9-F02 | 正确性与性能基线汇总 | workspace | P1-F08, P4-F10, P5-F07 | 汇总逐功能测量；固定质量/收敛、step 分布、CPU/GPU 峰值、同步、启动、保存/恢复和取消延迟 | 同设备/数据/recipe 可复现；算子对照与端到端 time-to-quality 分开；不把编译或测试存在视为 GPU 实测 | 未开始 |
| P9-F03 | profiling 与 GPU 缓存 | `spirula-gpu`/`spirula-render` | P9-F02 | 采集 profile 和热点；实现 shader/pipeline 缓存；用同一基线解释性能收益和回归 | 先有基线，再用 profile 证明 shader/pipeline 缓存和优化没有未解释回归 | 未开始 |
| P9-F04 | 完整安装包与发布清单 | workspace/CI | P2-F07, P6-F02, P6-F03, P7-F05 | 在最小 CLI 包上完成 Slint 安装包/三平台矩阵/签名政策；版本/hash、能力/依赖/许可证清单；干净环境验证 | 隔离安装后微型真实流程可复现；平台未实测范围明确；不得由构建成功推断训练可用 | 未开始 |
| P9-F05 | 崩溃报告与已知差异 | application/文档 | P6-F01, P9-F01 | 定义崩溃报告字段和隐私边界；记录平台差异和已知限制；编写回滚和问题复现步骤 | 崩溃信息、隐私边界、已知平台差异和回滚步骤有文档 | 未开始 |

Phase 9 退出条件：P9-F01 至 P9-F05 的发布清单可复现，性能结果有基线和
环境，能力开关与已知差异均已记录。

## 4. 首批功能顺序

已完成 P0-F01–P0-F06、P1-F01–P1-F09、P2-F01–P2-F07、提前执行的 P6-F07、
P3-F01、P3-F02、P3-F03、P3-F04、P3-F05、P3-F06、P3-F07、P3-F08；Phase 1 退出条件已满足，Phase 2 退出条件尚未满足（surface
present 仍未验证，不得因 P2 功能点全部完成或 P6-F07 tip 完成而推断 Phase 2
出口通过）。**当前功能为 P4-F03**。计划编号总数 **75**（**P2-F08** 未开始）。
后续按依赖推进，每个编号独立 worktree、实现和验收；下列分组只是执行顺序，
不会把组内多个编号合并成一个已完成功能：

1. P1-F06–P1-F09（已完成）：基础检查、唯一会话、测量和最小 CLI。
2. P2-F01–P2-F07（已完成）：GPU 能力、真实窄资源路径与最小可运行包。
3. 提前执行 P6-F07（已完成，tip `82adae0eebbb1f1a0f20ddfec1d7dd824ae7006c`）：
   验证 Slint 显示及每帧复制成本；不宣称零复制 / surface present。
4. P3-F01（已完成并落地，tip `37b7a734fe0f7126a51fa86bd9ec5d2ece6dd758`；契约
   [phase-3/camera.md](phase-3/camera.md)）→ P3-F02（已完成并落地，tip
   `6f2b79ca5da9af5d8b14f5b2e2cd9b16cdacbc90`；契约
   [phase-3/image-orient-mask.md](phase-3/image-orient-mask.md)）→
   P3-F03（已完成，tip `813729f8238f6730ad0235416371197fc4bd985a`；契约
   [phase-3/colmap.md](phase-3/colmap.md)）→ P3-F04（已完成，tip
   `4c7519ae717c6f501da7fa07650f2993b69d93bb`；契约
   [phase-3/nerfstudio-metashape.md](phase-3/nerfstudio-metashape.md)）→
   **P3-F05（已完成）** → **P3-F06（已完成）** → **P3-F07（已完成）** → **P3-F08（已完成）**：微型数据集、格式和有界加载继续各自前置。正交在
   参考中无 `CameraModelType` → AD-P3-F01-01 显式 unsupported（`SP-INP-004`）；
   CPU host golden 与后续 GPU 容差分离；exact-fit active source `-1` 保留
   original metadata。其中 **P3-F03（COLMAP 读取）是数据侧关键路径**（已交付）：
   P5-F08、P4-F01 以及几乎所有 P3 后续编号都直接或间接依赖它。
5. **P5-F08：可执行训练计划（已完成）** → **P4-F01：渲染场景与相机接口（已完成）** → **P4-F02：splat 上传与资源绑定（已完成）** → **P4-F03：投影与 tile/bin 构建（当前）**；随后 P4-F04–P4-F06：经共享 CLI 输出真实预览。
6. P4-F07–P4-F10：训练前向状态、两段 VJP、梯度聚合与数值验收。
7. P5-F01–P5-F04：训练调度、一步优化、增长/剪枝和评估。
8. P5-F05/P5-F06 → P5-F07：完整检查点、新进程恢复与微型训练包。
9. P6-F01–P6-F06：完善业务集成、完整 CLI 和 Slint，共享同一会话。
10. **P2-F08（surface present 验证）**：与第 4–9 步并行，不阻塞数据/渲染/训练
    主线；它是 Phase 2 出口的唯一剩余条件，越晚验证，P4-F06 和 P6-F03 的显示
    假设越可能返工。验收须分开像素校验、`Queue::present` 提交与真实窗口
    可见性；readback 不能关闭本编号。若目标开发机无法提供真实窗口 present
    或 Layer C 可见性证据，必须按平台记录 unsupported（命令/环境/原因）；
    **该记录不等于 P2-F08 已完成，也不自动降低或关闭 Phase 2 出口**——任何
    出口书面调整须另行审查，且不得用 unsupported、software paint 或 readback
    偷换 present 验收。
11. P7-F01–P7-F07：编辑、网格、分块 splat 导出和其他导出。
12. P8-F07 → P8-F01/P8-F05，再按前置推进 P8-F02–P8-F04/P8-F06。
13. P9-F01–P9-F05：汇总平台、质量/性能与完整发布；各功能的资源/数值测量已在此前执行。

## 5. 单个功能点的完成标准

功能点只有在对应编号的适用条件全部满足后才能标记为 `已完成`：

- 已从独立 Git worktree 开发，并在该 worktree 内完成测试和 diff 检查；
- 明确归属 crate、公共 API、输入输出、前置功能和依赖方向；
- 记录 C++ 行为、格式兼容假设和迁移说明；
- 覆盖正常输入、非法输入、取消、部分失败和持久化恢复；
- GPU 功能有 CPU reference 或确定性 fixture，前后向分别验证，并记录数值容差；
- 按 [开发门禁](development-gates.md) 提交资源预算、能力/回退、背压及恢复证据；
- 有 headless 测试；Slint 行为与 application 服务分开测试；
- 涉及性能时记录基线、测量命令、环境、指标和回归解释；
- 更新受影响的架构、计划、fixture、迁移记录和 README，且状态与代码一致；
- 通过 `cargo fmt --all -- --check`；
- 通过 `cargo check --workspace`；
- 通过 `cargo test --workspace`；
- 通过 `cargo clippy --workspace --all-targets --all-features -- -D warnings`；
- Git commit 只包含一个连贯功能，且可从干净 checkout 重现；
- 硬件、模型或 fixture 缺失时记录具体命令、环境和原因，不能把未测试功能标为完成。

## 6. Git 提交策略

每个功能点都执行：

1. 按编号从目标基线创建独立 Git worktree 和功能分支；
2. 盘点参考实现和兼容性，补齐契约、失败行为和验收证据；
3. 在同一 worktree 内实现功能、测试、fixture 和文档；
4. 运行局部测试及完整 workspace 检查；
5. 检查 `git diff`、生成文件、文档状态和 worktree 状态；
6. 使用一个包含功能点编号的 Conventional Commit；
7. 充分开发和测试通过后才合并，合并后再清理 worktree。

提交示例：

```text
feat(P1-F01): add stable domain errors and ids
feat(P2-F06): add headless wgpu smoke test
feat(P3-F03): parse colmap fixtures
feat(P4-F02): upload splat scene resources
feat(P5-F05): save and resume tiny training run
feat(P6-F03): add Slint application shell
```

Phase 0 已完成，证据见 `docs/phase-0/exit-report.md`。P1-F01（领域错误与稳定
ID）已在 `spirula-core` 落地，契约补记见 `docs/phase-0/runtime-contracts.md`
第 1.4 节。P1-F02（配置与版本化序列化）已在 `spirula-core` 落地，契约见
`docs/phase-1/config.md`。P1-F03（取消与进度原语）已在 `spirula-core` 落地，契约见
`docs/phase-1/cancellation.md`。P1-F04（数学、相机与场景基础类型）已在 `spirula-core` 落地，契约见
`docs/phase-1/geometry.md`。完整镜头、场景序列化和 splat 文件布局仍分别属于 P3-F01、P3-F05 和
P3-F06。P1-F05（原子文件与 fixture harness）已在 `spirula-io` 落地，契约见
`docs/phase-1/atomic.md`；配置文件的原子保存与迁移备份也在此完成。Checkpoint
目录交换与完成标记仍属于 P5-F05。P1-F06（workspace 基础检查）已固化
`spirula-xtask verify` 的 fmt/check/test/clippy 入口、workspace lint 继承、
架构依赖边界与声明 feature 组合，契约见 `docs/phase-1/workspace-checks.md`，
记录见 `docs/phase-1/exit-report.md`。P1-F07（最小 headless
application/session）已在 `spirula-application` 落地，契约见
`docs/phase-1/application.md`（仅依赖 `spirula-core`，fake probe，无训练/
GPU）。P1-F08（资源与性能记录基础设施）已在
`spirula-core`/`spirula-io` 落地，契约见 `docs/phase-1/performance.md`
（版本化报告、fixture/config hash、已核算字节与不可用 OS/GPU/时间戳字段、
CPU 微型样本生成与回读；无训练/GPU/UI 运行时）。P1-F09（最小 CLI 会话入口）已在 `spirula-cli` + application disk
diagnostics 落地，契约见 `docs/phase-1/cli.md`（help/capabilities、fake
probe、SIGINT 0/1/2/130、rotated disk sink；真实 train/preview 仍不可用）。
独立审查 READY，代码 tip `958d6ab2693a4e4cdd72b57140c5287c996593c7`；host-only
`spirula-xtask verify` 8/8。P2-F01（adapter 选择与能力协商）已在
`spirula-gpu` 落地，契约见 `docs/phase-2/gpu-adapter.md`（wgpu 30.0.1、
required/optional feature 与 limit、`ResolvedLimits` 保留 required floor、
`spirula-gpu-diagnose`）。P2-F02（device/queue/context 生命周期）已在
`spirula-gpu` 落地并独立批准，契约见 `docs/phase-2/device-context.md`
（session-owned `GpuContext`、同 snapshot index 配对、协商写入
`DeviceDescriptor`、`DeviceHandle` identity/generation、显式 close/`reopen`、
lost latch→owner poll、不写 SessionState、无隐式重建；窄入口
`spirula-gpu-diagnose --device`）。代码 tip
`75d1bd48efcc8ba6451af368ce60387e6cead6b9`；独立 full gate 8/8 与 host
owner/callback 证据见该契约。**诚实边界**：API owner 尚未接入
`spirula-application`；无 compute/training/UI 就绪声称。P2-F03
（buffer/texture/readback）已在 `spirula-gpu` 落地并独立批准，契约见
`docs/phase-2/gpu-resources.md`（窄标签与 handle、staging upload/readback、
显式 submit/wait/map、pending/in-flight 记账、ticket RAII、submission reap、
对齐/`max_buffer_size` 预检、wait 丢设备与 map deadline `unmap` 取消）；入口
`spirula-gpu-diagnose --resources`。代码 tip
`e2157c1e0578edd3d65014a5ad220400a14de65e`；parent full verify 8/8 与
M5 Max/Metal host 4/4、`after_drop` current 0 见该契约。**诚实边界**：当时尚无
pipeline/WGSL compute（已由 P2-F04 补齐）；application/train/UI 未接入。
P2-F04（shader/pipeline 契约）已在 `spirula-gpu` 落地并独立批准，契约见
`docs/phase-2/gpu-pipelines.md`（WGSL `shaders/<name>.wgsl`、key=
name/version/entry/spec/bind_layout_version、affine v1/v2 同 ABI 仅作 epoch
失效验证、SP-GPU-005、pending 命令↔staging 所有权、`--compute`）。代码 tip
`2bd98deae866da09a3c14f694d9d2028f8b17f35`；parent full verify 8/8 与
M5 Max/Metal host 3/3、`match=cpu_exact` / `after_drop` current 0 见该契约
（可复现命令优先；`/tmp` 日志为样本）。**诚实边界**：application/train/UI 未
接入；不声称 P9-F03 缓存性能优化。
P2-F05（GPU 错误与诊断事件）已在 `spirula-gpu` 落地并独立批准，契约见
`docs/phase-2/gpu-diagnostics.md`（error scope→`SP-GPU-001..005`、Lost+旧
handle 拒绝、有界 diagnostic ring、upload/readback/map 元数据、post-submit
scope 返回可 wait handle 且 wait 在 reclaim 后返回 latched typed Err；真实
OOM **unsupported**；60 s stall watchdog deferred；不接 application
session）。代码 tip `31b6087695fcf54322bca822b2de36f1f906b6fe`；独立 READY
`/tmp/spirula-p2-f05-review/REVIEW-final-31b6087.md`。
P2-F06（headless GPU smoke）已在 `spirula-gpu` 落地并独立批准，契约见
`docs/phase-2/gpu-smoke.md`（无窗口 affine compute smoke 复用 P2-F04/P2-F05、
adapter/device unavailable 精确 skip、validation/lost/timeout fail + Fail
reclaim、最小 surface 诚实 **unsupported**/不伪造窗口且**不**声称 present
已验证；记录设备/backend/features/limits 与 cleanup current0）。代码 tip
`e66af3d6e5ecdd4b6ecaac90759fad0079489a95`；独立 READY
`/tmp/spirula-p2-f06-review/REVIEW-final-e66af3d.md`；parent full verify 8/8
与 host 5/5 + synthetic 6/6 见该契约（样本
`/tmp/spirula-codex-p2-f06-e66af3d-independent-verify.log`、
`/tmp/spirula-codex-p2-f06-e66af3d-focused.log`）。P2-F07（最小 CLI 包与隔离
启动）**已完成**：代码 tip
`17c1202a6a2b70183192a1367d96ffbb0e992e67`；独立 READY
`/tmp/spirula-p2-f07-review/REVIEW-final-17c1202.md`；owner
`spirula-xtask`；`package`/`package-check`/`package-run` 以 `--locked` +
完整 staging 一次 `renameat_with(NOREPLACE)` 发布；MANIFEST/NOTICE/allowlist
与恰好三个 canonical `bin/<name>`；macOS 实测（含
`/tmp/spirula-codex-cli-pkg-17c1202` 隔离 Metal smoke pass/cpu_exact/current0）；
Linux 同路径实现但本 tip **未**主机实测；Windows publish **unsupported**；CLI
不新增 GPU 边；无签名/三平台/P9 安装包。契约
[phase-2/cli-package.md](phase-2/cli-package.md)。提前执行的 **P6-F07 已完成**
（tip `82adae0eebbb1f1a0f20ddfec1d7dd824ae7006c`；独立 READY
`/tmp/spirula-p6-f07-review/REVIEW-final-82adae0.md`；契约
[phase-6/presentation-path.md](phase-6/presentation-path.md)；AD-P6-F07-01
application→gpu 窄边；readback→Slint `Image`；CUA 可见软件绘制；
app `software_paint` notifier 在 Unsupported 时仍 unverified；复制字节与
host produce→deliver 已测；**进程 CPU 总峰值未测**；不宣称零复制 /
wgpu surface present）。**P3-F01 已完成并落地 main**（代码 tip
`37b7a734fe0f7126a51fa86bd9ec5d2ece6dd758`；独立 READY
`/tmp/spirula-p3-f01-review/REVIEW-37b7a73.md`；集成 tip `a35a384`；契约
[phase-3/camera.md](phase-3/camera.md)；C-DATA-11..14；pin `9a24f099…` +
`tools/camera-golden` / holdout；正交 AD-P3-F01-01 unsupported；active
source `-1` vs original metadata；golden 已在 `fixtures/manifest.json`
登记）。**P3-F02 已完成并落地 main**（代码 tip
`6f2b79ca5da9af5d8b14f5b2e2cd9b16cdacbc90`；merge `da0ea36…`；契约
[phase-3/image-orient-mask.md](phase-3/image-orient-mask.md)；C-DATA-07..10；
第三方 `image` mid-decode Non-claim 不阻塞 READY）。**P3-F03 已完成（独立
READY）**（代码 tip `813729f8238f6730ad0235416371197fc4bd985a`；契约
[phase-3/colmap.md](phase-3/colmap.md)；C-DATA-01..03 / C-DATA-06）。按原路线
**当前功能为 P4-F03**。计划编号总数仍为 **75**。**诚实边界**：P2-F01–P2-F07
与 P6-F07 tip 全部完成 **不** 等于 Phase 2 出口通过——surface present 仍未
验证（**P2-F08 未开始**）；正式 UI / train/preview / P6-F03 导航未接入；不声称
真实硬件 OOM；60 s watchdog 仍 deferred；dense `fit_model` 单次 LM 不可抢占；
P3-F05 Non-claim：mesh/prism/label region、EXIF normalize leveling；P3-F08 已完成；训练 cache 的 application/session 接入仍未开始。2026-10-25 的 OOOSplat / Brush 审查已形成新增功能与依赖调整，见
[审查记录](research/ooosplat-brush-review.md)。训练与正式 UI 实现状态没有因
P6-F07 tip、P3-F01–P3-F05 已完成而提前完成。

2026-10-27 计划评审新增 **P2-F08（wgpu surface present 验证）**，编号总数
由 74 增至 75（此前文档中的“74 编号”仅保留该日之前的历史含义）。原因：
P2-F06 的验收文字要求“最小 surface 测试在支持设备上可复现”，但该功能诚实
交付的是 unsupported 门禁，没有任何编号负责真正验证 present，导致 Phase 2
出口缺少负责人而无限期挂起。P2-F08 接手该责任，P2-F06 的验收文字同步收窄。
既有编号不重排、不复用；P2-F08 追加在 P2 末尾。unsupported 证据不能偷换
功能完成，也不能自动降低 Phase 2 出口标准。契约钉死 wgpu 30.0.1：
`Surface::get_capabilities`、`CurrentSurfaceTexture` 全分支、`Queue::present`
返回 `()`；可见性证据与像素 readback 分离，见
[phase-2/gpu-surface-present.md](phase-2/gpu-surface-present.md)。

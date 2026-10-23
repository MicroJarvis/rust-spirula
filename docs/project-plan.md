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

## 3. 功能点编号与状态

每个可独立开发、测试和合并的交付单元都必须有全局唯一编号，格式为
`P<phase>-F<feature>`，例如 `P0-F01`。编号一经分配不得复用；代码、测试、
fixture、迁移记录和提交说明都使用同一编号。状态只允许使用 `当前`、`未开始`、
`进行中`、`已完成`、`阻塞`，并在每次开发完成后同步更新。

每个功能点都要明确：归属 crate、前置功能、输入输出或公共 API、正常和
失败行为、测试/fixture、文档变更以及完成门禁。“细分范围”列是实现清单，
每个分号分隔项都必须在代码、测试或记录中留下证据。下表是当前基线；`当前` 表示
下一项工作，完成后要把下一项改为 `当前`。

### Phase 0：基线与架构

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P0-F01 | 固定 C++ 参考基线 | 文档/工具 | — | 记录仓库地址和 commit；固定构建工具链与依赖版本；保存构建、测试和失败日志入口 | 记录 commit、构建方式、测试入口、依赖版本和可复现命令 | 已完成 |
| P0-F02 | 建立子系统行为清单 | 文档 | P0-F01 | 列出 C++ 主入口和调用链；标注数据读写者与格式；记录正常输出、错误和取消路径 | 记录数据集、相机、checkpoint、CLI、UI、GPU 入口、读写者和可观察输出 | 已完成 |
| P0-F03 | 建立能力矩阵 | 文档 | P0-F02 | 按数据、渲染、训练、UI、CLI 分类；为每项标注源码位置和 owner；定义验证方式与当前状态 | 每项能力有参考源码、输入格式、输出、兼容假设、验证方式和负责人 | 已完成 |
| P0-F04 | 建立最小兼容 fixtures | `spirula-core`/`spirula-io` | P0-F02, P0-F03 | 制作单相机/小图像样本；制作小 splat/点云/checkpoint；保存 golden 元数据、图像和校验值 | 提供相机、数据集、场景变换、splat、checkpoint、微型渲染样本及 golden 结果 | 已完成 |
| P0-F05 | 固化运行时契约 | `spirula-core`/架构 | P0-F02, P0-F03 | 定义错误类别和稳定字段；定义命令、事件、状态转换和取消；记录 GPU 资源与 shader 边界 | 定义错误、事件、session 状态、取消、GPU 边界、shader 约定和版本策略 | 当前 |
| P0-F06 | 建立验证入口 | workspace | P0-F01–P0-F05 | 提供一条命令运行 fixture 检查；提供文档/状态一致性检查；记录 Phase 0 退出报告 | 提供 fixture 检查、文档状态检查和 Phase 0 验收命令；干净 checkout 可复现 | 未开始 |

Phase 0 退出条件：P0-F01 至 P0-F06 均有记录，工作区可构建，架构/能力矩阵
和第一批 fixtures 可被其他功能点直接引用。

### Phase 1：Rust 基础层

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P1-F01 | 领域错误与稳定 ID | `spirula-core` | P0-F05 | 定义领域错误枚举；分配稳定错误码；保留来源、字段和用户可读信息 | 定义可匹配错误类别、稳定 ID 和显示信息；覆盖非法输入测试 | 未开始 |
| P1-F02 | 配置与版本化序列化 | `spirula-core` | P0-F05, P1-F01 | 定义配置字段和默认值；加入 schema/version；测试缺失、未知和非法字段 | 配置有默认值、校验、版本字段和兼容读取失败语义 | 未开始 |
| P1-F03 | 取消与进度原语 | `spirula-core` | P0-F05, P1-F01 | 定义取消 token 和检查点；定义进度值与事件载荷；测试重复取消、终止和错误终态 | 定义合作式取消、进度值和事件载荷；测试重复取消和终态 | 未开始 |
| P1-F04 | 数学、相机与场景基础类型 | `spirula-core` | P0-F04, P1-F01 | 实现向量、矩阵和四元数；实现相机投影与坐标变换；定义场景和 splat 不变量 | 实现变换、投影、坐标约定和场景不变量；与 P0 fixtures 对照 | 未开始 |
| P1-F05 | 原子文件与 fixture harness | `spirula-io` | P0-F04, P1-F01 | 实现临时文件命名与原子替换；处理损坏和中断写入；提供可复用临时目录 fixture harness | 临时文件加原子替换；损坏、并发失败和恢复路径有测试 | 未开始 |
| P1-F06 | workspace 基础检查 | workspace | P0-F06 | 固定 fmt/check/test/clippy 命令；固定 lint 与 feature 组合；检查 CLI 仅作入口适配 | 固化 lint、格式化、测试和本地检查命令；CLI 不含隐藏业务逻辑 | 未开始 |

Phase 1 退出条件：P1-F01 至 P1-F06 通过 workspace 检查，核心契约可被后续
IO、GPU、渲染和训练功能复用。

### Phase 2：wgpu 基础层

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P2-F01 | adapter 选择与能力协商 | `spirula-gpu` | P1-F06 | 枚举 adapter 和 backend；声明必需 feature/limit；实现无可用设备时的候选诊断 | 按 feature/limit 选择 adapter；不可用时返回可操作错误 | 未开始 |
| P2-F02 | device、queue 与上下文生命周期 | `spirula-gpu` | P2-F01 | 创建 instance、adapter、device、queue；定义所有权和释放顺序；覆盖 device lost 与重建 | 定义创建、关闭、device lost 和资源所有权；覆盖失败路径 | 未开始 |
| P2-F03 | buffer、texture 与 readback | `spirula-gpu` | P2-F02 | 定义 buffer/texture 用途和标签；实现 staging 与同步；验证最小上传和 readback round-trip | 定义资源标签、staging、同步和 readback 契约；提供最小 round-trip | 未开始 |
| P2-F04 | shader 与 pipeline 契约 | `spirula-gpu` | P2-F02 | 定义 WGSL 文件加载和版本；固定 bind group/uniform layout；生成可解释的 pipeline key 与编译错误 | WGSL 加载、uniform/layout、pipeline key 和编译错误可诊断 | 未开始 |
| P2-F05 | GPU 错误与诊断事件 | `spirula-gpu` | P2-F02–P2-F04 | 包裹 error scope 和提交错误；把 device lost 转成 typed error；发布带资源标签的诊断事件 | error scope、设备丢失和提交失败转为 typed error/event | 未开始 |
| P2-F06 | headless GPU smoke test | `spirula-gpu` | P2-F01–P2-F05 | 编写无窗口 compute smoke test；编写最小 surface smoke test；记录设备、backend 和跳过原因 | 无窗口 compute 测试和最小 surface 测试在支持设备上可复现 | 未开始 |

Phase 2 退出条件：P2-F01 至 P2-F06 在目标开发机通过；不支持的 adapter、
shader 错误和 device lost 均有稳定错误信息。

### Phase 3：数据与场景兼容

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P3-F01 | 相机投影与坐标约定 | `spirula-core`/`spirula-io` | P0-F04, P1-F04 | 实现透视、正交、全景和畸变参数；固定坐标系和单位；对比参考投影数值与边界相机 | 迁移投影、畸变、全景和坐标系；fixture 数值与参考一致 | 未开始 |
| P3-F02 | 图像方向与 mask | `spirula-io` | P1-F02 | 读取图像尺寸和 EXIF 方向；规范化 mask 尺寸与通道；测试缺失、旋转和非法元数据 | 处理图像元数据、EXIF 方向和 mask；非法元数据有稳定错误 | 未开始 |
| P3-F03 | COLMAP 数据集读取 | `spirula-io` | P0-F02, P0-F04 | 解析 COLMAP 相机和图像表；解析点云与关联关系；用 golden fixture 验证单位和 round-trip | 读取相机、图像和点云；round-trip 或 golden fixture 通过 | 未开始 |
| P3-F04 | Nerfstudio/Metashape 适配 | `spirula-io` | P3-F03 | 识别 Nerfstudio/Metashape 版本；映射相机和图像字段；记录缺失字段、默认值和格式差异 | 兼容读取并记录格式差异、缺失字段和迁移策略 | 未开始 |
| P3-F05 | 点云、场景变换与区域 | `spirula-core`/`spirula-io` | P1-F04, P3-F01 | 读取点云属性；应用场景平移/旋转/缩放；验证区域边界、序列化和非法输入不改原文件 | 变换、区域和不变量可序列化；非法输入不改变原文件 | 未开始 |
| P3-F06 | splat 场景格式 | `spirula-core`/`spirula-io` | P0-F04, P1-F04 | 定义 splat 属性布局和精度；加入版本与校验；验证空场景、极值和兼容读取 | 定义 splat 属性、版本、校验和兼容读取；与最小 fixture 对照 | 未开始 |
| P3-F07 | checkpoint 元数据读取 | `spirula-io` | P1-F02, P1-F05 | 读取 checkpoint header 和版本；校验必需块与完整性；为旧版本和损坏文件返回恢复前置条件 | 识别旧 checkpoint 版本、完整性和恢复前置条件；损坏文件可诊断 | 未开始 |

Phase 3 退出条件：P3-F01 至 P3-F07 的参考 fixtures 可加载，兼容格式有明确
版本策略，非法输入和旧 checkpoint 行为稳定。

### Phase 4：渲染 MVP

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | `spirula-render` | P2-F06, P3-F06, P1-F04 | 定义 scene/camera/render request；定义图像、深度和统计输出；实现可比较的 CPU reference 入口 | 定义渲染输入、相机、输出图像和误差容限；提供 CPU reference 入口 | 未开始 |
| P4-F02 | splat 上传与资源绑定 | `spirula-render`/`spirula-gpu` | P4-F01, P2-F03 | 把 splat 属性上传到 GPU；建立 bind group 和资源标签；测试空场景、重用和释放 | 将场景上传 GPU，资源布局和生命周期可诊断 | 未开始 |
| P4-F03 | 投影与 tile/bin 构建 | `spirula-render` | P4-F01–P4-F02 | 实现屏幕投影；计算 tile/bin 覆盖；比较 CPU/GPU 中间 buffer 与边界样本 | CPU reference 与 headless GPU 结果在记录容限内一致 | 未开始 |
| P4-F04 | 排序与光栅化 | `spirula-render` | P4-F03 | 实现深度键和排序规则；实现光栅化覆盖；测试同深度、越界、空输入和确定性 | 深度/顺序规则、边界和空场景有确定性测试 | 未开始 |
| P4-F05 | 合成与深度 | `spirula-render` | P4-F04 | 实现颜色/透明度合成；实现深度写入和清除；用 golden 图像与误差容限验收 | 颜色、透明度、深度和异常输入有 golden 图像或数值验收 | 未开始 |
| P4-F06 | readback 与预览输出 | `spirula-render`/`spirula-gpu` | P4-F05, P2-F03 | 实现 GPU readback 和图像封装；接入取消与资源回收；验证 headless 预览和提交失败 | headless API 生成稳定预览，错误和取消不会泄漏资源 | 未开始 |

Phase 4 退出条件：P4-F01 至 P4-F06 可通过 application/session 生成稳定预览，
CPU/GPU 对照和图像容差测试通过。

### Phase 5：训练核心

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P5-F01 | 训练 session 与 step 调度 | `spirula-train` | P1-F03, P4-F06 | 定义 session 状态和 step 计数；定义启动/停止/失败事件顺序；测试边界 step 和重复命令 | 定义状态机、step 边界、终止原因和事件顺序 | 未开始 |
| P5-F02 | loss 与优化器 | `spirula-train` | P5-F01 | 实现 loss 输入与输出；实现优化器状态；用微型场景验证一步更新和确定性 | 微型场景 loss、优化器状态和确定性输入输出有测试 | 未开始 |
| P5-F03 | densification 与剪枝 | `spirula-train` | P5-F02 | 实现 densification 触发统计；实现剪枝阈值和上限；测试资源不足、空结果和重复执行 | 触发条件、上限和失败行为明确；结果可复现 | 未开始 |
| P5-F04 | 评估与进度事件 | `spirula-train` | P5-F01 | 定义评估采样与指标；定义事件频率和 payload；验证 UI/CLI 可消费且不阻塞训练 | 指标、频率、进度和错误事件可被 UI/CLI 消费 | 未开始 |
| P5-F05 | checkpoint 保存与 resume | `spirula-train`/`spirula-io` | P1-F05, P3-F07, P5-F01 | 定义 checkpoint schema 和临时路径；保存模型/优化器/step 元数据；从中断文件恢复并继续 | 原子保存、版本校验、崩溃恢复和继续训练通过微型 fixture | 未开始 |
| P5-F06 | stop、cancel 与部分失败 | `spirula-train` | P5-F01, P5-F05 | 实现合作式 stop/cancel；区分用户取消、可重试和不可恢复错误；验证已保存结果和清理 | 合作式取消、重试、终止状态和已保存结果有测试 | 未开始 |
| P5-F07 | 训练端到端验收 | `spirula-train`/`spirula-render` | P5-F01–P5-F06 | 串联 session、render 和 checkpoint；固定微型训练步数；比较 resume 前后指标、输出和事件 | 微型场景达到确定 step，resume 后指标和输出在约定范围内 | 未开始 |

Phase 5 退出条件：P5-F01 至 P5-F07 全部有 session 事件和 checkpoint 证据，
训练复用渲染接口且可从失败或取消状态安全恢复。

### Phase 6：CLI 与 Slint 应用壳

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P6-F01 | application/session 服务 | `spirula-application`（新增） | P1-F03, P5-F01 | 定义 application command/event API；实现 bounded channel 和 session owner；统一 UI/CLI 取消与进度 | 统一命令、状态、取消、进度和事件；CLI/UI 不重复编排 | 未开始 |
| P6-F02 | CLI 命令与文本事件 | `spirula-cli` | P6-F01 | 实现加载/预览/训练/停止/恢复/导出参数；映射稳定错误码和退出码；输出 typed event 文本 | 解析加载、预览、训练、停止、恢复和导出命令；错误码稳定 | 未开始 |
| P6-F03 | Slint 窗口与导航 | `spirula-ui` | P6-F01 | 建立 Slint 窗口和导航模型；绑定日志、校验和错误反馈；禁止 UI 直接创建 GPU/IO 对象 | 建立窗口、页面状态、日志和验证反馈；不直接访问 IO/GPU | 未开始 |
| P6-F04 | 数据集加载与预览 | `spirula-ui`/application | P4-F06, P6-F01, P6-F03 | 绑定数据集选择和加载命令；显示相机/场景摘要；在共享 session 上展示 P3/P4 fixture 预览 | 通过共享 session 加载 P3 fixture 并显示稳定预览 | 未开始 |
| P6-F05 | 训练 start/stop/resume | `spirula-ui`/application | P5-F05, P6-F01, P6-F03 | 绑定 start/stop/resume；显示 step、指标和错误；验证取消后可保存并恢复 | UI 命令映射到同一 session，进度、取消和失败可见 | 未开始 |
| P6-F06 | CLI/UI 共享事件验收 | application/CLI/UI | P6-F02–P6-F05 | 用同一 fixture 运行 CLI 和 UI；比较命令、状态、事件和输出；覆盖连接断开和 session 失败 | 同一 fixture 下两入口状态、错误和恢复结果一致 | 未开始 |

Phase 6 退出条件：P6-F01 至 P6-F06 通过 headless application 测试和 Slint
行为测试；CLI 与 UI 使用同一 session 服务。

### Phase 7：编辑、网格与导出

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P7-F01 | 选择、mask 与区域编辑 | `spirula-core`/`spirula-ui` | P3-F05, P6-F01 | 定义选择和 mask 数据结构；实现区域编辑命令；测试边界、撤销/失败和持久化 | 编辑命令、边界和撤销/失败语义明确；状态可持久化 | 未开始 |
| P7-F02 | 场景变换与稀疏编辑 | `spirula-core`/`spirula-io` | P7-F01 | 实现选中 splat 的变换；保留未选数据和版本字段；验证重开后状态一致 | 变换后场景可重开，未选数据不变，版本字段可识别 | 未开始 |
| P7-F03 | 网格生成 | `spirula-render`/`spirula-io` | P4-F06, P6-F01 | 定义网格生成输入和参数；处理空结果、内存上限和取消；保存中间错误与进度 | 生成过程、空结果和资源限制有测试与错误信息 | 未开始 |
| P7-F04 | 网格导入导出 | `spirula-io` | P7-F03, P1-F05 | 实现网格格式读写；写入版本、校验和元数据；用 reference viewer 做 round-trip 和原子保存验证 | 格式版本化、校验和原子保存；可被 reference 工具读取 | 未开始 |
| P7-F05 | 图像与视频导出 | `spirula-io`/application | P7-F04, P6-F01 | 实现图像帧/视频导出；报告进度并支持取消/重试；清理部分文件并验证可播放结果 | 导出进度、取消、重试和部分文件清理有测试 | 未开始 |
| P7-F06 | web viewer 资源 | `spirula-io`/`spirula-ui` | P7-F04 | 打包 viewer 资源和场景索引；固定 URL/文件布局；用现有 viewer 打开导出结果 | 生成的资源与现有 viewer 消费契约一致 | 未开始 |

Phase 7 退出条件：P7-F01 至 P7-F06 的编辑状态可重新打开，导出失败可见且
不会留下不可识别的半成品。

### Phase 8：辅助管线

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P8-F01 | SfM 管线适配 | `spirula-io`/application | P3-F03, P6-F01 | 探测 SfM 外部工具和版本；准备输入/输出目录；隔离集成测试成功、超时和失败 | 外部工具能力探测、输入输出和失败信息有隔离集成测试 | 未开始 |
| P8-F02 | 稠密重建管线 | `spirula-io`/application | P8-F01 | 定义稠密重建适配接口；检查模型/许可证/平台；用最小样本验证输出和资源限制 | 版本、模型和平台要求明确；最小样本可复现 | 未开始 |
| P8-F03 | 深度与法线管线 | `spirula-io`/application | P2-F06, P6-F01 | 加载深度/法线模型；检查设备和精度能力；验证输出尺寸、格式和不可用错误 | 模型加载、设备能力、输出格式和不可用结果可诊断 | 未开始 |
| P8-F04 | 分割与特征匹配 | `spirula-io`/application | P8-F03 | 接入分割和特征匹配模型；记录许可证和版本；测试空结果、重试和结果格式 | 许可证、模型版本、失败重试和结果格式有记录 | 未开始 |
| P8-F05 | 视频解码 | `spirula-io` | P3-F02, P6-F01 | 探测 codec 和平台解码能力；处理时间戳与方向；测试损坏视频、取消和帧顺序 | codec/平台能力显式检查；时间戳、方向和错误可测试 | 未开始 |
| P8-F06 | 模型管理 | `spirula-io`/application | P8-F01–P8-F05 | 实现模型下载和本地缓存；校验 hash/版本；支持取消、失败清理和迁移 | 模型下载、缓存、校验、取消和版本迁移语义明确 | 未开始 |

Phase 8 退出条件：P8-F01 至 P8-F06 各有隔离测试、能力矩阵和失败信息，
缺少外部模型或 codec 时不会被误报为已完成。

### Phase 9：跨平台、性能与发布

| 编号 | 功能点 | 归属 | 前置 | 细分范围 | 交付与验收 | 状态 |
|---|---|---|---|---|---|---|
| P9-F01 | 跨平台 adapter 验证 | `spirula-gpu`/CI | P2-F06 | 在三平台枚举 adapter；记录 backend、limits 和 feature；归档不可用设备原因 | macOS、Linux、Windows 的设备能力和不可用结果有记录 | 未开始 |
| P9-F02 | 正确性与性能基线 | workspace | P4-F06, P5-F07 | 固定渲染质量和训练收敛样本；测量内存/启动/checkpoint；保存命令、硬件和原始结果 | 固定渲染质量、训练收敛、内存、启动时间和 checkpoint 大小的测量命令与环境 | 未开始 |
| P9-F03 | profiling 与 GPU 缓存 | `spirula-gpu`/`spirula-render` | P9-F02 | 采集 profile 和热点；实现 shader/pipeline 缓存；用同一基线解释性能收益和回归 | 先有基线，再用 profile 证明 shader/pipeline 缓存和优化没有未解释回归 | 未开始 |
| P9-F04 | 打包与发布清单 | workspace/CI | P6-F02, P7-F05 | 定义构建矩阵和产物命名；从干净 checkout 打包；记录运行库、能力开关和校验值 | 从干净 checkout 生成可复现包，能力开关、版本和依赖清单完整 | 未开始 |
| P9-F05 | 崩溃报告与已知差异 | application/文档 | P6-F01, P9-F01 | 定义崩溃报告字段和隐私边界；记录平台差异和已知限制；编写回滚和问题复现步骤 | 崩溃信息、隐私边界、已知平台差异和回滚步骤有文档 | 未开始 |

Phase 9 退出条件：P9-F01 至 P9-F05 的发布清单可复现，性能结果有基线和
环境，能力开关与已知差异均已记录。

## 4. 首批功能顺序

首批只推进以下编号，完成一个后再进入下一个：

1. P0-F01：固定 C++ 参考基线；
2. P0-F02：建立子系统行为清单；
3. P0-F03：建立能力矩阵；
4. P0-F04：建立最小兼容 fixtures；
5. P0-F05：固化运行时契约；
6. P0-F06：建立验证入口；
7. P1-F01：领域错误与稳定 ID；
8. P1-F02：配置与版本化序列化；
9. P1-F03：取消与进度原语；
10. P1-F04：数学、相机与场景基础类型；
11. P1-F05：原子文件与 fixture harness；
12. P2-F06：headless GPU smoke test；
13. P3-F01/P3-F03：相机与数据集兼容；
14. P4-F01/P4-F02：splat 场景上传和渲染入口；
15. P5-F01/P5-F05：微型训练循环和 resume；
16. P6-F01/P6-F02：共享 session 与 CLI；
17. P6-F03/P6-F06：Slint 壳和共享会话事件；
18. P7-F01/P7-F04：编辑、网格和导出；
19. P8-F01/P8-F03/P8-F05：辅助管线；
20. P9-F01/P9-F02：跨平台和性能基线。

## 5. 单个功能点的完成标准

功能点只有在对应编号的适用条件全部满足后才能标记为 `已完成`：

- 已从独立 Git worktree 开发，并在该 worktree 内完成测试和 diff 检查；
- 明确归属 crate、公共 API、输入输出、前置功能和依赖方向；
- 记录 C++ 行为、格式兼容假设和迁移说明；
- 覆盖正常输入、非法输入、取消、部分失败和持久化恢复；
- GPU 功能有 CPU reference 或确定性 fixture，并记录数值容差；
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

当前仓库下一步是 P0-F05：固化运行时契约；完成后更新本文件中的状态、
当前执行点和相关能力矩阵，再开始 P0-F06。

# 开发切片与阶段门禁

本页是后续实现的约束和验收清单；功能编号、依赖和状态以
[project-plan.md](project-plan.md) 为准。2026-10-25 的
[OOOSplat / Brush 审查](research/ooosplat-brush-review.md) 形成这些调整。
新增要求尚未实现，不能因文档写入而标为完成。

## 1. 最小应用骨架与垂直切片

- P1-F07 建立 `spirula-application`，唯一持有应用状态、命令准入、取消、
  快照和订阅；使用 fake worker 验证生命周期，不依赖尚未实现的训练。
- P1-F09 建立最小 CLI 适配，只提供已接入的命令；后续由 P4-F06 接入真实
  预览，P5-F07 接入真实训练/恢复，P6-F02 完善全部 CLI 能力。
- P5-F01 只拥有训练 step/schedule 和工作状态，向应用 owner 报告结果；
  应用状态机不得在 CLI、Slint 或训练 worker 中各复制一份。
- 最小真实流程按“读取微型 COLMAP → 预览 → 一步更新 → 小规模训练 →
  保存 → 新进程恢复”推进；每段记录输入、输出、错误、取消和耗时。
- Phase 编号表示归属领域；跨 Phase 前置必须按依赖执行。例如 P6-F07
  显示路径验证在 Phase 2 后进行，完整 Slint 窗口仍在 P6-F03。

## 2. 配置兼容与可执行计划

P1-F02 的 251 字段读取/保存能力保留。它不证明对应算法已经可运行。
P5-F08 负责把兼容配置转换为版本化的 `TrainPlan`；P4-F01 对渲染请求
实行同样的能力校验。计划属于业务层，`spirula-core` 不读取设备或文件。

转换输入为兼容配置、数据集摘要和 GPU 能力值；输出为生效选项、调度、
资源预算、能力要求及稳定 digest。必须定义规范化序列化、默认值展开和
digest 算法/版本，并保存用户输入与 resolved plan 的关系。

- 数值范围、有限性、相机/SH/loss/优化器组合和资源需求在提交工作前校验。
- 有行为影响的选项必须已实现；不支持时使用 `SP-INP-004`，范围错误用
  `SP-INP-003`，设备不满足用 `SP-GPU-001`，不能静默忽略。
- 纯兼容保留字段与实际生效字段有明确分类和映射，未知扩展字段仍可保留。
- 降级必须显式确认策略、记录原因及新 digest；不能在恢复时悄悄换算法。
- P6-F03 仅呈现 capability registry 声明已实现的配置项。

## 3. 前向、反向与一步优化的证据链

P4-F01–P4-F06 固定参数布局、坐标、SH 约定、深度键、同深度顺序、裁剪、
alpha cutoff、合成/early-out 和输出色彩空间；输出中间 buffer 对照证据。
scan/radix 要验证空输入、重复键、非整块长度、溢出和稳定性。

可微路径单独拆分：

| 功能 | 必须明确的输入/输出与门禁 |
|---|---|
| P4-F07 | 训练前向的浮点输出、保存/重算策略、可见到全量索引、版本和资源所有权；与预览前向一致性及峰值字节数 |
| P4-F08 | raster/compositing VJP；像素梯度到投影参数梯度；多层透明度、遮挡、提前终止和 cutoff 边界 |
| P4-F09 | projection/covariance/SH VJP；归一化四元数、exp/log-scale、opacity 参数化及 SH viewdir→mean；覆盖每类可训练参数 |
| P4-F10 | 跨 tile 聚合、可见/全量梯度映射、refinement 统计；有限值检查、CAS/归约策略及高重叠压力测试 |
| P5-F02 | loss 导数、Adam moments/计数/LR schedule；与 CPU 微型样本比较一次完整参数更新 |
| P5-F03 | split/prune/relocation 同步重映射参数、Adam 和累计统计；资源预检失败不发布半更新 |

前向 golden 与导数验证分别记录。有限差分选择远离离散裁剪/排序切换的输入；
若采用平滑 cutoff 只用于导数测试，必须单独验证生产硬 cutoff，不能用平滑
测试声称生产阈值行为等价。CPU f64 数学容差和 GPU f32 容差分别制定。

每份数值报告记录参数化、eps、absolute/relative tolerance、非确定性来源、
设备和原始误差。原子浮点累加不默认跨设备逐 bit 一致。Brush 可作独立
对照，C++ Spirula 兼容结果仍以固定参考基线及明确差异为准。

## 4. GPU 边界与 Slint 显示路径

`wgpu` 公共类型只在 `spirula-gpu` 出现。先提供满足真实 slice 的窄类型
资源封装，不预先复制整套 wgpu API。P2-F03/P2-F04 用真实的
upload→compute→readback 验证接口，P4-F02 验证 render/train 共享资源。
资源带 context identity/generation；设备重建后旧 handle 必须失效。
覆盖跨 context 误用、in-flight drop、陈旧 handle、OOM 和 device lost。
如需改变公共类型边界，先同步架构和运行时契约并记录决策。

P2-F01 明确 required/optional feature 与 limits。subgroup、f16、timestamp
和浮点累加路径分别声明能力、回退或不可用诊断，不因使用 wgpu 就默认可用。

P6-F07（已完成，tip `82adae0eebbb1f1a0f20ddfec1d7dd824ae7006c`）在完整窗口
开发前做了最小 Slint 显示验证：固定 Slint 1.18 `renderer-software` 与 wgpu
30.0.1、adapter/backend、线程与纹理所有权；AD-P6-F07-01 允许
application→`spirula-gpu` 窄边生产 host 帧。本 tip 无共享纹理能力 → 显式
readback→host→Slint `Image` 上传；已记录每帧复制字节、同步点
（`submit`/`wait`/`map`/`set-image`）、produce→deliver 延迟；进程 CPU 总峰值
**未测**；GPU→display present / compositor 不可测。UI 只接收
`PresentedFrame`，不直接 GPU/IO。app software rendering notifier 在
`Unsupported` 时保持 unverified，与 CUA 人工可见软件绘制分别记录；
software paint ≠ wgpu surface present；不能宣称零复制；Phase 2 出口仍 open。
正式 UI / train/preview / P6-F03 未接入。契约
[phase-6/presentation-path.md](phase-6/presentation-path.md)。计划当前为
P3-F01。

P2-F08（2026-10-27 评审新增）是 Phase 2 出口唯一剩余条件的负责人。
P2-F06 交付的是 unsupported 门禁，不是 present 通过；不得因为 P2-F06
验收文字里的“最小 surface 测试”而回溯宣称 present 已验证。P2-F08 必须
提交：真实窗口 surface 配置、adapter surface 能力与 format/present mode
协商结果、present 的可观察证据（present 后读回或合成器可见性），以及
present 失败与 surface lost 的 typed error。若目标主机无法提供真实窗口，
按平台记录 unsupported 的具体命令、环境和原因；**该记录不等于 P2-F08
已完成，也不自动降低或关闭 Phase 2 出口**。software paint / Slint
`renderer-software` 永远不算 present。任何出口书面调整须另行审查，不得用
unsupported 偷换 present 验收，也不允许“功能点全标完成但出口永久 open”
且无人负责。

## 5. 资源预算、背压与逐阶段测量

P1-F08 建立结果格式和采样工具，后续功能提交各自样本，不等待 Phase 9。
每项记录代表性 fixture、参数规模、限制值、采样来源和分配失败行为。
显存查询不可用时，分开报告可核算的已分配字节与不可测的设备整体峰值。

| 功能 | 必须限制或测量的资源 |
|---|---|
| P1-F07/P1-F09 | 命令容量、事件/日志单条大小、通知队列、内存日志尾部、磁盘日志轮转；满队列时 Stop/Shutdown 响应 |
| P2-F03 | buffer/texture/staging 字节、在途提交、readback 和释放；设备限额与应用预算分别记录 |
| P3-F08 | 解码任务数、在途解码字节、packed cache、预取队列及 staging；队列容量不替代全局预算 |
| P4-F03/P4-F04/P4-F07 | splat-tile 交点数量、scan/radix scratch、保存的前向状态；大 footprint/高重叠样本 |
| P5-F02/P5-F03 | 参数、梯度、Adam、临时张量峰值；增长前预检、可解释拒绝和状态完整性 |
| P7-F07 | 编辑模型视图、分块输出与回读；压缩需要全模型内存时显式声明并预检 |
| P8-F07 | 子进程输出单行上限/累计尾部、进程树取消和下载/磁盘预算 |

事件投递的权威定义见 [runtime-contracts.md](phase-0/runtime-contracts.md)
第 3 节。GPU worker 不等待 UI；控制事件写有界可恢复历史，UI 用快照及
游标恢复。丢弃/合并允许源序号跳跃，必须报告计数；慢消费者不能导致
无限积压，无法重放时明确要求重新同步。

P9-F02 汇总同一基线的质量、time-to-quality、step 分布、阶段 CPU/GPU 时间、
readback/sync、CPU/GPU 峰值、启动、checkpoint 和取消延迟。固定源码/数据
hash、设备/driver/backend、分辨率、SH、loss、seed/采样、增长上限、warmup
及 cold/warm pipeline 编译。算法不同时分别做算子对照和端到端质量对照。
没有真实 GPU 证据的路径保留未验证状态。

## 6. 检查点完整恢复与发布

P3-F07 的读取报告必须区分“仅模型初始化”“可恢复训练”“不完整/损坏”。
模型 PLY 可用于重新初始化，但不能声称恢复了优化器或训练轨迹。

P5-F05 的完整状态至少包括：模型参数、Adam 一/二阶矩及计数、step、LR 和
其他 schedule、RNG 状态、sampler 顺序/游标、refinement 统计和触发位置、
数据集/划分标识、resolved plan digest、schema 与必需能力。
并发预取必须可排空/重建，或保存足够状态保证恢复采样顺序。

恢复先验证完整性、版本和配置/数据集匹配，再创建运行态；缺块或能力不符
返回稳定错误，保留原检查点。明确区分位级恢复、容差恢复和模型重启，记录
每种模式允许的差异；不承诺未经证明的跨 GPU 位级确定性。

多文件发布采用不可变 generation 目录，顺序为：

1. 在同文件系统的唯一临时 generation 写数据块并同步。
2. 写 manifest，包含版本、块长度/hash、step 和恢复前提，校验后同步。
3. 最后写 completion marker；同步目录，发布 generation。
4. 用 P1-F05 原子替换当前 generation 指针，再按平台同步父目录。

读取只接受完整且校验通过的 generation；更新指针前崩溃保留旧指针，更新后
发现损坏需明确失败并允许显式选择上个完整版本。不可把多个独立 rename
视作一个事务。保留 C++ 旧格式读取器；新 marker/manifest 属于 Rust 扩展，
兼容导出路径与不可表达的恢复状态必须单独说明。

P5-F05 注入数据块、manifest、marker、generation 发布、指针更新各边界的
失败；区分发布前失败和发布后 durability 失败。P5-F07 在新进程验证
`N+M` 对比 `N→保存→退出→加载→M`，比较参数、优化器、采样和指标，并测试
受控磁盘不足、损坏块、取消保存及旧版本读取。

## 7. 最小交付、外部工具与导出

P2-F07 从干净 checkout 构建最小 CLI 包，在没有源码/开发依赖的隔离目录
验证 help、能力诊断和微型 GPU smoke；缺少设备时记录精确跳过原因。
P4-F06/P5-F07 增加预览/训练样本，P9-F04 完成正式 Slint 安装包、签名和
全平台发布。可复现命令不等于未经验证的二进制 bit-for-bit 可复现。

P8-F07 统一后续外部工具安装/进程适配：固定源码 commit、不可变下载地址、
目标架构、archive/binary hash、实际 version/help、许可证及 provenance；
检查解压路径、动态依赖、并发安装锁、离线验证和失败回滚。取消终止进程树，
产物须在退出码和实际格式/完整性验证后发布，日志进度不代表导出成功。
声明工具能力与不可用原因；引入外部软件仍需满足现有运行时/技术栈约束，
涉及例外必须先记录架构决策。该功能不授权引入 OOOBrush 为运行时引擎。

P7-F07 用编辑模型视图统一选择/裁剪/变换，按块导出 PLY，独立回读比较位置、
尺度、SH、坐标与点数；不以 viewer 的补偿旋转替代格式契约。压缩格式按后续
独立范围扩展，保留资源需求、取消、失败清理及原文件不变的证据。

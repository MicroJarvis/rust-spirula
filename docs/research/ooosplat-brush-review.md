# OOOSplat / Brush 技术审查与计划调整

日期：2026-10-25。本文记录外部源码参考和已采纳的计划方向；不表示相应
Rust 功能已完成。实现状态以 [项目计划](../project-plan.md) 为准，执行门禁
见 [development-gates.md](../development-gates.md)。

## 固定参考与职责

| 仓库 | commit | 用途 |
|---|---|---|
| [OOOSplat](https://github.com/ooolabdev/ooosplat) | `6f8427a1a27ff97d690c619852c0c78b30f8a167` | 桌面工作流、任务服务、进程和引擎管理、预览/导出 |
| [OOOBrush](https://github.com/ooolabdev/OOOBrush) | `866a7d65b4d6a591ba0d52a88f8b20ea3465c1e5`，`ooo-v1.0.0` | OOOSplat 实际锁定的训练实现 |
| [Brush](https://github.com/ArthurBrussee/brush) | OOOBrush 继承基线 `6378a76add3b93501abb55c2dc08d71688537679` | 自定义 GPU 前向/反向、Burn/CubeCL、优化与数据加载的主要来源 |

OOOBrush 是 Brush 的源码 fork。该固定版本相对继承基线的两次提交主要
增加 CLI 训练/设备/导出诊断和三平台构建/发布；核心投影、光栅化、反向和
Adam 文件未在这部分差异中修改。算法来源应归于 Brush，不能全部算作
OOOSplat 或 OOOBrush 的独立实现。

## 实现观察

- OOOSplat 为 React/Tauri 应用，编排 FFmpeg/COLMAP/外部 Brush CLI，
  发布验证过的 PLY，再由 PlayCanvas 预览。应用预览与训练渲染是两条路径。
- Brush 使用 Burn 张量/自动微分基础设施和 Rust CubeCL 自定义内核，
  wgpu 负责后端；渲染 VJP 为手写，并有 scan/radix、原子累加和索引映射。
- 训练内核存在计数 readback、refinement CPU 采样和动态临时资源；
  Rust/wgpu 本身不能证明无同步、低显存或跨设备性能一致。
- OOOSplat 的任务身份、迟到事件过滤、进程树取消、固定引擎和产物校验
  值得参考。UI 通知有界不代表子进程日志累计与所有内存资源都有界。
- OOOBrush 的 PLY checkpoint 保存模型；完整 Adam/RNG/sampler 恢复
  尚不在该保存路径中。OOOSplat 复用已完成阶段与重新训练应分开理解。
- 有限差分套件覆盖多类参数，但采用平滑 cutoff；生产硬 cutoff 和外部
  前向 golden 分开验证。有测试文件不代表本次在真实 GPU 运行通过。

可复查入口均在上表 commit：OOOSplat 的 `engines/README.md`、
`src-tauri/src/engines/brush.rs`、`tasks/mod.rs`、`process/mod.rs`、
`pipeline/runner.rs`、`PreviewBackend.ts`；OOOBrush 的
`crates/brush-render/src/render.rs`、`bwd/`、`crates/brush-train/src/train.rs`、
`adam_scaled.rs`、`crates/brush-dataset/src/scene_loader.rs`、
`crates/brush-process/src/train_stream.rs` 和 `crates/brush-bench-test/tests/finite_diff.rs`。

## 采纳范围与技术决策

继续采用 Rust 2024 / Slint / wgpu / WGSL，自主实现渲染和训练。C++ Spirula
仍是行为兼容基线；Brush 为独立算法和测试参考。没有引入 Burn、CubeCL、
OOOBrush 运行时或任何新的 Python/C++/CUDA/直接 Vulkan 依赖。
复用源码时记录来源、版本和许可，不把参考样本自动当作 C++ 等价证据。

| 意见 | 计划落点 |
|---|---|
| 提前 headless session、CLI，逐段打通真实流程 | P1-F07/P1-F09、P4-F06、P5-F07；P6-F01 完善集成 |
| 独立拆分可微渲染与梯度验证 | P4-F07–P4-F10、P5-F02/P5-F03 |
| 资源预算与性能证据随功能建立 | P1-F08、P2-F03、P3-F08、P4-F03/P4-F04、P5-F03；P9-F02 汇总 |
| 配置兼容与可执行能力分层 | P4-F01、P5-F08、P6-F03 |
| 完整状态恢复及多文件发布 | P3-F07、P5-F05/P5-F07 |
| 验证窄 GPU 封装及 Slint 显示成本 | P2-F03/P2-F04、P4-F02、P6-F07 |
| 提前最小交付、外部工具管理和分块导出 | P2-F07、P8-F07、P7-F07 |

## 证据边界

初次审查时，rust-spirula 基线 `204e1f4` 的 `cargo test --workspace`
45 项通过；OOOSplat 的
`node --test scripts/brush-runtime.test.mjs scripts/colmap-runtime.test.mjs scripts/ffmpeg-release-policy.test.mjs`
30 项通过。这是历史审查记录，不替代当前提交的验证结果；后续 P1-F05
已加入更多测试。未构建/运行 OOOBrush GPU 测试、完整应用或 C++ 训练，
没有同设备性能/质量排名，不能据此宣称自主 WGSL 路径比 Brush 更快。

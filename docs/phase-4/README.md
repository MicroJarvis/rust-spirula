# Phase 4：渲染与可微内核

| 编号 | 功能点 | 契约 | 状态 |
|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | [render-scene-camera.md](render-scene-camera.md) | 已完成 |
| P4-F02 | splat 上传与资源绑定 | [splat-buffer.md](splat-buffer.md) | 已完成 |
| P4-F03 | 投影与 tile/bin 构建 | [projection-tiles.md](projection-tiles.md) | 已完成 |
| P4-F04 | 排序与光栅化 | [sort-raster.md](sort-raster.md) | 已完成 |
| P4-F05 | 合成与深度 | [compositing-depth.md](compositing-depth.md) | 当前 |
| P4-F06 | readback 与预览输出 | (not started) | 未开始 |
| P4-F07 | 训练前向状态与索引契约 | (not started) | 未开始 |
| P4-F08 | 光栅化与合成反向传播 | (not started) | 未开始 |
| P4-F09 | 投影与 SH 反向传播 | (not started) | 未开始 |
| P4-F10 | 梯度聚合与可微渲染验收 | (not started) | 未开始 |

P4-F01 只建立 host 侧 scene/camera/render request 和 CPU projection reference；
后续功能再接入 GPU resource、kernel、raster、readback 和训练前向。
唯一验证入口为 `cargo run -p spirula-xtask -- verify`。

P4-F03 已完成并合入 main：代码 tip `04e4ab42af78fd91191e2b883885c94e526ce962`，
合并提交 `4bfe999d32b0133b69eae8b20d7adfac8c45d26b`。契约
[projection-tiles.md](projection-tiles.md) 记录 3dgs/无畸变 pinhole 的 CPU f64
与真实 GPU projection/count/scan/emission、source-major 中间 buffer、
预算预检、C++ golden 对照与保守 AABB 的额外交点成本。未实现的 primitive、
畸变与 ellipse rejection 保持显式不可用；不声称已能生成预览图。

2026-10-31 独立审核通过；`SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p
spirula-xtask -- verify` 在 Apple M5 Max / Metal 上 **8/8 通过（22.5 s）**；
文档同步后复核 **8/8（20.6 s）**。render 测试 18/18，GPU retained-owner
生命周期回归 2/2。487500 交点样本的 accounted 峰值为 7804192 B，返回后
current bytes 与 retained submissions 为零。

P4-F04 已完成并合入 main：代码 tip `de76f79b502d8f4f8905f95f73051dcad7d3a6ab`，
合并提交 `e415f9e521674abb1203c27fb4568496ff7b939c`。契约
[sort-raster.md](sort-raster.md) 记录真实 GPU 16×4-bit 稳定 LSD radix、tile
ranges 与 conic/alpha 像素覆盖；TileBins v2 固定投影与光栅阈值关系。

2026-10-31 独立审核通过；`SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p
spirula-xtask -- verify` **8/8（23.2 s）**，render 测试 30/30，其中独立解析与
GPU 审核 4/4、排序/覆盖/失败路径 6/6。GPU F03→F04 的 800×600、65 sources、
487500 pairs / 3120 万次 raster tests 与 CPU 整条参考相符；radix scratch
16575104 B，accounted 峰值 32373472 B（上界 32375968 B），返回后 current
与 retained submissions 均为零。此为 buffers+staging 记账，非设备整体显存。
完成状态与上述文档同步后，同一 GPU 必需 verify 复核 **8/8（22.0 s）**。
当前功能为 P4-F05：合成与深度；预览接入仍属 P4-F06。

P4-F05 实现中（**未合入 / 未推进计划状态**）。契约
[compositing-depth.md](compositing-depth.md)：SH0=`max(C0·dc+0.5,0)`；
median 在 early-out 前、strict `next_T<0.5`、virtual z0；expected=
`depth_acc/max(1-T,1e-10)`（无 weight==0 清零）；公开 API 仅
`RenderRequest` 串联；prepared 私有；`GpuCompositor::compose_empty` 真实
WGSL 写清每个像素；颜色表 `with_capacity(N)` 精确容量。独立 review
`tests/p4_f05_review.rs` **10/10**（含背景预检、compose_empty OOM/timeout
retry、10000 culled P0、入口已知不可能 GPU 预算拒绝）；focused
`tests/p4_f05_compositing.rs` 含 800×600/65/487500 F03→F05 压力（含 alpha）、
`dispatch_compositing` 族准入，以及 `compose_empty` OOM/timeout
retain→wait→release。Metal/Apple M5 Max 实测 ledger 峰值
**46448480 B**（bound 46452544 B ≈ 44.30 MiB），返回后 current/retained/
pending 为 0。待主线程审核与 GPU verify 证据后再关闭。

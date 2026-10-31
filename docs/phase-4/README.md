# Phase 4：渲染与可微内核

| 编号 | 功能点 | 契约 | 状态 |
|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | [render-scene-camera.md](render-scene-camera.md) | 已完成 |
| P4-F02 | splat 上传与资源绑定 | [splat-buffer.md](splat-buffer.md) | 已完成 |
| P4-F03 | 投影与 tile/bin 构建 | [projection-tiles.md](projection-tiles.md) | 已完成 |
| P4-F04 | 排序与光栅化 | (not started) | 当前 |
| P4-F05 | 合成与深度 | (not started) | 未开始 |
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
current bytes 与 retained submissions 为零。下一功能为 P4-F04。

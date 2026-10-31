# Phase 4：渲染与可微内核

| 编号 | 功能点 | 契约 | 状态 |
|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | [render-scene-camera.md](render-scene-camera.md) | 已完成 |
| P4-F02 | splat 上传与资源绑定 | [splat-buffer.md](splat-buffer.md) | 已完成 |
| P4-F03 | 投影与 tile/bin 构建 | (not started) | 当前 |
| P4-F04 | 排序与光栅化 | (not started) | 未开始 |
| P4-F05 | 合成与深度 | (not started) | 未开始 |
| P4-F06 | readback 与预览输出 | (not started) | 未开始 |
| P4-F07 | 训练前向状态与索引契约 | (not started) | 未开始 |
| P4-F08 | 光栅化与合成反向传播 | (not started) | 未开始 |
| P4-F09 | 投影与 SH 反向传播 | (not started) | 未开始 |
| P4-F10 | 梯度聚合与可微渲染验收 | (not started) | 未开始 |

P4-F01 只建立 host 侧 scene/camera/render request 和 CPU projection reference；
后续功能再接入 GPU resource、kernel、raster、readback 和训练前向。
唯一验证入口为 `cargo run -p spirula-xtask -- verify`。

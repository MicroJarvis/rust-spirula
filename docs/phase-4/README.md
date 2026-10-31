# Phase 4：渲染与可微内核

| 编号 | 功能点 | 契约 | 状态 |
|---|---|---|---|
| P4-F01 | 渲染场景与相机接口 | [render-scene-camera.md](render-scene-camera.md) | 已完成 |
| P4-F02 | splat 上传与资源绑定 | [splat-buffer.md](splat-buffer.md) | 已完成 |
| P4-F03 | 投影与 tile/bin 构建 | [projection-tiles.md](projection-tiles.md) | 已完成 |
| P4-F04 | 排序与光栅化 | [sort-raster.md](sort-raster.md) | 已完成 |
| P4-F05 | 合成与深度 | [compositing-depth.md](compositing-depth.md) | 已完成 |
| P4-F06 | readback 与预览输出 | [preview-output.md](preview-output.md) | 已完成 |
| P4-F07 | 训练前向状态与索引契约 | (planned; implementation not started) | 当前 |
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
当前功能为 P4-F07：训练前向状态与索引契约（本轮未开始实现）。

P4-F05 已完成并合入 main：代码 tip `74c537e5d97c150979cf681e480de58777cbea8d`，
合并提交 `9ea81be6d95e28a0944964fcbc7c4bc65e814026`。契约
[compositing-depth.md](compositing-depth.md) 固定 SH0 下限 clamp、front-to-back
RGB/alpha/T、expected ray depth、early-out 前的 median log-T 插值，以及真实
GPU 空图/未覆盖像素写入。公开 `RenderRequest` 入口串联 F03→F04→F05，prepared
输入保持私有；已知 GPU 预算下界在投影分配前检查，颜色表精确容量与跨阶段
host 预算有回归证据。CLI/图像文件、训练状态和 VJP 仍属后续功能。

2026-10-31 独立审核通过；`SPIRULA_REQUIRE_RENDER_GPU=1 cargo run -p
spirula-xtask -- verify` **8/8（29.1 s）**。render 测试 **54/54**：16 unit、
3 reference、8 F03、4 F04 独立审核、6 F04、7 F05 focused、10 F05 独立审核。
独立解析 TSV 含 12 行 golden；17×9 全图末像素有非零贡献；还验证 10000
culled sources/P=0、已知不可能预算在 GPU 分配前拒绝、F05 自身 OOM/timeout
保留→等待→回收→成功重试，以及 family/layout/version/entry/range/alias/
cross-context/reopen 等拒绝路径。

800×600、65 sources、487500 pairs 的完整 GPU 链与 CPU f64 参考相比，样本
最大绝对误差 `5.00991022445163e-7`（含 alpha），验收容限 `2e-4`。实际
accounted 峰值 **46448480 B（44.30 MiB）**，上界 **46452544 B**；host
payload 上界 **89137168 B**。返回后 current/retained/pending 均为零。
一次 focused 测量：CPU 1.575464250 s，GPU 完整调用 570.072125 ms，F05
encode 84.343500 ms、submit/wait/map 2.943166 ms。Metal / Apple M5 Max，
新 context 首次合成调用，driver/pipeline cache 温度未知；这是 wall-clock
样本与 buffers/staging 记账，非正式性能保证、GPU timestamp、VRAM 或 RSS。
early-out 限制实际累积层数，F03/F04 仍处理全部交点。

完成状态、capability matrix 与唯一当前功能同步后，同一 GPU 必需 verify
复核 **8/8（24.5 s）**。

P4-F06 已完成并合入 main：代码 tip `b0b80452e0bac53ba252621050d71a148ca67ca1`，
合并提交 `234031884eac65ebaaa29f63c3b993cd5c0e536b`。契约 [preview-output.md](preview-output.md)
记录不透明 RGB8 PNG、request v1、真实 GPU facade、共享 Preview command、
原子 rename 提交点与 CLI 退出码、host/GPU 上界和实际 buffer-copy 请求量。
独立审核 render3/3、IO6/6、application3/3、CLI3/3、package2/2 通过；包括
真实 rename 后 SIGINT 保持 exit0、临时文件写入后取消保留旧文件并可重试、
已接受 GPU 提交 timeout→settle→current/pending/retained 归零→重试成功，
以及 BMP/IHDR-only 负例、篡改/符号链接/额外文件拒绝和 schema1 兼容。

2026-10-31 Apple M5 Max / Metal：GPU 必需 verify **8/8（80.1 s）**；Cursor
最终门禁 **8/8（107.1 s）**。800×600、65 sources、487500 pairs：最终 facade
open+compose+pack 574.543833 ms，RGB8 1440000 B、float+RGB8 reserve 28320000 B。
按已执行 buffer ranges 审核的 D2H staging 请求总量 34804160 B，mapped
host Vec 复制同量（非总线计数器）；实测 CLI 进程 wall1.084638958 s，PNG
2166591 B，完整解码像素与独立解析预期一致。具体阶段和限制见契约。

`cargo run -p spirula-xtask -- package --from-ref b0b80452e0bac53ba252621050d71a148ca67ca1 --profile dev
--out <fresh-dir>` 的干净 git archive 构建通过；同包 package-check 与 source-free
package-run 的 preview3×2/full RGB golden、Metal smoke `cpu_exact` 均通过，
smoke current=0。包内不依赖源码、Cargo、C++ 或 Python。

按用户要求，完成本功能后停止；计划唯一当前行移到 P4-F07，但不开展开发。
工作树与分支保留，未经确认不删除。

完成状态、capability matrix 与唯一当前行同步后，main 上同一 GPU 必需 verify
复核 **8/8（118.7 s）**。

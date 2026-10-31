# P3-F08 有界训练数据加载与缓存

P3-F08 的 owner 是 `spirula-io`，范围停留在 host 上的 train split RGB8 图像。它复用 P3-F03 的 `ParsedDataset` / `LoadedFrame` 和 P3-F02 的 `decode_rgb8`，不连接 `spirula-application`、`spirula-train`、Slint 或 `wgpu`。C++ 参考入口是 `data/DataManager.cpp` 的训练图像读取与采样行为；Rust 版本把资源边界和可恢复 sampler state 写成显式契约，Brush `scene_loader.rs` 只作队列设计的次级参考。

公开入口为 `BoundedDatasetLoader`、`BoundedLoaderConfig`、`SamplerState`、`LoadedRgbFrame` 和 `LoaderMetrics`。配置校验在创建 worker 前完成：cache、in-flight decode、prefetch queue 和 worker 数均必须大于零；每个 train frame 的 `width × height × 3` 先通过 checked arithmetic 计算，单帧超过 cache 或 in-flight cap 会返回 `SP-INP-003`。

loader 为每个实例创建 `worker_count` 个 owned decode worker 和 `prefetch_queue_cap` 的 `sync_channel`。任务入队前独立增加 `queue_count` 和 `in_flight_bytes`，worker 取走任务后减少 queue count，第三方 `image` 解码只在调用前后检查取消；完成或失败都释放 in-flight reservation。成功的 packed RGB8 payload 进入独立的 byte-capped LRU，淘汰从最旧条目开始。`LoaderMetrics` 分开报告 `cache_bytes`、`in_flight_bytes`、`queue_count`、`decode_worker_count`、cache entries 和 hit/miss，队列容量不代替全局字节预算。

`SamplerState` 保存版本、seed、epoch、cursor 和当前有序 frame indices，可通过 JSON 重建。相同 state 产生相同顺序；`next` 只有在 frame 成功取回后才推进 cursor，因此 decode、预算和取消错误不会留下半步状态。`drain_prefetch` 等待 queue/in-flight 账目归零，`rebuild_prefetch` 在排空后按 state 重新提交。该契约提供 host 侧容差恢复语义，不承诺跨进程或跨设备 bit-level 训练恢复；完整 optimizer/RNG/checkpoint 恢复属于 P5-F05/P5-F07。

取消是合作式的：prefetch、队列等待、worker loop、drain 和 batch 获取均检查 `CancelToken`。第三方图像库调用中的 mid-decode 取消不可抢占，边界在调用前后。loader drop 设置 shutdown cancel、释放 sender、等待 owned workers，并清空 cache 与 pending/in-flight 账目；worker 失败返回稳定 typed error，修复源文件后可用 `retry_failed` 重试。

测试覆盖 deterministic sampler JSON round-trip、cache 命中/账目、单帧预算拒绝、取消和 tiny COLMAP 真实 PNG decode；完整 workspace 门禁和 `spirula-xtask verify` 结果记录在 Phase 3 README。训练 batch 的 application/session 接入、GPU staging、完整训练恢复和性能优化仍是后续功能范围。

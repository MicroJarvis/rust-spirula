# P4-F02：splat 上传与资源绑定

P4-F02 在 `spirula-render` 定义 `SplatGpuLayout`、`PackedSplatData` 和
`GpuSplatBuffer`，复用 `spirula-gpu` 已有的窄 `BufferHandle`、资源账目和
context generation。公共接口不暴露 `wgpu` 类型。

每个 degree-0 Gaussian 使用固定 64 字节 little-endian record，按 16 字节
lane 保存 mean、wxyz quaternion、log scale、opacity logit + DC RGB。布局
版本为 1，`STORAGE_COPY` buffer 作为后续 bind group 的资源边界；P4-F02 不
实现 projection、tile/bin、sort、raster、readback 或 UI。

`PackedSplatData::upload_peak_bytes` 以 resident buffer + upload staging 计算
峰值，`GpuSplatBuffer::upload` 在已有 resource budget 上做 checked preflight，
再调用 P2-F03 的 create/upload/submit。`wait_ready` 保留 submission 在超时
时重试；成功等待后 staging 释放，buffer drop 释放 resident accounting。跨
context 和旧 generation 通过 typed `SP-INP-001`/`SP-GPU-002` 语义拒绝。

headless tests 覆盖 64 字节布局、预算拒绝、f64→finite f32 溢出、提交/等待后
释放、跨 context 和 reopen generation 失效。完整门禁使用
`cargo run -p spirula-xtask -- verify`；P4-F03 继续负责投影和 tile/bin。

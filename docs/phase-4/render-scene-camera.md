# P4-F01：渲染场景与相机接口

`spirula-render` 暴露 GPU-independent 的 `RenderRequest`、
`RenderThresholds`、`RenderPrimitive`、`ColorSpace` 和 `CpuReference`。
请求携带 P3 的 OpenGL camera-to-world `PinholeCamera`、P3-F06 的有序
`SplatSet`、contract version、参数化和阈值；不携带文件、GPU handle 或
`wgpu` 类型。

当前可执行参数化固定为 degree-0 `3dgs` Gaussian 与 linear RGB。Gaussian
使用 `wxyz` unit quaternion、log scale、logit opacity 和 SH DC base color，
坐标采用 OpenGL camera-to-world：相机空间 `z < 0` 为前方，输出 depth 为
`-camera_z`。MIP/3DGUT、spherical-harmonic bands、sRGB/appearance chain
返回稳定 `SP-INP-004`，不会静默降级。

`RenderThresholds` 固定 alpha cutoff 和 early-out transmittance 的有限范围；
它们只进入后续 raster/compositing 契约。`CpuReference::project` 只把每个
splat mean 投影为 source index、pixel、positive depth、opacity 和 base color，
不执行 tile/bin、排序、光栅化、合成或 readback。背面/相机平面、非有限值、
非法阈值和未知 contract version 都是 typed errors。

证据位于 `crates/spirula-render/src/scene.rs` 的四个 headless tests：
中心投影和深度、非有限阈值、不支持 primitive/SH、背面拒绝和版本门禁。
完整 workspace 验收使用 `cargo run -p spirula-xtask -- verify`；GPU f32 容差、
上传资源、投影 kernel、排序/光栅、readback 和训练 forward 属于后续 P4 功能。

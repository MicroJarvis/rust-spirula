# 架构规则

## 依赖方向

```text
spirula-ui ───────┐
spirula-cli ──────┼──> application/session
                  │        ├──> spirula-train ──> spirula-render ──> spirula-gpu
                  │        ├──> spirula-io ──────> spirula-core
                  │        └──> spirula-core
```

后续需要时新增 `spirula-application`，集中维护 session、命令、取消、进度和
事件；UI 与 CLI 只负责适配入口。不得让 UI 直接编排训练、读写检查点或创建
GPU 管线。

## crate 职责

- `spirula-core`：领域类型、ID、数学、错误契约和纯算法；不依赖 UI/GPU/OS。
- `spirula-io`：数据集、图像、场景、检查点和导出格式；负责兼容与原子写入。
- `spirula-gpu`：`wgpu` instance/adapter/device/queue、资源、错误和 readback。
- `spirula-render`：渲染场景、相机、排序、光栅化、合成和 CPU 参考实现。
- `spirula-train`：训练生命周期、优化、评估、保存、恢复和进度事件。
- `spirula-ui`：Slint 界面、view model、输入校验和事件绑定。
- `spirula-cli`：命令解析和 application/session 入口，不复制业务逻辑。

## GPU 规则

GPU 功能按 kernel family 迁移。每个 GPU 算法都要有可测试的 CPU 参考路径、
输入输出契约、误差容限、资源标签和错误报告。优先使用 WGSL；需要平台能力
时通过 `wgpu` feature/limit 探测，并提供清晰的不可用结果。GPU 测试应优先
支持 headless 执行。

## 运行时规则

session 使用有界命令通道、类型化事件、合作式取消和可观察状态转换。检查点
与导出使用临时文件加原子替换；恢复只能从完整检查点开始，失败时报告原因。
CLI 和 Slint 必须调用同一 session 服务。

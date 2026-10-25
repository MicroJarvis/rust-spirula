# 架构规则

## 依赖方向

```text
spirula-ui ───────┐
spirula-cli ──────┼──> application/session
                  │        ├──> spirula-train ──> spirula-render ──> spirula-gpu
                  │        ├──> spirula-io ──────> spirula-core
                  │        └──> spirula-core
```

P1-F07 提前新增 `spirula-application`，集中维护唯一应用 session、命令准入、
取消、快照和事件；P1-F09 提供最小 CLI，P6-F01 完善集成。训练 worker 只维护
step 调度，UI 与 CLI 只负责适配入口。不得让 UI 直接编排训练、读写检查点或创建
GPU 管线。

## crate 职责

- `spirula-core`：领域类型、ID、数学、错误契约和纯算法；不依赖 UI/GPU/OS。
- `spirula-io`：数据集、图像、场景、检查点和导出格式；负责兼容与原子写入。
- `spirula-gpu`：`wgpu` instance/adapter/device/queue、资源、错误和 readback。
- `spirula-render`：渲染场景、相机、排序、光栅化、合成、训练前向状态、VJP 和 CPU 参考。
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

公共 `wgpu` 类型只在 `spirula-gpu` 出现；使用经真实计算切片验证的窄类型封装，
带 context/generation 校验。Slint 呈现经 application 接口，提前测量拷贝和同步。

细分的梯度、资源、背压、可执行配置和 checkpoint 门禁见
[../development-gates.md](../development-gates.md)。事件序号允许合并/丢弃后的
缺口，慢消费者通过快照/游标重同步；取消控制与 GPU worker 不等待 UI。

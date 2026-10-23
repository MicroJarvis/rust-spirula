# Agents.md — Spirula Studio Rust rewrite

这是本仓库的工作入口。默认先读本文件；执行具体任务时，按“按需文档索引”读取对应规则。

## 目标与边界

将 \`/Users/tfjiang/Projects/spirula-studio\` 逐步重构为 Rust 应用：

- UI 使用 Slint。
- GPU 使用 \`wgpu\` 与 WGSL。
- C++ 仓库只作为行为参考，不是依赖，也不在本仓库任务中修改。
- 先确定架构，再按可观察的垂直切片逐项迁移。
- 每个功能完成完整验证后，单独提交 Git。

当前仓库只有 Rust workspace 骨架，业务功能尚未迁移：

\`\`\`text
crates/
  spirula-core     领域契约与纯算法
  spirula-io       数据集、图像、检查点与导出
  spirula-gpu      wgpu 设备与资源边界
  spirula-render   Gaussian 渲染与预览
  spirula-train    训练编排与恢复
  spirula-ui       Slint UI 与 view model
  spirula-cli      spirula 进程入口
\`\`\`

## 硬性规则

1. 先架构后功能；不得绕过 [docs/architecture.md](docs/architecture.md) 的依赖方向。
2. 先记录 C++ 行为，再实现 Rust 行为；使用 fixture、golden output 或明确验收条件。
3. 每个功能必须是完整垂直切片：契约、实现、入口、测试、文档。
4. 提交前必须通过适用的完整检查，并保证干净 checkout 可复现。
5. 保持迁移可回滚；兼容读取器、旧 fixture 和迁移路径不得提前删除。
6. domain 层不依赖 UI、GPU、操作系统；CLI 与 Slint 共享同一 application/session 服务。
7. 不为新运行时路径引入 Python、C++、CUDA 或直接 Vulkan 依赖。
8. 初始迁移阶段避免 unsafe；优化必须有 benchmark 或 profile 依据。
9. 部分失败、取消、重试、检查点和导出必须有明确且可测试的语义。

## 按需文档索引

| 任务 | 读取 |
|---|---|
| 完整阶段计划、功能顺序与验收 | [docs/project-plan.md](docs/project-plan.md) |
| crate 边界、session、GPU 约束 | [docs/architecture.md](docs/architecture.md)、[docs/agent/architecture-rules.md](docs/agent/architecture-rules.md) |
| 子系统迁移步骤与切片组织 | [docs/agent/migration-workflow.md](docs/agent/migration-workflow.md) |
| 功能完成标准与测试要求 | [docs/agent/feature-definition-of-done.md](docs/agent/feature-definition-of-done.md) |
| 验证、提交与提交内容边界 | [docs/agent/git-workflow.md](docs/agent/git-workflow.md) |
| 所有 agent 文档入口 | [docs/agent/README.md](docs/agent/README.md) |

## 当前执行点

下一步是 Phase 0：固定 C++ 参考版本，建立功能能力矩阵与最小 fixtures，
并确认依赖方向、session 状态机、错误/事件模型和 GPU 边界。具体清单见
[docs/project-plan.md](docs/project-plan.md)。

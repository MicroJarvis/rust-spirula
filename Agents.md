# Spirula Studio agent entry

这是仓库级入口。先读本页，再按任务从索引中选择需要的文档；不要为了
开始一个局部任务加载全部项目资料。

## 按需索引

| 任务 | 读取 |
|---|---|
| 了解目标、阶段顺序和当前执行点 | [docs/project-plan.md](docs/project-plan.md) |
| 选择代码模块、crate 边界或接口归属 | [docs/architecture.md](docs/architecture.md)、[docs/agent/architecture-rules.md](docs/agent/architecture-rules.md) |
| 迁移一个子系统或组织垂直切片 | [docs/agent/migration-workflow.md](docs/agent/migration-workflow.md) |
| 判断功能或性能是否可以完成 | [docs/agent/feature-definition-of-done.md](docs/agent/feature-definition-of-done.md) |
| 运行检查、整理提交或确认变更边界 | [docs/agent/git-workflow.md](docs/agent/git-workflow.md) |
| 查看跨任务的不可变约束 | [docs/agent/working-rules.md](docs/agent/working-rules.md) |
| 查看全部 agent 文档入口 | [docs/agent/README.md](docs/agent/README.md) |
| 执行 Phase 0：参考基线、行为清单、能力矩阵、fixtures 与运行时契约 | [docs/phase-0/README.md](docs/phase-0/README.md) |

## 技术栈约束

- 语言与构建：Rust 2024 workspace，使用 Cargo resolver 3。
- 桌面 UI：Slint，集中在 `spirula-ui`；UI 通过 application/session 服务访问业务。
- GPU：`wgpu` 管理设备、资源和管线，WGSL 编写 shader；不引入直接 Vulkan 或 CUDA 运行时路径。
- 领域、数据、渲染和训练：按 workspace crate 边界使用 Rust 实现，边界见架构文档。
- C++ 仓库只用于记录和核对参考行为，不作为 Rust 运行时依赖。
- 技术栈发生变化时，必须同步更新本节、README 和相关架构文档。

## 最小工作约定

- 先记录 C++ 参考行为，再实现 Rust 行为，并保留可核对的 fixture、golden output 或验收条件。
- 每个功能都要形成完整垂直切片：契约、实现、入口、测试和文档。
- 改动应可回滚；兼容读取器、旧 fixture 和迁移路径在确认替代方案前保留。
- 每次开发完成后必须同步更新相关文档、计划、fixture 或迁移记录；文档记录必须反映当前代码进展，不能把未完成内容标为已完成。
- 每个功能点必须使用独立的 Git worktree；只有在 worktree 内完成开发、充分测试、文档同步和 diff 检查后，才能提交并合并。

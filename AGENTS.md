# Spirula Studio agent 入口

这是仓库级入口。先读本页，再按任务从下表选择文档。不要为了开始一个局部任务而加载全部项目资料。

## 事实来源

- 功能编号、依赖和状态以 `docs/project-plan.md` 为准。当前功能是该文件中唯一状态为 `当前` 的行。判断进度时，不要依据 README 或阶段文档里的描述。
- 依赖版本以各 crate 的 `Cargo.toml` 和 workspace 依赖为准。本页只记录策略约束。

## 按需索引

| 任务 | 读取 |
|---|---|
| 了解目标、阶段顺序和当前执行点 | [docs/project-plan.md](docs/project-plan.md) |
| 选择代码模块、crate 边界或接口归属 | [docs/architecture.md](docs/architecture.md)、[docs/agent/architecture-rules.md](docs/agent/architecture-rules.md) |
| 迁移一个子系统或组织垂直切片 | [docs/agent/migration-workflow.md](docs/agent/migration-workflow.md) |
| 判断功能或性能是否可以完成 | [docs/agent/feature-definition-of-done.md](docs/agent/feature-definition-of-done.md) |
| 运行检查、整理提交或确认变更边界 | [docs/agent/git-workflow.md](docs/agent/git-workflow.md) |
| 查看跨任务的不可变约束 | [docs/agent/working-rules.md](docs/agent/working-rules.md) |
| 拆分训练/渲染功能、资源预算、恢复与早期交付 | [docs/development-gates.md](docs/development-gates.md) |
| 查看 OOOSplat / Brush 参考来源和采纳决策 | [docs/research/ooosplat-brush-review.md](docs/research/ooosplat-brush-review.md) |
| 查看全部 agent 文档入口 | [docs/agent/README.md](docs/agent/README.md) |
| 执行 Phase 0：参考基线、行为清单、能力矩阵、fixtures 与运行时契约 | [docs/phase-0/README.md](docs/phase-0/README.md) |

## 技术栈约束

- 语言与构建：Rust 2024 workspace，使用 Cargo resolver 3。
- 桌面 UI：Slint，集中在 `spirula-ui`。UI 只经 application 的 presentation 和 session 访问业务，不直接访问 GPU 或 IO。渲染后端的固定方式见 [docs/phase-6/presentation-path.md](docs/phase-6/presentation-path.md)。
- GPU：`wgpu` 限定在 `spirula-gpu` 内，shader 使用 WGSL。`wgpu*` 类型不得暴露给 UI 或 core。application 只允许 AD-P6-F07-01 规定的窄边。
- 不引入直接 Vulkan 或 CUDA 运行时路径。
- C++ 仓库只作为行为参考，不是 Rust 运行时依赖。
- 技术栈变化时，同步更新本节、根目录 README 和架构文档。

## 工作约定

- 先记录 C++ 参考行为，再实现 Rust 行为。保留可核对的 fixture、golden 输出或验收条件。
- 每个功能都是完整垂直切片：契约、实现、入口、测试和文档。
- 改动必须可回滚。兼容读取器、旧 fixture 和迁移路径在确认替代方案前保留。
- 只修改当前功能涉及的 crate 和文档，不顺手改其他 phase 的内容。
- 每个功能点使用独立 Git worktree。合并后清理该 worktree 和分支，删除前先征得用户确认。
- 不执行破坏性操作（如 `git reset --hard`、强制推送、删除数据），除非用户明确要求。

## 功能完成的收尾清单

完成一个功能时，在同一次提交中同步以下文档：

1. `docs/project-plan.md`：该功能状态改为 `已完成`，唯一的 `当前` 行移到下一个功能。
2. 所属 phase 的 README（如 `docs/phase-3/README.md`）：补充契约链接、代码 tip 和合并提交。
3. `docs/phase-0/capability-matrix.md`：更新受影响 C-* 行的状态和证据。
4. 根目录 `README.md`：更新当前功能描述。
5. 运行 `cargo run -p spirula-xtask -- verify`，把结果记入 phase 文档。
6. 若文档状态与代码或 verify 结果不一致，以代码和 verify 结果为准修正文档。

状态列只能使用 `当前`、`未开始`、`进行中`、`已完成`、`阻塞` 五个词。`READY`、`HOLD` 等审查结论只能写在证据描述里，不能作为状态值。

## 验证

- 本地唯一入口：`cargo run -p spirula-xtask -- verify`。它包含 fixture 检查、文档一致性检查、workspace 策略检查，以及 fmt、check、test、clippy。
- 完成门禁的具体条款见 [docs/agent/feature-definition-of-done.md](docs/agent/feature-definition-of-done.md)。
- 提交前检查 `git diff` 和 worktree 状态。

## 开发准则

- 先说明假设和取舍。遇到歧义时先问，不要猜。
- 只写满足需求的最小实现，不加入无需求的功能或抽象。
- 外科式修改：只改必须改的内容，只清理自己产生的残留。
- 目标驱动：先定义可验证的成功标准，再循环执行直到验证通过。

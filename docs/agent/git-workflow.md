# Git 工作流

## 功能点的 worktree 流程

每个功能点都从独立 Git worktree 开始，避免未完成改动污染其他工作：

1. 从目标基线创建 worktree 和功能分支。
2. 在该 worktree 内完成契约、实现、测试、文档和 fixture 更新。
3. 在同一 worktree 内运行适用的完整检查，确认 `git diff --check`、文档同步和工作区状态。
4. 提交一个可回滚的结果；只有充分开发和测试通过后，才能将该分支合并回目标分支。
5. 合并后再删除 worktree；未完成或未通过门禁的 worktree 保留并标明状态。

每个提交只包含一个可说明、可回滚的结果。推荐使用 Conventional Commits：

```text
feat(core): add camera projection contracts
feat(gpu): add headless wgpu device smoke test
feat(data): parse colmap sparse datasets
feat(render): render fixture gaussian scene
feat(train): save and resume tiny training run
feat(ui): add dataset preview screen
```

提交前按功能适用范围执行：

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
git status
```

GPU、模型、平台编解码器等环境依赖要记录命令、系统、适配器或版本和结果。
提交前还要核对代码进展与文档记录：同步更新受影响的架构、项目计划、迁移
记录、fixture 说明或 README；如果功能尚未完成，文档必须保留进行中状态和
未完成项。检查 diff 后再提交。不得提交 `target/`、构建产物、本地数据集、
模型权重、凭据或机器专属绝对路径。文档与计划的结构调整可以合并为一个
`docs:` 提交。

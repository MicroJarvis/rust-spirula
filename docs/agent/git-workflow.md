# Git 工作流

每个提交只包含一个可说明、可回滚的结果。推荐使用 Conventional Commits：

\`\`\`text
feat(core): add camera projection contracts
feat(gpu): add headless wgpu device smoke test
feat(data): parse colmap sparse datasets
feat(render): render fixture gaussian scene
feat(train): save and resume tiny training run
feat(ui): add dataset preview screen
\`\`\`

提交前按功能适用范围执行：

\`\`\`text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
git status
\`\`\`

GPU、模型、平台编解码器等环境依赖要记录命令、系统、适配器或版本和结果。
检查 diff 后再提交。不得提交 \`target/\`、构建产物、本地数据集、模型权重、
凭据或机器专属绝对路径。文档与计划的结构调整可以合并为一个 \`docs:\` 提交。

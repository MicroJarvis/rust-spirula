# 功能完成标准

功能只有在适用条件全部满足后才能提交：

- owner crate、公开 API、依赖方向和兼容假设已明确；
- C++ 参考行为有文档、fixture、golden output 或验收条件；
- 覆盖正常输入、非法输入、取消、部分失败和持久化/恢复；
- 使用 GPU 时，CPU 参考与 GPU 结果在记录的容限内一致；
- 有 headless 测试；Slint 行为与 application 服务分开测试；
- 检查点和导出失败可见、重试安全、写入原子；
- 相关架构、计划和迁移文档已更新；
- 干净 checkout 可复现；
- 以下检查通过：

\`\`\`text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
\`\`\`

GPU、模型或 fixture 不可用时，必须记录精确命令、环境和原因；该功能不能
因此标记为已完成。

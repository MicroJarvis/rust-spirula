# 功能完成标准

功能只有在适用条件全部满足后才能提交：

- owner crate、公开 API、依赖方向和兼容假设已明确；
- C++ 参考行为有文档、fixture、golden output 或验收条件；
- 覆盖正常输入、非法输入、取消、部分失败和持久化/恢复；
- 使用 GPU 时，CPU 参考与 GPU 结果在记录的容限内一致；可微路径分别验证前向、VJP 和一步更新；
- 按 [开发门禁](../development-gates.md) 记录能力/回退、资源预算、峰值来源与阶段耗时；
- 配置进入执行前完成 capability/范围/资源校验，不把兼容读取成功当作功能可用；
- 有 headless 测试；Slint 行为与 application 服务分开测试；
- 检查点和导出失败可见、重试安全、写入原子；完整恢复包含优化器/采样/调度/增长状态，并有新进程对照；
- 任务/日志/事件测试慢消费者和满队列，取消仍响应；Slint 呈现有复制成本和释放证据；
- 涉及性能的功能先记录代表性基线、测量命令、环境和验收指标；优化后必须重复同一测量，结果达到约定指标且没有未解释的回归；
- 相关架构、计划和迁移文档已更新，且与本次代码、测试和 fixture 的实际状态一致；
- 干净 checkout 可复现；
- 以下检查通过：

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

GPU、模型、fixture 或性能测量环境不可用时，必须记录精确命令、环境和原因；
缺少必要证据的功能不能标记为已完成。

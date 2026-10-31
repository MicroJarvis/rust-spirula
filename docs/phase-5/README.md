# Phase 5：训练与恢复

| 编号 | 功能点 | 契约 | 状态 |
|---|---|---|---|
| P5-F01 | 训练 worker 与 step 调度 | (not started) | 未开始 |
| P5-F02 | loss 与优化器 | (not started) | 未开始 |
| P5-F03 | densification 与剪枝 | (not started) | 未开始 |
| P5-F04 | 评估与进度事件 | (not started) | 未开始 |
| P5-F05 | checkpoint 保存与 resume | (not started) | 未开始 |
| P5-F06 | stop、cancel 与部分失败 | (not started) | 未开始 |
| P5-F07 | 训练端到端验收 | (not started) | 未开始 |
| P5-F08 | 可执行 TrainPlan 与能力校验 | [train-plan.md](train-plan.md) | 已完成 |

P5-F08 的代码位于 `spirula-train`，只负责把兼容配置、数据集摘要和
`spirula-gpu` 的窄能力快照解析为可执行计划。它不启动 worker、不读取文件、
不创建 GPU 资源，也不把 `wgpu` 类型暴露给业务层。P5-F01 至 P5-F07
仍按 [docs/project-plan.md](../project-plan.md) 的依赖顺序推进。

验收命令：

```text
cargo run -p spirula-xtask -- verify
```

P5-F08 合并提交和 verify 结果在该功能契约及主线提交记录中维护。

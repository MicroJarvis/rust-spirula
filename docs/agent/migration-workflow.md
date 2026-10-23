# 迁移工作流

每个子系统先完成行为盘点，再进入实现。迁移顺序固定为：

1. **Inventory**：记录 C++ 入口、读写者、格式、测试、可观察输出和依赖。
2. **Contract**：定义 Rust 类型、不变量、错误、兼容窗口、fixture 和回滚路径。
3. **Expand**：在保留参考行为的前提下实现 Rust 路径。
4. **Verify**：比较结果、错误、持久化数据和必要的性能指标。
5. **Adopt**：验证通过后接入 application/session，再接入 CLI 或 Slint。
6. **Cleanup**：单独安排删除旧路径、兼容代码或临时适配层的任务。

每个垂直切片应同时包含：

- domain contract 与 owner crate；
- 实现和共享 application/session 入口；
- 正常、非法输入、取消、恢复或持久化测试；
- 需要时的 CPU/GPU 对照和 golden fixture；
- 用户入口、文档和迁移记录。

迁移优先顺序见 [../project-plan.md](../project-plan.md)：

核心契约与 fixture → wgpu smoke test → 数据集与相机 → 渲染 → 检查点 →
训练恢复 → CLI → Slint shell → 编辑/导出/mesh → 辅助管线。

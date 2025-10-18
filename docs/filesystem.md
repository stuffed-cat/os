# 文件系统服务说明

本文档介绍内核 `fs` 子系统的关键实现细节，涵盖基础的 ext2/3/4 镜像支持、写时覆盖 (overlay) 机制，以及新增的写入日志功能。

## 支持的磁盘格式

内核当前通过只读解析在内存中的 ext 系列镜像来提供基础文件系统能力：

- **ext2**：默认格式，未启用日志。超级块兼容位 `has_journal` 关闭且 inode 大小保持 128 字节时会被识别为 ext2。
- **ext3**：当超级块 `has_journal` 兼容位开启时被识别为 ext3。由于镜像仍以内存 snapshot 形式挂载，目前对日志文件本身不做重放，仅用于正确识别格式。
- **ext4**：若镜像开启 extents/huge_file/extra_isize 等特性，或 inode 尺寸大于 128 字节，会自动判定为 ext4。`fs::init_from_ramdisk` 会在串口输出检测到的具体格式，方便调试。

> ⚠️ 仍禁止 `64bit` 与 `flex_bg` 特性，因为它们需要额外的块组解析逻辑；加载此类镜像会返回 `FsError::Unsupported`。

## 写时覆盖 (Overlay)

所有写操作都通过 `FILE_OVERLAY` 维护的内存结构完成：

1. 读取时优先命中 overlay；若不存在则回退到原始 ext 镜像。
2. 写入会将文件内容复制到 overlay，并在内存中应用增量修改。
3. 删除、`chmod`、`chown` 等操作也只更新 overlay，确保底层镜像始终保持只读。

该设计允许我们在不破坏根镜像的前提下进行单元测试或用户态实验，又能在需要时清空 overlay 恢复初始状态。

## 写入日志

为了追踪 overlay 变更，内核引入了轻量级的内存日志：

- 辅助类型位于 `kernel::fs` 模块中：
  - `JournalOp`：记录操作类型（`Write`/`Create`/`Remove`/`Chmod`/`Chown`）。
  - `JournalInfo`：为不同操作附带额外信息，例如写入偏移与长度。
  - `JournalEntry`：日志条目本体，包含序号、路径和 `JournalInfo`。
- 所有增量操作都会调用内部的 `record_journal`，写入一个最多 256 条的环形缓冲。
- 通过公开函数 `fs::journal_snapshot()` 可以在调试或测试中获取日志副本。

示例：

```rust
use kernel::fs::{
  create_file_with_credentials, journal_snapshot, write_file_with_credentials, Credentials,
  JournalInfo, JournalOp,
};

let creds = Credentials::root();
create_file_with_credentials("/log.txt", &creds, 0o644)?;
write_file_with_credentials("/log.txt", &creds, 0, b"hello", true)?;

let entries = journal_snapshot();
assert!(entries.iter().any(|e| e.op == JournalOp::Create));
assert!(entries.iter().any(|e| matches!(e.info, JournalInfo::Write { len: 5, .. }))); 
```

日志缓冲维护先进先出的 256 条记录，超过容量时会自动丢弃最早的条目。这套机制可为调试 shell 写操作、单元测试或未来的持久化覆盖层提供可观测性。

## 后续扩展方向

- **ext4 进阶特性**：通过扩展块组描述符解析支持 `64bit`、`flex_bg`、`meta_bg` 等特性。
- **持久化日志**：将当前内存日志导出到串口或专用调试接口，以便 QEMU 会话外部分析。
- **回放机制**：结合写入日志与 overlay，为持久化存储或崩溃恢复提供基础。

如需贡献更多特性，建议先阅读 `kernel/src/fs.rs` 中的实现以及单元测试，了解现有的权限检查与 overlay 协议。

# ext2/3/4 特性 & 持久化改造评估

> 更新时间：2025-10-19

本文梳理当前 `kernel/src/fs.rs` 的实现限制，并给出支持 ext4 高级特性（`64bit`, `flex_bg`, `meta_bg` 等）及真正持久化的改造路线。

## 现状回顾

- **基础结构**：系统仍以 `init_from_ramdisk` 将 `rootfs.ext4` 映射为只读切片，解析 superblock、块组描述符与 inode 表。所有写操作通过内存 overlay (`FILE_OVERLAY`) 与简易 journal (`FILESYSTEM_JOURNAL`) 完成。
- **特性支持**：
  - 识别 ext4 superblock 标志，但对大部分高级特性仅做 *允许* 检查，并未真正处理。
  - inode 解析支持 extents (`EXT4_INODE_FLAG_EXTENTS`) 与 64bit 文件大小字段，但块指针仍以 32bit 处理。
  - 块组描述符只从“主拷贝”位置读取，未考虑 `flex_bg`/`meta_bg` 的重定位策略。
- **写入语义**：所有写入都落在 overlay，未回写到镜像；journal 仅用于调试观察，不具备崩溃恢复能力。

## 特性差距明细

| 特性 | 当前行为 | 目标行为 | 改造要点 |
| ---- | -------- | -------- | -------- |
| `64bit`（块号扩展） | 仅在 extents 中合成 48bit 块号；普通块指针使用 32bit | 支持 64bit block group/inode 计数、块映射、位图 | - 引入 `read_u64_low_high` 工具，处理描述符/位图高位字段<br/>- `collect_blocks_from_indirect` 需要读取 `i_block_hi` 扩展字段<br/>- inode/block group 引用统一使用 `u64` |
| `flex_bg`（块组聚合） | 读取块组描述符时按固定偏移计算 | 根据 `Flex` 大小跨块组共享位图/表 | - 从 superblock 第 36 字节 (`log_cluster_size`) + `s_log_groups_per_flex` 获取配置<br/>- 重新计算 inode/block 位图所在块；可能需要一次性缓存 flex 结构 |
| `meta_bg`（元块组） | 未处理；默认所有描述符在 superblock 之后 | 根据 `meta_bg` 拓扑定位备用描述符 | - 按 `s_first_meta_bg` 和 `s_blocks_per_group` 计算 meta block 起点<br/>- 在 `Ext2Fs::parse` 中为每个块组缓存 descriptor 指针（主+备）<br/>- 读取时优先主表，损坏时回退备份 |
| 稀疏超级块 (`sparse_super2`) | 未支持 | 仅保留必要副本 | - 解析 `s_backup_bgs` 列表
| 校验 (`metadata_csum`) | 根镜像未启用，但未来可能需要 | 验证并更新 CRC | - 引入 crc32c 算法，更新 superblock/描述符校验 |
| 持久化 | overlay 常驻 RAM | 修改应落地到磁盘镜像 | - 提供块级写 API，将 overlay 差异应用到 `&mut [u8]` 或后端块设备<br/>- 设计 WAL：在数据写前记录日志块，崩溃后回放 |

## 设计建议

### 1. 内核块设备抽象

目前 `Ext2Fs` 直接持有 `&'static [u8]`。若要持久化，需要可写后端：

- 抽象一个 `BlockDevice` trait（支持 `read_block`, `write_block`, `flush`）。
- Ramdisk 模式：在 `init_from_ramdisk` 时将镜像复制到 `Box<[u8]>` 并实现 `write_block` 为内存写；在 `shutdown`/`sync` 时经由 bootloader 回写到磁盘映像（需要 bootloader 支持）。
- QEMU/BIOS 磁盘模式：后续可以接入 ATA/virtio 块设备驱动，将写入直接落盘。

### 2. overlay → 日志化的 Copy-on-Write

短期内仍可保留 overlay，但需要：

1. **WAL 文件**：在磁盘上保留固定大小的 journal 区域（可使用 ext4 未分配块或额外分区）。
2. **提交流程**：
   - 写入时：将修改块写入 WAL，并在内存中更新 overlay。
   - `sync`/`umount` 时：按顺序将 WAL 内容应用到主数据区，更新相关位图、校验，清空 WAL。
3. **启动恢复**：读取 WAL 元信息，如果上次未完成提交，则回放。

> 在当前以 ramdisk 启动的环境下，可先实现“软持久化”：将 overlay 数据序列化为压缩日志存入镜像中的特殊文件（如 `/var/overlay.log`），启动时解析恢复。待块设备抽象就绪后再切换到真正的块级回写。

### 3. 代码结构调整

为避免单文件臃肿，建议拆分：

- `fs/superblock.rs`: superblock 解析、特性 flags
- `fs/group.rs`: 块组描述符、位图解析（处理 64bit/flex_bg/meta_bg）
- `fs/inode.rs`: inode 与 extent 处理
- `fs/overlay.rs`: 现有 overlay + journal（后续并入 WAL）
- `fs/persist.rs`: 持久化后端、WAL 管理

拆分后可以为每个模块增加单元测试，覆盖高位字段解析与多块组场景。

### 4. 测试策略

- **单元测试**：使用自制 ext4 镜像（启用 `64bit`, `flex_bg`）作为 fixture，通过 `tests` 目录下的 integration test 验证 `list_dir`/`read_file` 能正常工作。
- **工具链**：扩展 `xtask rootfs` 支持利用 `mke2fs` 生成不同特性的测试镜像；在 CI 中缓存生成的镜像。
- **回归测试**：添加持久化测试：
  1. 启动内核 -> 写文件 -> 调用 `sync`
  2. 模拟“重启”（重新解析镜像）
  3. 验证修改是否存在

### 5. 待办清单（按优先级）

1. 引入 `BlockDevice` 抽象及基于 `Box<[u8]>` 的内存实现。
2. 重写 `Ext2Fs::parse`，缓存块组描述符高位字段与 flex/meta 布局。
3. 更新 inode 块读取逻辑，统一使用 `u64` 块号。
4. overlay 序列化：先实现 `/.overlay.log` 软持久化，用 JSON/Bincode 记录文件增删改。
5. WAL & 块级提交（需要块设备写能力）。
6. 元数据校验 (`metadata_csum`) 与多副本策略。

---

本评估文件会随实现进度更新。建议在每个阶段完成后在 `docs/architecture.md` 或新的“文件系统设计文档”中记录最终方案。
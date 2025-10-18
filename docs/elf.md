# ELF64 加载器详解

> 适用于 `kernel::elf` 模块的实现概览，记录解析流程、校验规则与导出的元数据，便于后续扩展用户态加载能力。

## 解析目标

当前实现面向 x86-64 小端 ELF64 可执行文件（支持 `ET_EXEC` 与 `ET_DYN`），主要输出以下结构供进程子系统使用：

- `ExecutableImage`：封装入口地址、加载段数组、可选解释器路径、`PT_GNU_STACK` 栈权限以及 `PT_TLS` 的线程局部存储模板。
- `ExecutableSegment`：记录每个 `PT_LOAD` 段的虚拟地址、`p_memsz`/`p_filesz`、对齐约束与 payload 数据（已零扩展到内存长度）。
- `TlsTemplate`：当二进制包含 `PT_TLS` 时，保存初始数据、副本总大小与对齐，方便将来为每个线程分配 TLS 块。

## 校验流程

解析过程中会执行一系列健全性检查，以避免畸形或恶意 ELF 破坏内核状态：

1. **ELF Header 基础字段**
   - 校验魔数、`EI_CLASS`（仅接受 64 位）、`EI_DATA`（仅接受小端）。
   - `EI_VERSION` 必须为 `EV_CURRENT` (1)。
   - OS ABI 允许 System V (`EI_OSABI = 0`) 与 Linux (`EI_OSABI = 3`)；其他值会触发 `UnsupportedAbi`。
   - `e_type` 限制为 `ET_EXEC` / `ET_DYN`，`e_machine` 必须是 `EM_X86_64`。

2. **Program Header 表**
   - 验证 `e_phentsize == 56`（ELF64 Program Header 长度）。
   - 检查表长度乘法不会溢出，并确保整体落在文件范围内。

3. **段解析**
   - 仅 `PT_LOAD`、`PT_INTERP`、`PT_GNU_STACK`、`PT_TLS` 会被特殊处理，其他类型自动跳过。
   - `PT_LOAD`：
     - `p_memsz >= p_filesz`。
     - 偏移+长度不得越界，`p_align` 需为 2 的幂，且 `p_offset % p_align == p_vaddr % p_align`。
     - 段虚拟地址与末尾（`p_vaddr + p_memsz`）需处于用户态 canonical 空间（`< 0x0000_8000_0000_0000`）。
     - 生成 `ExecutableSegment`，payload 自动零填充到 `mem_size`。
   - `PT_INTERP`：提取以 `NUL` 结尾的字符串，填入 `ExecutableImage::interpreter`。
   - `PT_GNU_STACK`：转换 `p_flags` 为 `SegmentFlags`，影响栈的可执行属性。
   - `PT_TLS`：保留初始数据副本，零填充到 `p_memsz`，记录 `TlsTemplate`。

4. **段关系校验**
   - 所有 `PT_LOAD` 按虚拟地址排序，并检测是否重叠（发现重叠时返回 `SegmentOverlap`）。
   - 程序入口 (`e_entry`) 必须落在至少一个加载段范围内，否则返回 `EntryNotLoadable`（防止跳转到未映射地址）。

## 公开元数据

- `ExecutableImage::segments()`：`ExecutableSegment` 列表，含 `mem_size`、`file_size`、`align` 与 `SegmentFlags`。
- `ExecutableImage::stack_flags()`：若存在 `PT_GNU_STACK` 则反映其权限，否则默认为可读/可写/不可执行。
- `ExecutableImage::tls_template()`：返回 `Option<&TlsTemplate>`，供未来 TLS 初始化使用。
- `ExecutableSegment::data`：包含文件内容与补齐零字节，可直接写入目标地址空间。

## 与其他模块的协作

- `process::exec`：调用 `ExecutableImage::parse`，在失败时将不同 `ElfError` 映射为可读的 `SubsystemError`；成功后将镜像交给 `AddressSpace`。
- `user::AddressSpace::from_executable`：
  - 将 `ExecutableSegment` 转换为 `SegmentMapping` 并排序。
  - 结合 `StackConfig` 与 `ExecutableImage::stack_flags()` 合并栈权限，确保遵守 ELF 的栈执行策略。
  - 后续版本可以读取 `tls_template()` 为线程分配 TLS 数据块。

## 测试覆盖

`kernel::elf` 自带单元测试涵盖：

- 解释器路径提取。
- 对齐校验失败 (`BadSegmentAlignment`)。
- `PT_GNU_STACK` 可执行栈标志解析。
- `PT_TLS` 模板生成与零扩展。
- 入口地址必须位于加载段内部。

这些测试与 `kernel/tests/integration.rs` 中的执行路径一同保证 ELF 加载路径的健壮性。

## 后续扩展建议

- 解析 `PT_GNU_RELRO` 以便在地址空间中记录只读数据段。
- 利用 `TlsTemplate` 在 `process`/`scheduler` 中构建 per-thread TLS blocks。
- 支持 `DT_DEBUG`、`PT_DYNAMIC` 等动态链接元信息，为用户态动态加载器打基础。

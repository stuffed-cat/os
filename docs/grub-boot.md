# 使用GRUB启动系统

## 问题背景

原有的自定义bootloader存在以下问题:
- 启动时无输出,难以调试
- FAT32支持不完整
- 维护复杂,稳定性差

## 解决方案

使用成熟稳定的GRUB2作为引导加载程序。

## 当前状态

**重要**: 现有内核使用 `bootloader_api`,这是rust-osdev/bootloader项目的专有协议,**不兼容**标准的Multiboot/Multiboot2协议。

要使用GRUB启动,需要以下两种方案之一:

### 方案1: 修改内核支持Multiboot2(推荐)

需要做以下修改:

1. **添加Multiboot2头** (在 `kernel-bin/src/boot/multiboot2.rs`)
   - 定义Multiboot2魔数和头结构
   - 在链接脚本中将头放到文件开头

2. **创建Multiboot2入口点**
   - 替换`bootloader_api::entry_point!`宏
   - 手动解析Multiboot2信息结构
   - 设置页表、堆分配器等

3. **更新链接脚本** (`kernel-bin/multiboot2.ld`)
   ```ld
   ENTRY(_start)
   SECTIONS {
       . = 1M;
       .multiboot2 ALIGN(8) : {
           KEEP(*(.multiboot2))
       }
       .boot : {
           KEEP(*(.boot))
       }
       /* ... 其他段 ... */
   }
   ```

4. **修改Cargo.toml**
   ```toml
   [profile.dev]
   panic = "abort"
   
   [package.metadata.cargo-xbuild]
   linker-script = "multiboot2.ld"
   ```

### 方案2: 继续使用原bootloader但修复问题

保持当前架构,专注修复:
1. Stage-2 FAT读取器的问题
2. 确保串口输出正常工作
3. 调试启动过程中的卡死点

## 构建GRUB镜像(假设内核已支持Multiboot2)

```bash
# 安装必要工具
sudo apt-get install grub-pc-bin parted

# 运行构建脚本
bash scripts/build-grub-image.sh

# 启动测试
qemu-system-x86_64 -drive format=raw,file=target/os-grub.img -m 512M -serial stdio
```

## 推荐做法

考虑到工作量和稳定性,建议:

1. **短期**: 修复现有bootloader的串口输出问题,至少能看到启动日志
2. **中期**: 实现Multiboot2支持,迁移到GRUB
3. **长期**: 考虑使用limine等现代bootloader

## 相关文件

- `scripts/build-grub-image.sh` - GRUB镜像构建脚本(已创建)
- `kernel-bin/src/boot/multiboot2.rs` - Multiboot2启动代码(待实现)
- `kernel-bin/multiboot2.ld` - Multiboot2链接脚本(已创建模板)

## 参考资源

- [Multiboot2规范](https://www.gnu.org/software/grub/manual/multiboot2/multiboot.html)
- [OSDev Wiki - Multiboot](https://wiki.osdev.org/Multiboot)
- [rust-osdev/multiboot2](https://github.com/rust-osdev/multiboot2) - Rust Multiboot2库

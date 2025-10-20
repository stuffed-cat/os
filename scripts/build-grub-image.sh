#!/bin/bash
set -e

echo "==> 检查GRUB BIOS支持..."
if ! dpkg -l | grep -q "grub-pc-bin"; then
    echo "错误: 未安装 grub-pc-bin"
    echo "请运行以下命令安装:"
    echo "  sudo apt-get install grub-pc-bin"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="$WORKSPACE_ROOT/target"
BUILD_DIR="$TARGET_DIR/grub-build"
IMG_FILE="$TARGET_DIR/os-grub.img"
IMG_SIZE_MB=100

# 清理
rm -rf "$BUILD_DIR"
rm -f "$IMG_FILE"

# 创建目录结构
mkdir -p "$BUILD_DIR/boot/grub"
mkdir -p "$BUILD_DIR/mnt"

echo "==> 重新生成rootfs..."
cd "$WORKSPACE_ROOT"
cargo run -p xtask -- rootfs || true

echo "==> 构建内核..."
cargo build -p kernel-bin --target x86_64-unknown-none --features boot

# 复制内核
KERNEL_BIN="$TARGET_DIR/x86_64-unknown-none/debug/kernel-bin"
if [ ! -f "$KERNEL_BIN" ]; then
    echo "错误: 内核不存在: $KERNEL_BIN"
    exit 1
fi
cp "$KERNEL_BIN" "$BUILD_DIR/boot/kernel.bin"

# 复制ramdisk  
RAMDISK="$WORKSPACE_ROOT/assets/rootfs.ext4"
if [ ! -f "$RAMDISK" ]; then
    echo "错误: ramdisk不存在: $RAMDISK"
    exit 1
fi
cp "$RAMDISK" "$BUILD_DIR/boot/initrd.img"

# 创建GRUB配置
cat > "$BUILD_DIR/boot/grub/grub.cfg" << 'EOF'
set timeout=3
set default=0

menuentry "Hanxi Cat OS" {
    multiboot /boot/kernel.bin
    module /boot/initrd.img
    boot
}
EOF

echo "==> 创建磁盘镜像..."
dd if=/dev/zero of="$IMG_FILE" bs=1M count=$IMG_SIZE_MB status=progress

echo "==> 创建分区..."
parted -s "$IMG_FILE" mklabel msdos
parted -s "$IMG_FILE" mkpart primary ext2 1MiB 100%
parted -s "$IMG_FILE" set 1 boot on

echo "==> 设置loop设备..."
LOOP_DEV=$(sudo losetup -f --show -P "$IMG_FILE")
echo "使用loop设备: $LOOP_DEV"

# 确保分区设备就绪
sleep 2
if [ ! -b "${LOOP_DEV}p1" ]; then
    echo "错误: 分区设备 ${LOOP_DEV}p1 不存在"
    sudo losetup -d "$LOOP_DEV"
    exit 1
fi

echo "==> 格式化分区..."
sudo mkfs.ext2 -F "${LOOP_DEV}p1"

echo "==> 挂载分区..."
sudo mount "${LOOP_DEV}p1" "$BUILD_DIR/mnt"

echo "==> 复制文件..."
sudo cp -r "$BUILD_DIR/boot" "$BUILD_DIR/mnt/"

echo "==> 安装GRUB..."
sudo grub-install --target=i386-pc --boot-directory="$BUILD_DIR/mnt/boot" --modules="biosdisk part_msdos ext2 multiboot" "$LOOP_DEV"

echo "==> 清理..."
sudo umount "$BUILD_DIR/mnt"
sudo losetup -d "$LOOP_DEV"

echo ""
echo "==> 完成! 启动镜像: $IMG_FILE"
echo ""
echo "启动命令:"
echo "  qemu-system-x86_64 -drive format=raw,file=$IMG_FILE -m 512M -serial stdio"

#!/bin/bash
# ============================================================
# nopeekOS – Install to NVMe/SSD
# ============================================================
#
# Installs nopeekOS to an NVMe SSD or any block device:
#   - GPT partition table
#   - EFI System Partition (512MB) with GRUB + kernel
#   - npkFS data partition (rest of disk, formatted on first boot)
#
# Usage:
#   sudo ./tools/install.sh /dev/nvme0n1
#
# Run from any Linux environment (live USB, your laptop, etc.)
# After install, set NVMe as first boot device in BIOS/UEFI.
#
# Prerequisites (Arch):
#   pacman -S grub efibootmgr dosfstools gdisk
# Prerequisites (Debian/Ubuntu):
#   apt install grub-efi-amd64-bin dosfstools gdisk

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'
YELLOW='\033[0;33m'; NC='\033[0m'

log()  { echo -e "${CYAN}[npk]${NC} $1"; }
ok()   { echo -e "${GREEN}[npk]${NC} $1"; }
warn() { echo -e "${YELLOW}[npk]${NC} $1"; }
err()  { echo -e "${RED}[npk]${NC} $1"; exit 1; }

# ============================================================
# Args & checks
# ============================================================

if [ $# -ne 1 ]; then
    echo "Usage: sudo $0 /dev/nvme0n1"
    echo ""
    echo "Installs nopeekOS (GRUB + kernel + npkFS partition) to a disk."
    echo "WARNING: All data on the target device will be erased!"
    exit 1
fi

DEVICE="$1"

[ "$(id -u)" -ne 0 ] && err "Must run as root (sudo)."
[ ! -b "$DEVICE" ] && err "'$DEVICE' is not a block device."

# Locate kernel
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
KERNEL_BIN="$PROJECT_DIR/target/x86_64-unknown-none/release/nopeekos-kernel"

[ ! -f "$KERNEL_BIN" ] && err "Kernel not found. Run './build.sh build' first."

# Check tools
for cmd in sgdisk mkfs.fat grub-install; do
    command -v "$cmd" &>/dev/null || err "Missing: $cmd"
done

# ============================================================
# Confirm
# ============================================================

DEVICE_SIZE=$(lsblk -bno SIZE "$DEVICE" 2>/dev/null | head -1)
DEVICE_SIZE_GB=$((DEVICE_SIZE / 1024 / 1024 / 1024))
KERNEL_SIZE=$(stat -c%s "$KERNEL_BIN" 2>/dev/null || echo 0)
KERNEL_SIZE_KB=$((KERNEL_SIZE / 1024))

echo ""
log "┌─────────────────────────────────────┐"
log "│  nopeekOS Installer                 │"
log "└─────────────────────────────────────┘"
echo ""
log "Target:  $DEVICE (${DEVICE_SIZE_GB} GB)"
log "Kernel:  $KERNEL_BIN (${KERNEL_SIZE_KB} KB)"
echo ""
log "Partition layout:"
log "  Part 1:  ESP       512 MB   (FAT32, GRUB + kernel)"
log "  Part 2:  npkFS     rest     (~$((DEVICE_SIZE_GB - 1)) GB, formatted on first boot)"
echo ""
warn "ALL DATA ON $DEVICE WILL BE ERASED!"
echo -n "Continue? [y/N] "
read -r answer
[[ "$answer" != "y" && "$answer" != "Y" ]] && err "Aborted."

# ============================================================
# Unmount
# ============================================================

log "Unmounting existing partitions..."
for part in "${DEVICE}"* "${DEVICE}p"*; do
    [ -b "$part" ] && umount "$part" 2>/dev/null || true
done

# ============================================================
# Partition: GPT
# ============================================================

log "Creating GPT partition table..."
sgdisk --zap-all "$DEVICE" >/dev/null 2>&1 || true
sgdisk \
    --new=1:0:+512M  --typecode=1:EF00 --change-name=1:"ESP" \
    --new=2:0:0      --typecode=2:8300 --change-name=2:"npkFS" \
    "$DEVICE" >/dev/null

partprobe "$DEVICE" 2>/dev/null || sleep 2

# Determine partition names (sdX1 vs sdXp1 vs nvme0n1p1)
find_part() {
    local n=$1
    for p in "${DEVICE}p${n}" "${DEVICE}${n}"; do
        [ -b "$p" ] && echo "$p" && return
    done
    sleep 2; partprobe "$DEVICE" 2>/dev/null || true
    for p in "${DEVICE}p${n}" "${DEVICE}${n}"; do
        [ -b "$p" ] && echo "$p" && return
    done
    err "Could not find partition ${n}."
}

ESP_PART=$(find_part 1)
DATA_PART=$(find_part 2)

log "ESP:   $ESP_PART"
log "npkFS: $DATA_PART"

# ============================================================
# Format ESP
# ============================================================

log "Formatting ESP as FAT32..."
mkfs.fat -F 32 -n NOPEEKOS "$ESP_PART" >/dev/null

# ============================================================
# Install GRUB + kernel
# ============================================================

MOUNT_DIR=$(mktemp -d)
mount "$ESP_PART" "$MOUNT_DIR"

log "Installing GRUB EFI bootloader..."
grub-install \
    --target=x86_64-efi \
    --efi-directory="$MOUNT_DIR" \
    --boot-directory="$MOUNT_DIR/boot" \
    --removable \
    --no-nvram \
    2>/dev/null

log "Copying kernel (${KERNEL_SIZE_KB} KB)..."
cp "$KERNEL_BIN" "$MOUNT_DIR/boot/kernel.bin"

log "Writing GRUB config..."
mkdir -p "$MOUNT_DIR/boot/grub"
cat > "$MOUNT_DIR/boot/grub/grub.cfg" << 'GRUBCFG'
set timeout=0
set default=0

insmod efi_gop
set gfxmode=1920x1080x32,1280x720x32,auto
set gfxpayload=keep

menuentry "nopeekOS" {
    multiboot2 /boot/kernel.bin
    boot
}
GRUBCFG

# ============================================================
# Write npkFS marker (so nopeekOS knows which partition to use)
# ============================================================

log "Writing npkFS partition marker..."
# Write a magic header so nopeekOS can identify the data partition.
# First 16 bytes: "npkFS-data-v1\0\0\0" — rest left as zeros for mkfs.
printf 'npkFS-data-v1\x00\x00\x00' | dd of="$DATA_PART" bs=16 count=1 conv=notrunc 2>/dev/null

# ============================================================
# Cleanup
# ============================================================

sync
umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"

echo ""
ok "═══════════════════════════════════════════════"
ok " nopeekOS installed to $DEVICE"
ok "═══════════════════════════════════════════════"
echo ""
log "Next steps:"
log "  1. Set '$DEVICE' as first boot device in BIOS/UEFI"
log "  2. Disable Secure Boot (GRUB is unsigned)"
log "  3. Boot — nopeekOS will format the npkFS partition"
log "  4. Choose a passphrase and you're in"
echo ""
log "To update the kernel later:"
log "  sudo mount ${ESP_PART} /mnt"
log "  sudo cp target/x86_64-unknown-none/release/nopeekos-kernel /mnt/boot/kernel.bin"
log "  sudo umount /mnt"
echo ""

#!/bin/bash
# ============================================================
# nopeekOS – Build & Run Script
# ============================================================
#
# Usage:
#   ./build.sh build        Kompiliert Kernel + erstellt ISO
#   ./build.sh qemu         Build + Run in QEMU (Serial auf stdio)
#   ./build.sh debug        Build + Run in QEMU mit GDB-Stub
#   ./build.sh vbox         Build + Run in VirtualBox
#   ./build.sh vbox-clean   VirtualBox VM entfernen
#   ./build.sh all          Build + QEMU + VirtualBox
#
# Ohne Argument: build + qemu

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="x86_64-unknown-none"
KERNEL_BIN="$PROJECT_DIR/target/$TARGET/release/nopeekos-kernel"
ISO_DIR="$PROJECT_DIR/target/iso"
ISO_FILE="$PROJECT_DIR/target/nopeekos.iso"
DISK_IMG="$PROJECT_DIR/target/disk.img"
VM_NAME="nopeekOS"

# Farben
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[0;33m'
NC='\033[0m'

log()  { echo -e "${CYAN}[npk]${NC} $1"; }
ok()   { echo -e "${GREEN}[npk]${NC} $1"; }
warn() { echo -e "${YELLOW}[npk]${NC} $1"; }
err()  { echo -e "${RED}[npk]${NC} $1"; }

# ============================================================
# Build
# ============================================================

build() {
    log "Building kernel..."

    cd "$PROJECT_DIR"

    # Rust bare-metal build (nightly features via rust-toolchain.toml)
    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        2>&1

    ok "Kernel built: $KERNEL_BIN"

    # ISO erstellen mit GRUB (bootbar in QEMU + VirtualBox)
    log "Creating bootable ISO..."

    mkdir -p "$ISO_DIR/boot/grub"
    cp "$KERNEL_BIN" "$ISO_DIR/boot/kernel.bin"

    cat > "$ISO_DIR/boot/grub/grub.cfg" << 'GRUBCFG'
set timeout=0
set default=0

menuentry "nopeekOS" {
    multiboot2 /boot/kernel.bin
    boot
}
GRUBCFG

    grub-mkrescue -o "$ISO_FILE" "$ISO_DIR" 2>/dev/null

    ok "ISO created: $ISO_FILE"

    # Create persistent disk image for virtio-blk (once)
    if [ ! -f "$DISK_IMG" ]; then
        log "Creating 16MB disk image..."
        dd if=/dev/zero of="$DISK_IMG" bs=1M count=16 2>/dev/null
        ok "Disk image: $DISK_IMG"
    fi

    echo ""
}

# ============================================================
# QEMU
# ============================================================

run_qemu() {
    if [ ! -f "$ISO_FILE" ]; then
        err "No ISO found. Run './build.sh build' first."
        exit 1
    fi

    log "Launching QEMU..."
    log "Serial console on stdio. Ctrl-A X to quit QEMU."
    echo ""

    qemu-system-x86_64 \
        -cdrom "$ISO_FILE" \
        -serial stdio \
        -display none \
        -m 128M \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -drive file="$DISK_IMG",format=raw,if=none,id=drive0 \
        -device virtio-blk-pci,drive=drive0 \
        -nic user,model=virtio-net-pci \
        -no-reboot \
        -no-shutdown
}

run_qemu_gui() {
    if [ ! -f "$ISO_FILE" ]; then
        err "No ISO found. Run './build.sh build' first."
        exit 1
    fi

    log "Launching QEMU with GUI + serial on stdio..."
    log "VGA window shows boot banner. Serial I/O in this terminal."
    log "Ctrl-A X to quit."
    echo ""

    qemu-system-x86_64 \
        -cdrom "$ISO_FILE" \
        -serial stdio \
        -m 128M \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -drive file="$DISK_IMG",format=raw,if=none,id=drive0 \
        -device virtio-blk-pci,drive=drive0 \
        -nic user,model=virtio-net-pci \
        -no-reboot \
        -no-shutdown
}

run_debug() {
    if [ ! -f "$ISO_FILE" ]; then
        err "No ISO found. Run './build.sh build' first."
        exit 1
    fi

    log "Launching QEMU with GDB stub on :1234..."
    warn "Waiting for GDB connection. In another terminal:"
    warn "  gdb $KERNEL_BIN -ex 'target remote :1234'"
    echo ""

    qemu-system-x86_64 \
        -cdrom "$ISO_FILE" \
        -serial stdio \
        -display none \
        -m 128M \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -drive file="$DISK_IMG",format=raw,if=none,id=drive0 \
        -device virtio-blk-pci,drive=drive0 \
        -nic user,model=virtio-net-pci \
        -no-reboot \
        -no-shutdown \
        -s -S
}

# ============================================================
# VirtualBox
# ============================================================

run_vbox() {
    if [ ! -f "$ISO_FILE" ]; then
        err "No ISO found. Run './build.sh build' first."
        exit 1
    fi

    # Prüfen ob VBoxManage verfügbar
    if ! command -v VBoxManage &> /dev/null; then
        err "VBoxManage not found. Is VirtualBox installed?"
        exit 1
    fi

    # VM erstellen falls nötig
    if ! VBoxManage showvminfo "$VM_NAME" &> /dev/null; then
        log "Creating VirtualBox VM '$VM_NAME'..."

        VBoxManage createvm \
            --name "$VM_NAME" \
            --ostype "Other_64" \
            --register

        # Grundkonfiguration
        VBoxManage modifyvm "$VM_NAME" \
            --memory 128 \
            --cpus 1 \
            --vram 16 \
            --graphicscontroller vmsvga \
            --audio-enabled off \
            --usb off \
            --uart1 0x3F8 4 \
            --uartmode1 server "/tmp/nopeekos-serial.sock"

        # IDE Controller für ISO
        VBoxManage storagectl "$VM_NAME" \
            --name "IDE" \
            --add ide

        ok "VM '$VM_NAME' created."
    else
        log "VM '$VM_NAME' already exists, updating..."
    fi

    # Sicherstellen dass VM gestoppt ist
    VBoxManage controlvm "$VM_NAME" poweroff 2>/dev/null || true
    sleep 1

    # ISO einlegen (aktualisieren)
    VBoxManage storageattach "$VM_NAME" \
        --storagectl "IDE" \
        --port 0 \
        --device 0 \
        --type dvddrive \
        --medium "$ISO_FILE" \
        2>/dev/null || \
    VBoxManage storageattach "$VM_NAME" \
        --storagectl "IDE" \
        --port 0 \
        --device 0 \
        --type dvddrive \
        --medium "$ISO_FILE" \
        --forceunmount

    # Serial Port Pipe Setup
    # Auf Linux: socat oder minicom zum Verbinden
    VBoxManage modifyvm "$VM_NAME" \
        --uart1 0x3F8 4 \
        --uartmode1 server "/tmp/nopeekos-serial.sock"

    log "Starting VirtualBox VM..."
    warn "Serial console via: socat - UNIX-CONNECT:/tmp/nopeekos-serial.sock"
    warn "Oder im VirtualBox GUI-Fenster den VGA-Output sehen."
    echo ""

    # VM starten (GUI-Modus für visuelles Testing)
    VBoxManage startvm "$VM_NAME" --type gui

    ok "VM gestartet. VGA zeigt Boot-Banner."
    log "Für Serial Console in neuem Terminal:"
    log "  socat - UNIX-CONNECT:/tmp/nopeekos-serial.sock"
    echo ""
}

clean_vbox() {
    if ! command -v VBoxManage &> /dev/null; then
        err "VBoxManage not found."
        exit 1
    fi

    log "Stopping VM '$VM_NAME'..."
    VBoxManage controlvm "$VM_NAME" poweroff 2>/dev/null || true
    sleep 1

    log "Removing VM '$VM_NAME'..."
    VBoxManage unregistervm "$VM_NAME" --delete 2>/dev/null || true

    # Socket aufräumen
    rm -f /tmp/nopeekos-serial.sock

    ok "VM '$VM_NAME' removed."
}

# ============================================================
# Hilfsfunktionen
# ============================================================

check_deps() {
    local missing=()

    # Rust
    if ! command -v cargo &> /dev/null; then
        missing+=("cargo (rustup install)")
    fi

    # GRUB
    if ! command -v grub-mkrescue &> /dev/null; then
        missing+=("grub-mkrescue (apt install grub-pc-bin)")
    fi

    # xorriso
    if ! command -v xorriso &> /dev/null; then
        missing+=("xorriso (apt install xorriso)")
    fi

    if [ ${#missing[@]} -gt 0 ]; then
        err "Missing dependencies:"
        for dep in "${missing[@]}"; do
            err "  - $dep"
        done
        exit 1
    fi
}

usage() {
    echo "nopeekOS Build System"
    echo ""
    echo "Usage: ./build.sh [command]"
    echo ""
    echo "Commands:"
    echo "  build       Compile kernel + create bootable ISO"
    echo "  qemu        Build + run in QEMU (serial on stdio)"
    echo "  debug       Build + run in QEMU with GDB stub (:1234)"
    echo "  vbox        Build + run in VirtualBox (GUI)"
    echo "  vbox-clean  Remove VirtualBox VM"
    echo "  all         Build + show run options"
    echo "  help        Show this help"
    echo ""
    echo "Without argument: build + qemu"
}

# ============================================================
# Main
# ============================================================

case "${1:-}" in
    build)
        check_deps
        build
        ;;
    qemu)
        check_deps
        build
        run_qemu
        ;;
    qemu-gui|gui)
        check_deps
        build
        run_qemu_gui
        ;;
    debug)
        check_deps
        build
        run_debug
        ;;
    vbox)
        check_deps
        build
        run_vbox
        ;;
    vbox-clean)
        clean_vbox
        ;;
    all)
        check_deps
        build
        echo ""
        ok "Build complete. Run with:"
        log "  ./build.sh qemu       # QEMU (Entwicklung)"
        log "  ./build.sh debug      # QEMU + GDB"
        log "  ./build.sh vbox       # VirtualBox (Demo)"
        echo ""
        ;;
    help|-h|--help)
        usage
        ;;
    *)
        check_deps
        build
        run_qemu
        ;;
esac

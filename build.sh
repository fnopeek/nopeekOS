#!/bin/bash
# ============================================================
# nopeekOS – Build & Run Script
# ============================================================
#
# Usage:
#   ./build.sh build        Kompiliert Kernel + erstellt ISO
#   ./build.sh qemu         Build + Run in QEMU (Serial auf stdio)
#   ./build.sh debug        Build + Run in QEMU mit GDB-Stub
#   ./build.sh usb /dev/sdX  Build + EFI-bootbaren USB-Stick erstellen
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

INSTALL_DATA="$PROJECT_DIR/kernel/src/install_data"

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

insmod efi_gop
set gfxpayload=auto

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

# Two-pass build for USB installer kernel
build_installer() {
    log "Building installer kernel (two-pass)..."

    cd "$PROJECT_DIR"
    mkdir -p "$INSTALL_DATA"

    # Pass 1: normal kernel (without embedded install data)
    log "Pass 1: building base kernel..."
    # Create empty placeholder so include_bytes! doesn't fail
    [ -f "$INSTALL_DATA/kernel.bin" ] || touch "$INSTALL_DATA/kernel.bin"

    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        2>&1

    # Copy pass 1 kernel (this is what gets installed on NVMe)
    cp "$KERNEL_BIN" "$INSTALL_DATA/kernel.bin"
    ok "Pass 1: base kernel $(du -h "$INSTALL_DATA/kernel.bin" | cut -f1)"

    # Create GRUB EFI binary (if not exists or kernel is newer)
    if [ ! -f "$INSTALL_DATA/grub.efi" ] || [ "$INSTALL_DATA/grub.efi" -ot "$INSTALL_DATA/kernel.bin" ]; then
        log "Building GRUB EFI binary..."
        grub-mkimage \
            --format=x86_64-efi \
            --output="$INSTALL_DATA/grub.efi" \
            --prefix=/boot/grub \
            part_gpt fat multiboot2 efi_gop search search_fs_file normal boot
        ok "GRUB EFI: $(du -h "$INSTALL_DATA/grub.efi" | cut -f1)"
    fi

    # NVMe grub.cfg (already exists in install_data/)
    if [ ! -f "$INSTALL_DATA/grub.cfg" ]; then
        cat > "$INSTALL_DATA/grub.cfg" << 'GRUBCFG'
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
    fi

    # Pass 2: installer kernel (with embedded GRUB + kernel + config)
    log "Pass 2: building installer kernel..."
    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        --features installer \
        2>&1

    ok "Pass 2: installer kernel $(du -h "$KERNEL_BIN" | cut -f1)"
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
        -nic user,model=virtio-net-pci,hostfwd=tcp::4444-:4444,hostfwd=tcp::4445-:4445 \
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
        -nic user,model=virtio-net-pci,hostfwd=tcp::4444-:4444,hostfwd=tcp::4445-:4445 \
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
        -nic user,model=virtio-net-pci,hostfwd=tcp::4444-:4444,hostfwd=tcp::4445-:4445 \
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

# ============================================================
# USB Stick
# ============================================================

write_usb() {
    local device="${1:-}"
    if [ -z "$device" ]; then
        err "Usage: ./build.sh usb /dev/sdX"
        err ""
        err "List devices with: lsblk"
        exit 1
    fi

    [ ! -b "$device" ] && { err "'$device' is not a block device."; exit 1; }
    [ ! -f "$KERNEL_BIN" ] && { err "Kernel not found. Run build first."; exit 1; }

    # Safety: never write to NVMe
    if [[ "$device" == *"nvme"* ]]; then
        err "Refusing to write to NVMe device: $device"
        exit 1
    fi

    # Check tools
    for cmd in sgdisk mkfs.fat grub-install; do
        command -v "$cmd" &>/dev/null || { err "Missing: $cmd"; exit 1; }
    done

    local dev_size
    dev_size=$(lsblk -bno SIZE "$device" 2>/dev/null | head -1)
    local dev_size_gb=$((dev_size / 1024 / 1024 / 1024))

    warn "This will ERASE ALL DATA on $device (${dev_size_gb} GB)!"
    read -p "[npk] Continue? (yes/NO): " confirm
    [ "$confirm" != "yes" ] && { err "Aborted."; exit 1; }

    log "Unmounting existing partitions..."
    for part in "${device}"* "${device}p"*; do
        [ -b "$part" ] && sudo umount "$part" 2>/dev/null || true
    done

    log "Creating GPT partition table..."
    sudo sgdisk --zap-all "$device" >/dev/null 2>&1 || true
    sudo sgdisk \
        --new=1:0:+512M  --typecode=1:EF00 --change-name=1:"ESP" \
        --new=2:0:0      --typecode=2:8300 --change-name=2:"npkFS" \
        "$device" >/dev/null
    sudo partprobe "$device" 2>/dev/null || sleep 2

    # Find partition names (sdX1 vs sdXp1)
    local esp_part=""
    for p in "${device}p1" "${device}1"; do
        [ -b "$p" ] && esp_part="$p" && break
    done
    [ -z "$esp_part" ] && { sleep 2; sudo partprobe "$device" 2>/dev/null; }
    for p in "${device}p1" "${device}1"; do
        [ -b "$p" ] && esp_part="$p" && break
    done
    [ -z "$esp_part" ] && { err "Could not find ESP partition."; exit 1; }

    log "Formatting ESP as FAT32..."
    sudo mkfs.fat -F 32 -n NPKUSB "$esp_part" >/dev/null

    local mnt
    mnt=$(mktemp -d)
    sudo mount "$esp_part" "$mnt"

    log "Installing GRUB EFI bootloader..."
    sudo grub-install \
        --target=x86_64-efi \
        --efi-directory="$mnt" \
        --boot-directory="$mnt/boot" \
        --removable \
        --no-nvram \
        2>/dev/null

    log "Copying kernel..."
    sudo cp "$KERNEL_BIN" "$mnt/boot/kernel.bin"

    # Unique marker file so GRUB finds THIS partition, not NVMe
    sudo touch "$mnt/.npk-usb-boot"

    log "Writing GRUB config..."
    sudo mkdir -p "$mnt/boot/grub"
    sudo tee "$mnt/boot/grub/grub.cfg" > /dev/null << 'GRUBCFG'
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

    sync
    sudo umount "$mnt"
    rmdir "$mnt"

    ok "USB stick ready: $device"
    log "ESP: $esp_part (FAT32, GRUB EFI + kernel)"
    log "Plug into NUC, select USB in boot menu."
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
    echo "  usb /dev/sdX  Build + create EFI-bootable USB stick"
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
    usb)
        check_deps
        build_installer
        write_usb "${2:-}"
        ;;
    vbox-clean)
        clean_vbox
        ;;
    release)
        check_deps
        build
        log "Creating release artifacts..."
        RELEASE_DIR="$PROJECT_DIR/release"
        mkdir -p "$RELEASE_DIR"

        # Copy kernel binary
        cp "$KERNEL_BIN" "$RELEASE_DIR/kernel.bin"

        # Read version from Cargo.toml
        VERSION=$(grep '^version' "$PROJECT_DIR/kernel/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
        SIZE=$(stat -c%s "$RELEASE_DIR/kernel.bin")
        SHA384=$(openssl dgst -sha384 -hex "$RELEASE_DIR/kernel.bin" 2>/dev/null | awk '{print $NF}')

        # Write manifest
        cat > "$RELEASE_DIR/manifest" <<MANIFEST
version=$VERSION
size=$SIZE
sha384=$SHA384
MANIFEST

        ok "Manifest: v$VERSION, $SIZE bytes"
        log "SHA-384: $SHA384"

        # Sign with ECDSA P-384 if key exists
        KEY_FILE="$PROJECT_DIR/update.key"
        if [ -f "$KEY_FILE" ]; then
            openssl dgst -sha384 -sign "$KEY_FILE" -out "$RELEASE_DIR/kernel.sig" "$RELEASE_DIR/kernel.bin"
            ok "Signed with $KEY_FILE"
        else
            warn "No signing key found at $KEY_FILE"
            warn "Generate with: openssl ecparam -name secp384r1 -genkey -noout -out update.key"
            warn "Extract pubkey: openssl ec -in update.key -pubout -outform DER -out update.pub"
        fi

        # Sign WASM modules in release/modules/ (if any)
        if [ -d "$RELEASE_DIR/modules" ]; then
            log "Signing WASM modules..."
            MODULE_MANIFEST=""
            for wasm_file in "$RELEASE_DIR/modules/"*.wasm; do
                [ -f "$wasm_file" ] || continue
                MOD_NAME=$(basename "$wasm_file" .wasm)
                MOD_SIZE=$(stat -c%s "$wasm_file")
                MOD_SHA=$(openssl dgst -sha384 -hex "$wasm_file" 2>/dev/null | awk '{print $NF}')
                MOD_VER=$(head -1 "$RELEASE_DIR/modules/${MOD_NAME}.version" 2>/dev/null || echo "0.1.0")

                MODULE_MANIFEST="${MODULE_MANIFEST}[${MOD_NAME}]
version=${MOD_VER}
size=${MOD_SIZE}
sha384=${MOD_SHA}

"
                # Sign module
                if [ -f "$KEY_FILE" ]; then
                    openssl dgst -sha384 -sign "$KEY_FILE" -out "$RELEASE_DIR/modules/${MOD_NAME}.sig" "$wasm_file"
                    ok "Signed module: $MOD_NAME ($MOD_SIZE bytes)"
                fi
            done

            if [ -n "$MODULE_MANIFEST" ]; then
                echo "$MODULE_MANIFEST" > "$RELEASE_DIR/modules/manifest"
                ok "Module manifest written"
            fi
        fi

        ok "Release artifacts in $RELEASE_DIR/"
        ls -la "$RELEASE_DIR/"
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

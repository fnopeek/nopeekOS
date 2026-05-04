#!/bin/bash
# ============================================================
# nopeekOS – Build & Run Script
# ============================================================
#
# Usage: ./build.sh <command> [args]
#
# Common:
#   build                Compile kernel + create bootable ISO
#   qemu                 Build + run in QEMU (KVM, host CPU, serial)
#   qemu-gui             Build + run in QEMU with framebuffer GUI
#   debug                Build + run in QEMU with GDB stub on :1234
#
# Cross-vendor testing (TCG, slow but vendor-correct):
#   qemu-intel           Force Intel CPU emulation regardless of host
#   qemu-amd             Force AMD CPU emulation regardless of host
#
# Installer / release:
#   installer            Two-pass installer build (bundled assets)
#   qemu-installer       Installer + run (wipes disk, fresh install)
#   qemu-installer-gui   Same with framebuffer
#   usb /dev/sdX         Build installer + flash USB stick
#   release              Sign kernel + modules + assets (ECDSA P-384)
#
# Without argument: build + qemu

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="x86_64-unknown-none"
KERNEL_BIN="$PROJECT_DIR/target/$TARGET/release/nopeekos-kernel"
ISO_DIR="$PROJECT_DIR/target/iso"
ISO_FILE="$PROJECT_DIR/target/nopeekos.iso"
QEMU_ISO_DIR="$PROJECT_DIR/target/iso-qemu"
QEMU_ISO_FILE="$PROJECT_DIR/target/nopeekos-qemu.iso"
DISK_IMG="$PROJECT_DIR/target/disk.img"

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

build_microvm_initramfs() {
    # Build the Rust PID-1 + cpio.gz initramfs that the kernel
    # embeds via include_bytes! Skip silently if bsdtar is missing
    # so a partial environment can still build the kernel against the
    # last-committed initramfs.
    INIT_DIR="$PROJECT_DIR/microvm/linux/init"
    INIT_OUT="$PROJECT_DIR/release/assets/microvm-initramfs.cpio.gz"
    if [ ! -d "$INIT_DIR" ]; then return; fi
    if ! command -v bsdtar >/dev/null 2>&1; then
        warn "bsdtar missing — skipping microvm-init rebuild (using committed cpio.gz)"
        return
    fi
    log "Building microvm-init (Rust PID-1)..."
    (cd "$INIT_DIR" && cargo build --release 2>&1 | tail -3)
    INIT_BIN="$INIT_DIR/target/x86_64-unknown-linux-gnu/release/microvm-init"
    if [ ! -f "$INIT_BIN" ]; then
        warn "microvm-init build failed — using committed cpio.gz"
        return
    fi
    mkdir -p "$PROJECT_DIR/release/assets"
    INITRAMFS_TMP=$(mktemp -d)
    cp "$INIT_BIN" "$INITRAMFS_TMP/init"
    chmod +x "$INITRAMFS_TMP/init"
    (cd "$INITRAMFS_TMP" && bsdtar --format newc -cf - init | gzip -9 > "$INIT_OUT")
    rm -rf "$INITRAMFS_TMP"
    ok "microvm-initramfs.cpio.gz ($(stat -c%s "$INIT_OUT") bytes)"
}

build() {
    log "Building kernel..."

    cd "$PROJECT_DIR"

    # Refresh the embedded initramfs before cargo so include_bytes!
    # picks up any microvm/linux/init/ source change.
    build_microvm_initramfs

    # Rust bare-metal build (nightly features via rust-toolchain.toml)
    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        2>&1

    ok "Kernel built: $KERNEL_BIN"

    # ISO erstellen mit GRUB (bootbar in QEMU, USB, jede HW)
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

    ensure_disk_img

    echo ""
}

# Build a QEMU-specific ISO with fixed FullHD gfxmode. Bare-metal ISO keeps
# gfxpayload=auto so the native GPU driver owns resolution selection.
build_qemu_iso() {
    log "Creating QEMU ISO (FullHD)..."
    mkdir -p "$QEMU_ISO_DIR/boot/grub"
    cp "$KERNEL_BIN" "$QEMU_ISO_DIR/boot/kernel.bin"

    cat > "$QEMU_ISO_DIR/boot/grub/grub.cfg" << 'GRUBCFG'
set timeout=0
set default=0

insmod efi_gop
insmod all_video
set gfxmode=1920x1080x32,1280x720x32,auto
set gfxpayload=keep

menuentry "nopeekOS" {
    multiboot2 /boot/kernel.bin
    boot
}
GRUBCFG

    grub-mkrescue -o "$QEMU_ISO_FILE" "$QEMU_ISO_DIR" 2>/dev/null
    ok "QEMU ISO: $QEMU_ISO_FILE"
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

    # Pre-Pass 2: stage bundled assets (font + WASM modules) into
    # install_data/assets/ so the installer kernel's include_bytes!
    # calls find them. Paths MUST match BUNDLED_ASSETS in
    # kernel/src/install_data/assets/mod.rs — if you add a new asset
    # there, add the copy below too.
    log "Staging bundled assets for installer..."
    ASSETS_DIR="$INSTALL_DATA/assets"
    mkdir -p "$ASSETS_DIR"

    # Font: fetch from sys/fonts/ (the canonical source tree).
    if [ -f "$PROJECT_DIR/sys/fonts/inter-variable.ttf" ]; then
        cp "$PROJECT_DIR/sys/fonts/inter-variable.ttf" "$ASSETS_DIR/inter-variable.ttf"
        ok "  font: inter-variable.ttf ($(du -h "$ASSETS_DIR/inter-variable.ttf" | cut -f1))"
    else
        warn "  font missing: sys/fonts/inter-variable.ttf — installer will fail to compile"
    fi

    # Icon atlas: fetch from release/assets/ (rasterized by
    # tools/regen-icons from icons/phosphor/*.svg).
    if [ -f "$PROJECT_DIR/release/assets/phosphor.atlas" ]; then
        cp "$PROJECT_DIR/release/assets/phosphor.atlas" "$ASSETS_DIR/phosphor.atlas"
        ok "  icons: phosphor.atlas ($(du -h "$ASSETS_DIR/phosphor.atlas" | cut -f1))"
    else
        warn "  icon atlas missing: release/assets/phosphor.atlas — run tools/regen-icons first"
        touch "$ASSETS_DIR/phosphor.atlas"
    fi

    # Linux-virt bzImage: fetched manually once into release/assets/
    # (Alpine v3.23 main/x86_64 linux-virt-*-r*.apk → boot/vmlinuz-virt).
    # Phase 12.1.1c-3+ MicroVM substrate. Optional — installer compiles
    # without it; runtime `microvm linux` will refuse if missing.
    if [ -f "$PROJECT_DIR/release/assets/linux-virt.bzImage" ]; then
        cp "$PROJECT_DIR/release/assets/linux-virt.bzImage" "$ASSETS_DIR/linux-virt.bzImage"
        ok "  microvm: linux-virt.bzImage ($(du -h "$ASSETS_DIR/linux-virt.bzImage" | cut -f1))"
    else
        warn "  bzImage missing: release/assets/linux-virt.bzImage — microvm linux will be unavailable"
        touch "$ASSETS_DIR/linux-virt.bzImage"
    fi

    # MicroVM initramfs: built by `./build.sh release` from
    # microvm/linux/init/. Phase 12.1.3+. Optional — installer compiles
    # without it; runtime `microvm linux` falls back to no-initramfs.
    if [ -f "$PROJECT_DIR/release/assets/microvm-initramfs.cpio.gz" ]; then
        cp "$PROJECT_DIR/release/assets/microvm-initramfs.cpio.gz" "$ASSETS_DIR/microvm-initramfs.cpio.gz"
        ok "  microvm: initramfs ($(du -h "$ASSETS_DIR/microvm-initramfs.cpio.gz" | cut -f1))"
    else
        warn "  initramfs missing: release/assets/microvm-initramfs.cpio.gz — run ./build.sh release first"
        touch "$ASSETS_DIR/microvm-initramfs.cpio.gz"
    fi

    # WASM modules + their .version files: fetch from release/modules/
    # (produced by prior release build). Expect all four first-party
    # modules. The .version file is what lets `intent::install` and
    # `intent::update::update_all_modules` tell that a bundled module
    # is already up-to-date — without it they trigger redownloads.
    for mod in top debug wallpaper wifi drun loft testdisk; do
        WASM_SRC="$PROJECT_DIR/release/modules/${mod}.wasm"
        VER_SRC="$PROJECT_DIR/release/modules/${mod}.version"

        if [ -f "$WASM_SRC" ]; then
            cp "$WASM_SRC" "$ASSETS_DIR/${mod}.wasm"
            ok "  module: ${mod}.wasm ($(du -h "$ASSETS_DIR/${mod}.wasm" | cut -f1))"
        else
            warn "  module missing: release/modules/${mod}.wasm — run ./build.sh release first"
            touch "$ASSETS_DIR/${mod}.wasm"
        fi

        if [ -f "$VER_SRC" ]; then
            cp "$VER_SRC" "$ASSETS_DIR/${mod}.version"
        else
            warn "  version missing: release/modules/${mod}.version — defaulting to 0.0.0"
            echo "0.0.0" > "$ASSETS_DIR/${mod}.version"
        fi
    done

    # Pass 2: installer kernel (with embedded GRUB + kernel + config + assets)
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

# Persistent QEMU disk image (256 MB floor: installer alone plants
# ~14 MB of bundled assets — 12 MB Alpine bzImage + 1.6 MB modules
# + 879 KB font + icons — plus npkFS metadata + headroom for blobs).
# Created once on demand and kept between runs so npkFS state survives.
ensure_disk_img() {
    if [ ! -f "$DISK_IMG" ]; then
        log "Creating 256MB disk image..."
        dd if=/dev/zero of="$DISK_IMG" bs=1M count=256 2>/dev/null
        ok "Disk image: $DISK_IMG"
    fi
}

# qemu-installer modes wipe the disk first — the installer's job is
# to lay down a fresh npkFS, and a stale disk would just trigger the
# "already set up, log in" path on second boot.
wipe_disk_img() {
    if [ -f "$DISK_IMG" ]; then
        log "Wiping disk image for fresh install..."
        rm -f "$DISK_IMG"
    fi
    ensure_disk_img
}

# Single QEMU launcher. Args:
#   $1: display      "serial" | "gui"
#   $2: accel        "kvm" | "tcg-intel" | "tcg-amd"
#   $3..: extra qemu args (optional, e.g. -s -S for GDB stub)
#
# kvm uses -cpu host so the guest sees the host vendor's virt extensions
# (VMX on Intel, SVM on AMD). tcg-intel/tcg-amd force a specific vendor
# CPU model regardless of host — slow, but the only way to test the VMX
# backend on AMD or the SVM backend on Intel.
run_qemu_generic() {
    local display="$1"
    local accel="$2"
    shift 2

    ensure_disk_img
    build_qemu_iso

    local -a accel_args
    case "$accel" in
        kvm)
            accel_args=(-enable-kvm -overcommit cpu-pm=on -cpu host,+invtsc)
            ;;
        tcg-intel)
            accel_args=(-accel tcg -cpu Skylake-Server)
            warn "TCG mode (slow). Forcing Intel CPU emulation — VMX path."
            ;;
        tcg-amd)
            accel_args=(-accel tcg -cpu EPYC)
            warn "TCG mode (slow). Forcing AMD CPU emulation — SVM path."
            ;;
        *)
            err "Unknown accel mode: $accel"
            exit 1
            ;;
    esac

    local -a display_args
    if [ "$display" = "gui" ]; then
        display_args=(-vga std \
            -global driver=VGA,property=xres,value=1920 \
            -global driver=VGA,property=yres,value=1080)
        log "Launching QEMU GUI @ 1920x1080. Serial in this terminal. Ctrl-A X to quit."
    else
        display_args=(-display none)
        log "Launching QEMU. Serial on stdio. Ctrl-A X to quit."
    fi
    echo ""

    qemu-system-x86_64 \
        "${accel_args[@]}" \
        -cdrom "$QEMU_ISO_FILE" \
        -serial stdio \
        "${display_args[@]}" \
        -m 256M \
        -smp 4 \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -drive file="$DISK_IMG",format=raw,if=none,id=drive0 \
        -device nvme,drive=drive0,serial=nopeekos-test \
        -device qemu-xhci,id=xhci \
        -device usb-kbd,bus=xhci.0 \
        -device usb-mouse,bus=xhci.0 \
        -nic user,model=virtio-net-pci,hostfwd=tcp::4444-:4444,hostfwd=tcp::4445-:4445 \
        -no-reboot \
        -no-shutdown \
        "$@"
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

# ============================================================
# Hilfsfunktionen
# ============================================================

check_deps() {
    local missing=()

    if ! command -v cargo &> /dev/null; then
        missing+=("cargo (rustup install)")
    fi
    if ! command -v grub-mkrescue &> /dev/null; then
        missing+=("grub-mkrescue (apt install grub-pc-bin)")
    fi
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
    cat <<'EOF'
nopeekOS Build System

Usage: ./build.sh <command> [args]

Common:
  build                Compile kernel + create bootable ISO
  qemu                 Build + run in QEMU (KVM, host CPU, serial)
  qemu-gui             Build + run in QEMU with framebuffer GUI
  debug                Build + run in QEMU with GDB stub on :1234

Cross-vendor testing (TCG, slow but vendor-correct):
  qemu-intel           Force Intel CPU emulation regardless of host
  qemu-amd             Force AMD CPU emulation regardless of host

Installer / release:
  installer            Two-pass installer build (bundled assets)
  qemu-installer       Installer + run (wipes disk, fresh install)
  qemu-installer-gui   Same with framebuffer
  usb /dev/sdX         Build installer + flash USB stick
  release              Sign kernel + modules + assets (ECDSA P-384)

Without argument: build + qemu
EOF
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
        run_qemu_generic serial kvm
        ;;
    qemu-gui)
        check_deps
        build
        run_qemu_generic gui kvm
        ;;
    qemu-intel)
        check_deps
        build
        run_qemu_generic serial tcg-intel
        ;;
    qemu-amd)
        check_deps
        build
        run_qemu_generic serial tcg-amd
        ;;
    qemu-installer)
        # Build the installer-flavored kernel (with all bundled assets
        # baked in via include_bytes!) and boot it in QEMU on serial.
        # Wipes the disk first — the installer formats a fresh npkFS,
        # and a leftover disk from a previous session would trigger the
        # "already set up, log in" path instead.
        check_deps
        build_installer
        wipe_disk_img
        run_qemu_generic serial kvm
        ;;
    qemu-installer-gui)
        # Same as qemu-installer but with GUI framebuffer (1920x1080).
        check_deps
        build_installer
        wipe_disk_img
        run_qemu_generic gui kvm
        ;;
    debug)
        check_deps
        build
        log "Launching QEMU with GDB stub on :1234..."
        warn "In another terminal: gdb $KERNEL_BIN -ex 'target remote :1234'"
        run_qemu_generic serial kvm -s -S
        ;;
    installer)
        check_deps
        build_installer
        ;;
    usb)
        check_deps
        build_installer
        write_usb "${2:-}"
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

        # ── Sign system assets (fonts) ───────────────────────────────
        # Copy tracked fonts from sys/fonts/ into release/assets/ and
        # sign them the same way modules are signed. These ship with
        # the installer for fresh-install seeding (see install.rs
        # bundled_assets) and are OTA-updatable afterwards.
        mkdir -p "$RELEASE_DIR/assets"
        ASSET_MANIFEST=""
        if [ -d "$PROJECT_DIR/sys/fonts" ]; then
            for font_src in "$PROJECT_DIR/sys/fonts/"*.ttf; do
                [ -f "$font_src" ] || continue
                FONT_NAME=$(basename "$font_src")
                cp "$font_src" "$RELEASE_DIR/assets/$FONT_NAME"
                ASSET_SIZE=$(stat -c%s "$RELEASE_DIR/assets/$FONT_NAME")
                ASSET_SHA=$(openssl dgst -sha384 -hex "$RELEASE_DIR/assets/$FONT_NAME" 2>/dev/null | awk '{print $NF}')

                ASSET_MANIFEST="${ASSET_MANIFEST}[font:${FONT_NAME%.ttf}]
size=${ASSET_SIZE}
sha384=${ASSET_SHA}

"
                if [ -f "$KEY_FILE" ]; then
                    openssl dgst -sha384 -sign "$KEY_FILE" \
                        -out "$RELEASE_DIR/assets/${FONT_NAME}.sig" "$RELEASE_DIR/assets/$FONT_NAME"
                    ok "Signed font: $FONT_NAME ($ASSET_SIZE bytes)"
                fi
            done
        fi

        # Icon atlas — regenerated by tools/regen-icons from
        # icons/phosphor/*.svg, committed as release/assets/*.atlas.
        if [ -f "$RELEASE_DIR/assets/phosphor.atlas" ]; then
            ATLAS_SIZE=$(stat -c%s "$RELEASE_DIR/assets/phosphor.atlas")
            ATLAS_SHA=$(openssl dgst -sha384 -hex "$RELEASE_DIR/assets/phosphor.atlas" 2>/dev/null | awk '{print $NF}')

            ASSET_MANIFEST="${ASSET_MANIFEST}[icons:phosphor]
size=${ATLAS_SIZE}
sha384=${ATLAS_SHA}

"
            if [ -f "$KEY_FILE" ]; then
                openssl dgst -sha384 -sign "$KEY_FILE" \
                    -out "$RELEASE_DIR/assets/phosphor.atlas.sig" "$RELEASE_DIR/assets/phosphor.atlas"
                ok "Signed icons: phosphor.atlas ($ATLAS_SIZE bytes)"
            fi
        fi

        # MicroVM initramfs — already built by build_microvm_initramfs()
        # called from build(); just sign + manifest it here. Phase 12.1.3.
        INITRAMFS_FILE="$RELEASE_DIR/assets/microvm-initramfs.cpio.gz"
        if [ -f "$INITRAMFS_FILE" ]; then
            INITRAMFS_SIZE=$(stat -c%s "$INITRAMFS_FILE")
            INITRAMFS_SHA=$(openssl dgst -sha384 -hex "$INITRAMFS_FILE" 2>/dev/null | awk '{print $NF}')

            ASSET_MANIFEST="${ASSET_MANIFEST}[microvm:initramfs]
size=${INITRAMFS_SIZE}
sha384=${INITRAMFS_SHA}

"
            if [ -f "$KEY_FILE" ]; then
                openssl dgst -sha384 -sign "$KEY_FILE" \
                    -out "${INITRAMFS_FILE}.sig" "$INITRAMFS_FILE"
                ok "Signed initramfs: microvm-initramfs.cpio.gz ($INITRAMFS_SIZE bytes)"
            fi
        fi

        # Linux-virt bzImage — placed manually under release/assets/
        # by the maintainer (Alpine v3.23 main/x86_64 linux-virt apk →
        # boot/vmlinuz-virt). Phase 12.1.1c-3+ MicroVM payload.
        if [ -f "$RELEASE_DIR/assets/linux-virt.bzImage" ]; then
            BZIMG_SIZE=$(stat -c%s "$RELEASE_DIR/assets/linux-virt.bzImage")
            BZIMG_SHA=$(openssl dgst -sha384 -hex "$RELEASE_DIR/assets/linux-virt.bzImage" 2>/dev/null | awk '{print $NF}')

            ASSET_MANIFEST="${ASSET_MANIFEST}[microvm:linux-virt]
size=${BZIMG_SIZE}
sha384=${BZIMG_SHA}

"
            if [ -f "$KEY_FILE" ]; then
                openssl dgst -sha384 -sign "$KEY_FILE" \
                    -out "$RELEASE_DIR/assets/linux-virt.bzImage.sig" "$RELEASE_DIR/assets/linux-virt.bzImage"
                ok "Signed bzImage: linux-virt.bzImage ($BZIMG_SIZE bytes)"
            fi
        fi

        if [ -n "$ASSET_MANIFEST" ]; then
            echo "$ASSET_MANIFEST" > "$RELEASE_DIR/assets/manifest"
            ok "Asset manifest written"
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
    help|-h|--help)
        usage
        ;;
    *)
        check_deps
        build
        run_qemu_generic serial kvm
        ;;
esac

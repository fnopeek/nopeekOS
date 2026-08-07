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
#   boot / qemu-gui-boot Boot the EXISTING installed disk (NO rebuild, GUI)
#   qemu-boot            Same, serial only
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
#   usb /dev/sdX         Build self-contained installer + flash USB
#                        (bundles browser + all modules → offline-ready)
#   release              Sign kernel + modules + assets (ECDSA P-384)
#
# Without argument: build + qemu

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="x86_64-unknown-none"
KERNEL_BIN="$PROJECT_DIR/target/$TARGET/release/nopeekos-kernel"
KERNEL_EFI="$PROJECT_DIR/target/kernel.efi"
INSTALLER_DISK="$PROJECT_DIR/target/installer.img"
DISK_IMG="$PROJECT_DIR/target/disk.img"

# OVMF firmware (UEFI) — read-only CODE + writable per-VM VARS.
# Arch: /usr/share/edk2-ovmf/x64/  Debian/Ubuntu: /usr/share/OVMF/
OVMF_CODE_SRC="/usr/share/edk2-ovmf/x64/OVMF_CODE.4m.fd"
OVMF_VARS_SRC="/usr/share/edk2-ovmf/x64/OVMF_VARS.4m.fd"
[ -f "$OVMF_CODE_SRC" ] || OVMF_CODE_SRC="/usr/share/OVMF/OVMF_CODE_4M.fd"
[ -f "$OVMF_VARS_SRC" ] || OVMF_VARS_SRC="/usr/share/OVMF/OVMF_VARS_4M.fd"
OVMF_VARS_LOCAL="$PROJECT_DIR/target/OVMF_VARS.fd"

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

# PIE relocation guard. boot.s's self-relocation loop only applies
# R_X86_64_RELATIVE entries; a GOT/PLT/absolute dynamic reloc would be
# silently skipped → wrong pointer → boot crash on relocating firmware.
# Fail the build loudly here instead of debugging a black screen later.
assert_relative_only() {
    local elf="$1"
    local bad
    bad=$(readelf -rW "$elf" 2>/dev/null \
        | awk '$3 ~ /^R_X86_64/ && $3 != "R_X86_64_RELATIVE" {print $3}' \
        | sort -u)
    if [ -n "$bad" ]; then
        err "PIE reloc guard: non-RELATIVE dynamic relocations in $elf:"
        echo "$bad" >&2
        err "boot.s only applies R_X86_64_RELATIVE. Avoid the offending"
        err "construct (e.g. a static fn-ptr needing a GOT slot) or extend"
        err "the relocation loop in kernel/src/boot.s."
        exit 1
    fi
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

    # Convert ELF to PE32+ UEFI Application. UEFI firmware finds the
    # resulting kernel.efi as /EFI/BOOT/BOOTX64.EFI on the ESP and
    # runs it directly — no GRUB, no multiboot. See tools/pe-fixup.py
    # for why objcopy alone produces a PE that OVMF rejects.
    log "Converting to UEFI PE+ binary..."
    assert_relative_only "$KERNEL_BIN"
    objcopy -O pei-x86-64 --subsystem=efi-app --image-base=0x10000000 \
        "$KERNEL_BIN" "$KERNEL_EFI"
    python3 "$PROJECT_DIR/tools/pe-fixup.py" "$KERNEL_EFI" >/dev/null
    ok "UEFI binary: $KERNEL_EFI ($(du -h "$KERNEL_EFI" | cut -f1))"

    ensure_disk_img

    echo ""
}

# Build a minimal UEFI-bootable disk image with a FAT32 ESP that
# contains $KERNEL_EFI as /EFI/BOOT/BOOTX64.EFI. Used as the
# "installer USB stick" the installer-kernel boots from inside QEMU.
build_installer_disk() {
    log "Building installer boot disk..."
    rm -f "$INSTALLER_DISK"
    # Size the ESP to the actual kernel.efi + headroom. A plain installer
    # is ~3 MB, but the bundle-userspace (usb-full) build bakes the
    # ~261 MB LibreWolf sqfs in via include_bytes! → a ~272 MB .efi that
    # a fixed 96 MB image can't hold (mcopy → "Disk full"). +64 MB covers
    # GPT + FAT metadata with room to spare.
    local efi_mb=$(( ($(stat -c%s "$KERNEL_EFI") / 1024 / 1024) + 1 ))
    local disk_mb=$(( efi_mb + 64 ))
    [ "$disk_mb" -lt 96 ] && disk_mb=96
    log "  ESP size: ${disk_mb} MB (kernel.efi ${efi_mb} MB)"
    dd if=/dev/zero of="$INSTALLER_DISK" bs=1M count="$disk_mb" 2>/dev/null
    sgdisk --clear "$INSTALLER_DISK" >/dev/null
    sgdisk --new=1:2048:0 --typecode=1:EF00 --change-name=1:"ESP" "$INSTALLER_DISK" >/dev/null
    local part_offset=$((2048 * 512))
    mformat -i "$INSTALLER_DISK@@${part_offset}" -F -v NPKINSTALL
    mmd -i "$INSTALLER_DISK@@${part_offset}" ::/EFI ::/EFI/BOOT
    mcopy -i "$INSTALLER_DISK@@${part_offset}" "$KERNEL_EFI" ::/EFI/BOOT/BOOTX64.EFI
    ok "Installer disk: $INSTALLER_DISK ($(du -h "$INSTALLER_DISK" | cut -f1))"
    echo ""
}

# Two-pass build for USB installer kernel
build_installer() {
    log "Building installer kernel (two-pass)..."

    cd "$PROJECT_DIR"
    mkdir -p "$INSTALL_DATA"

    # Pass 1: regular kernel (no embedded data). Produces the kernel.efi
    # that the installer copies onto the NVMe ESP — that file is what
    # the installed system runs from on every subsequent UEFI boot.
    log "Pass 1: building base kernel..."
    # Placeholder so include_bytes!("install_data/kernel.efi") in
    # install.rs compiles before Pass 1 finishes.
    [ -f "$INSTALL_DATA/kernel.efi" ] || touch "$INSTALL_DATA/kernel.efi"

    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        2>&1

    # Pass 1: convert ELF to UEFI PE and stash for Pass 2's
    # include_bytes!. The installer kernel will plant this exact
    # binary at /EFI/BOOT/BOOTX64.EFI on the target ESP.
    assert_relative_only "$KERNEL_BIN"
    objcopy -O pei-x86-64 --subsystem=efi-app --image-base=0x10000000 \
        "$KERNEL_BIN" "$INSTALL_DATA/kernel.efi"
    python3 "$PROJECT_DIR/tools/pe-fixup.py" "$INSTALL_DATA/kernel.efi" >/dev/null
    ok "Pass 1: base kernel.efi $(du -h "$INSTALL_DATA/kernel.efi" | cut -f1)"

    # Pre-Pass 2: stage bundled assets (font + WASM modules) into
    # install_data/assets/ so the installer kernel's include_bytes!
    # calls find them. Paths MUST match BUNDLED_ASSETS in
    # kernel/src/install_data/assets/mod.rs — if you add a new asset
    # there, add the copy below too.
    log "Staging bundled assets for installer..."
    ASSETS_DIR="$INSTALL_DATA/assets"
    mkdir -p "$ASSETS_DIR"

    # Fonts + their licences: fetch from sys/fonts/ (canonical source
    # tree). Both faces are SIL OFL 1.1, which requires the licence text
    # to travel with every copy — so the .txt files are bundled too, not
    # just kept in the repo.
    for f in inter-variable.ttf ibm-plex-mono.ttf LICENSE-Inter.txt LICENSE-IBM-Plex.txt; do
        if [ -f "$PROJECT_DIR/sys/fonts/$f" ]; then
            cp "$PROJECT_DIR/sys/fonts/$f" "$ASSETS_DIR/$f"
            ok "  font: $f ($(du -h "$ASSETS_DIR/$f" | cut -f1))"
        else
            warn "  font missing: sys/fonts/$f — installer will fail to compile"
        fi
    done

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

    # MicroVM userspace bundle (LibreWolf, ~261 MB squashfs). Only
    # staged when BUNDLE_USERSPACE=1 → the `bundle-userspace` cargo
    # feature includes the file via include_bytes!. The resulting
    # installer kernel is ~260 MB larger but the USB is fully
    # self-contained: first boot has the browser without any OTA.
    # Without BUNDLE_USERSPACE the asset arrives via the next
    # `update` from GitHub Releases (manifest.large url= override).
    if [ "${BUNDLE_USERSPACE:-0}" = "1" ]; then
        SQFS_SRC="$PROJECT_DIR/release/assets/large/microvm-userspace.sqfs"
        if [ -f "$SQFS_SRC" ]; then
            cp "$SQFS_SRC" "$ASSETS_DIR/microvm-userspace.sqfs"
            ok "  microvm: userspace.sqfs ($(du -h "$ASSETS_DIR/microvm-userspace.sqfs" | cut -f1)) [BUNDLED]"
        else
            warn "  userspace.sqfs missing: $SQFS_SRC — run ./build.sh release-large first"
            warn "  BUNDLE_USERSPACE=1 was requested but the source is absent; falling back to OTA"
            export BUNDLE_USERSPACE=0
        fi
    fi

    # System wallpapers: every .png / .jpg in release/assets/wallpapers/
    # gets staged + bundled. Apps see them at sys/wallpapers/<file>;
    # setup.rs additionally copies them into the user's
    # home/<user>/pictures/wallpapers/ on first install. Adding a new
    # wallpaper = drop file + add a `BundledAsset` line in
    # install_data/assets/mod.rs (include_bytes! needs a literal name).
    # The BundledAsset list is GENERATED here — `include_bytes!` needs a
    # literal name, so a hand-written entry per file made "add a wallpaper"
    # a code change. Drop a file in release/assets/wallpapers/ and it ships.
    mkdir -p "$ASSETS_DIR/wallpapers"
    {
        echo "// GENERATED by build.sh — do not edit."
        echo "// One entry per file in release/assets/wallpapers/."
        echo "pub static WALLPAPERS: &[BundledAsset] = &["
    } > "$ASSETS_DIR/wallpapers.rs"
    if [ -d "$PROJECT_DIR/release/assets/wallpapers" ]; then
        for wp in "$PROJECT_DIR/release/assets/wallpapers/"*; do
            [ -f "$wp" ] || continue
            case "$wp" in *.sig) continue;; esac
            WP_NAME=$(basename "$wp")
            cp "$wp" "$ASSETS_DIR/wallpapers/$WP_NAME"
            {
                echo "    BundledAsset {"
                echo "        fs_path: \"sys/wallpapers/${WP_NAME}\","
                echo "        bytes:   include_bytes!(\"wallpapers/${WP_NAME}\"),"
                echo "        version: None,"
                echo "    },"
            } >> "$ASSETS_DIR/wallpapers.rs"
            ok "  wallpaper: $WP_NAME ($(du -h "$wp" | cut -f1))"
        done
    fi
    echo "];" >> "$ASSETS_DIR/wallpapers.rs"

    # WASM modules + their .version files: fetch from release/modules/
    # (produced by prior release build). Expect all four first-party
    # modules. The .version file is what lets `intent::install` and
    # `intent::update::update_all_modules` tell that a bundled module
    # is already up-to-date — without it they trigger redownloads.
    for mod in top debug wallpaper drun dock bar loft spell pick iris snap beak volume testdisk aml wifid wifi_ax200 audio_hda; do
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

    # Pass 2: installer kernel (embeds the Pass-1 kernel.efi + assets
    # via the `installer` Cargo feature → include_bytes!).
    # BUNDLE_USERSPACE=1 → also bakes in the 261 MB LibreWolf sqfs
    # via the `bundle-userspace` feature.
    log "Pass 2: building installer kernel..."
    PASS2_FEATURES="installer"
    if [ "${BUNDLE_USERSPACE:-0}" = "1" ]; then
        PASS2_FEATURES="installer,bundle-userspace"
        log "  with bundle-userspace (browser ready on first boot)"
    fi
    cargo build \
        --release \
        --target "$TARGET" \
        -Zbuild-std=core,alloc \
        -Zbuild-std-features=compiler-builtins-mem \
        --features "$PASS2_FEATURES" \
        2>&1

    ok "Pass 2: installer ELF $(du -h "$KERNEL_BIN" | cut -f1)"

    # The installer kernel itself is also UEFI-bootable — that's how
    # it runs from the USB stick / installer-disk image.
    log "Pass 2: converting installer to UEFI PE+..."
    assert_relative_only "$KERNEL_BIN"
    objcopy -O pei-x86-64 --subsystem=efi-app --image-base=0x10000000 \
        "$KERNEL_BIN" "$KERNEL_EFI"
    python3 "$PROJECT_DIR/tools/pe-fixup.py" "$KERNEL_EFI" >/dev/null
    ok "Installer .efi: $KERNEL_EFI ($(du -h "$KERNEL_EFI" | cut -f1))"
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
        # 16 GiB sparse — only host blocks for data actually written.
        # 1 GiB was too small once streaming downloads + the Mesa
        # (267 MB) / LibreWolf (~300-500 MB) userspace bundles landed,
        # and failed/aborted streaming writes leak orphan chunk blobs
        # until the next gc(), so headroom matters. Sparse keeps the
        # host cost == actual npkFS usage.
        log "Creating 16GB sparse disk image..."
        truncate -s 16G "$DISK_IMG"
        ok "Disk image: $DISK_IMG (16G sparse)"
    fi
}

# Guard for the boot-only targets: refuse to launch on a missing or empty
# (never-installed) disk — otherwise QEMU boots a blank 16G sparse image and
# just sits at the UEFI shell, which looks like a hang.
require_installed_disk() {
    if [ ! -f "$DISK_IMG" ]; then
        err "No disk image at $DISK_IMG — install first: ./build.sh qemu-installer-gui"
        exit 1
    fi
    # A freshly-truncated sparse image allocates ~0 blocks; an installed one
    # has the OS + npkFS written. `du` reports actual allocation, not the
    # 16G apparent size.
    local used_kb
    used_kb=$(du -k "$DISK_IMG" 2>/dev/null | cut -f1)
    if [ "${used_kb:-0}" -lt 1024 ]; then
        err "Disk image is empty (no OS installed) — run: ./build.sh qemu-installer-gui"
        exit 1
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
ensure_uefi_vars() {
    if [ ! -f "$OVMF_CODE_SRC" ]; then
        err "OVMF firmware not found. Install: pacman -S edk2-ovmf  (Arch)  /  apt install ovmf  (Debian)"
        exit 1
    fi
    if [ ! -f "$OVMF_VARS_LOCAL" ]; then
        log "Initializing UEFI NVRAM: $OVMF_VARS_LOCAL"
        cp "$OVMF_VARS_SRC" "$OVMF_VARS_LOCAL"
    fi
}

# run_qemu_generic <display> <accel> <mode> [extra qemu args...]
#   display:  serial | gui
#   accel:    kvm | tcg-intel | tcg-amd
#   mode:     normal     — OVMF + NVMe disk only (boots installed system)
#             installer  — OVMF + NVMe (wiped) + installer USB image
run_qemu_generic() {
    local display="$1"
    local accel="$2"
    local firmware="$3"
    shift 3

    ensure_disk_img

    local -a accel_args
    case "$accel" in
        kvm)
            # No cpu-pm=on: as of v0.186.0 worker APs arm their own 100 Hz
            # LAPIC timer (interrupts.rs arm_worker_timer) and idle with
            # plain HLT instead of MWAIT-on-the-WORK_AVAILABLE-cacheline.
            # cpu-pm=on used to be required because that MONITOR watch only
            # works with real-HW MWAIT passthrough — but passthrough HLT/
            # MWAIT never VMEXITs, so KVM never descheduled the idle vCPU
            # threads and ALL host cores sat at 100%/turbo while the guest
            # idled. Plain HLT VMEXITs → KVM blocks the idle thread → host
            # cores actually idle, and the per-core timer wakes the workers
            # so apps still start (no more native-ShadeBar fallback).
            accel_args=(-enable-kvm -cpu host,+invtsc)
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

    # The framebuffer is a fixed 1920x1080, but GTK defaults to zoom-to-fit:
    # it rescales that buffer to whatever size the window ends up with. Under
    # a tiling WM the window is never exactly 1920x1080, so every frame goes
    # through a resample and small text turns mushy — very visible in screen
    # recordings and screenshots. zoom-to-fit=off keeps one guest pixel on one
    # host pixel; show-menubar=off drops the GTK menu strip so the window IS
    # the framebuffer (nothing to crop out afterwards). Note this needs the
    # window to float, otherwise the tiler shrinks it and QEMU crops instead
    # of scaling. Override with e.g. QEMU_GTK=zoom-to-fit=on for the old
    # behaviour, or QEMU_GTK=full-screen=on for a fullscreen demo.
    local -a display_args
    if [ "$display" = "gui" ]; then
        display_args=(-vga std \
            -global driver=VGA,property=xres,value=1920 \
            -global driver=VGA,property=yres,value=1080 \
            -display "gtk,${QEMU_GTK:-show-menubar=off,zoom-to-fit=off}")
        log "Launching QEMU GUI @ 1920x1080 (1:1, unscaled). Serial in this terminal. Ctrl-A X to quit."
    else
        display_args=(-display none)
        log "Launching QEMU. Serial on stdio. Ctrl-A X to quit."
    fi

    # Installer mode forces fresh UEFI NVRAM so stale boot entries
    # (e.g. a "UEFI QEMU NVMe" pointing at the now-wiped disk) don't
    # block discovery of the new installer USB stick.
    if [ "$firmware" = "installer" ]; then
        log "Resetting UEFI NVRAM for fresh installer boot..."
        rm -f "$OVMF_VARS_LOCAL"
    fi
    ensure_uefi_vars

    local -a fw_args=(
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE_SRC"
        -drive if=pflash,format=raw,file="$OVMF_VARS_LOCAL"
    )

    # The installer drive is only attached on `installer` mode. With
    # a fresh NVRAM + wiped NVMe disk, OVMF auto-discovers any disk
    # whose ESP contains /EFI/BOOT/BOOTX64.EFI — the only such disk
    # is our installer USB, so it boots that.
    local -a installer_args=()
    if [ "$firmware" = "installer" ] && [ -f "$INSTALLER_DISK" ]; then
        installer_args=(
            -drive file="$INSTALLER_DISK",format=raw,if=none,id=instdisk
            -device usb-storage,bus=xhci.0,drive=instdisk
        )
    fi

    # Network backend. Default: slirp (-nic user) — easy, no host setup, but
    # single-threaded and caps ~300-340 Mbit. QEMU_NET=tap uses a vhost/tap
    # device (kernel-accelerated, near line-rate) to test our stack WITHOUT the
    # slirp ceiling. Requires a host tap device set up once (see message), e.g.:
    #   sudo ip tuntap add dev tap0 mode tap user "$USER"
    #   sudo ip addr add 172.30.0.1/24 dev tap0 && sudo ip link set tap0 up
    #   # + NAT to your uplink:  sudo iptables -t nat -A POSTROUTING -s 172.30.0.0/24 -j MASQUERADE
    #   # + sysctl net.ipv4.ip_forward=1
    # Override the device name with QEMU_TAP=tapX.
    local -a net_args
    if [ "${QEMU_NET:-}" = "tap" ]; then
        local tapdev="${QEMU_TAP:-tap0}"
        net_args=(
            -netdev tap,id=net0,ifname="$tapdev",script=no,downscript=no,vhost=on
            -device virtio-net-pci,netdev=net0
        )
        info "Network: vhost/tap ($tapdev) — bypassing slirp"
    else
        # Explicit device (not the -nic shorthand) so we can request MSI-X
        # vectors (config + RX + TX). The shorthand's virtio-net exposes NO
        # MSI-X capability, which blocks net-RX's event-driven RX IRQ
        # (confirmed: "[npk] msix: 00:03.0 no MSI-X capability"). Transitional
        # (1af4:1000) so the legacy driver still binds; slirp backend unchanged.
        # rx_queue_size=1024: deepen the host NIC RX ring (default 256) so a
        # download burst that outruns a brief drain gap is absorbed instead of
        # dropped by QEMU/slirp (the dropped frames made the SERVER retransmit →
        # its cwnd collapsed → the download lottery). Pairs with RX_BUFFERS=1024.
        # (tx_queue_size stays 256 — slirp/non-vhost caps TX at 256; download
        # loss is RX-side anyway.)
        net_args=(
            -netdev user,id=net0
            -device virtio-net-pci,netdev=net0,vectors=3,rx_queue_size=1024
        )
    fi
    echo ""

    # Serial output: COM1 → live file unbuffered AND mirrored to stdio
    # for interactive typing (installer prompt etc.). Two QEMU `-serial`
    # arms give two COM ports — COM1 (interactive stdio) + COM2 (file
    # log). Kernel mirrors every byte from COM1 (0x3F8) to COM2 (0x2F8)
    # so a `pkill -9 qemu` can't lose buffered stdio output — every
    # byte hit COM2's file backend synchronously already.
    # After QEMU exits inspect with:  cat target/serial.log
    local serial_log="$PROJECT_DIR/target/serial.log"
    rm -f "$serial_log"
    log "Serial log: $serial_log"

    qemu-system-x86_64 \
        "${accel_args[@]}" \
        "${fw_args[@]}" \
        -serial mon:stdio \
        -serial file:"$serial_log" \
        "${display_args[@]}" \
        -m 4096M \
        -smp "${QEMU_SMP:-4}" \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -drive file="$DISK_IMG",format=raw,if=none,id=drive0 \
        -device nvme,drive=drive0,serial=nopeekos-test \
        -device qemu-xhci,id=xhci \
        -device usb-kbd,bus=xhci.0 \
        -device usb-mouse,bus=xhci.0 \
        -device intel-hda \
        -device hda-output,audiodev=snd0 \
        -audiodev "${QEMU_AUDIODEV:-wav,id=snd0,path=/tmp/nopeek-audio.wav}" \
        "${installer_args[@]}" \
        "${net_args[@]}" \
        -no-reboot \
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
    [ ! -f "$KERNEL_EFI" ] && { err "kernel.efi not found. Run ./build.sh installer first."; exit 1; }

    # Safety: never write to NVMe
    if [[ "$device" == *"nvme"* ]]; then
        err "Refusing to write to NVMe device: $device"
        exit 1
    fi

    # Check tools
    for cmd in sgdisk mkfs.fat; do
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

    log "Installing kernel.efi as /EFI/BOOT/BOOTX64.EFI..."
    sudo mkdir -p "$mnt/EFI/BOOT"
    sudo cp "$KERNEL_EFI" "$mnt/EFI/BOOT/BOOTX64.EFI"

    # Marker so the installer kernel knows this is a USB install medium
    # (vs being booted from the target NVMe ESP that it itself wrote).
    sudo touch "$mnt/.npk-usb-boot"

    sync
    sudo umount "$mnt"
    rmdir "$mnt"

    ok "USB stick ready: $device"
    log "ESP: $esp_part (FAT32, /EFI/BOOT/BOOTX64.EFI = kernel.efi)"
    log "Plug into NUC, select USB in boot menu (UEFI)."
}

# ============================================================
# Hilfsfunktionen
# ============================================================

check_deps() {
    local missing=()

    if ! command -v cargo &> /dev/null; then
        missing+=("cargo (rustup install)")
    fi
    if ! command -v objcopy &> /dev/null; then
        missing+=("objcopy (apt install binutils / pacman -S binutils)")
    fi
    if ! command -v mformat &> /dev/null; then
        missing+=("mtools (apt install mtools)")
    fi
    if ! command -v sgdisk &> /dev/null; then
        missing+=("sgdisk (apt install gdisk)")
    fi
    if ! command -v python3 &> /dev/null; then
        missing+=("python3 (apt install python3)")
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

UEFI-only. All QEMU runs use OVMF; no SeaBIOS / multiboot path.

Common:
  build                Compile kernel.efi (PE32+ UEFI Application)
  qemu                 Build + run in QEMU (KVM, host CPU, serial)
  qemu-gui             Build + run in QEMU with framebuffer GUI
  boot / qemu-gui-boot Boot the EXISTING installed disk, NO rebuild (GUI)
  qemu-boot            Same, serial only
  debug                Build + run in QEMU with GDB stub on :1234

Cross-vendor testing (TCG, slow but vendor-correct):
  qemu-intel           Force Intel CPU emulation regardless of host
  qemu-amd             Force AMD CPU emulation regardless of host

Installer / release:
  installer            Two-pass installer build (kernel.efi + bundled assets)
  qemu-installer       Installer + run (wipes NVMe, attaches installer USB)
  qemu-installer-gui   Same with framebuffer (1920x1080)
  qemu-installer-full  Same as qemu-installer-gui but bundles the
                       LibreWolf userspace sqfs (~261 MB) so the
                       fresh install has the browser ready on first
                       boot without any OTA round-trip
  usb /dev/sdX         Build fully self-contained installer + flash USB.
                       Bundles EVERYTHING (microVM Linux+initramfs, all
                       WASM modules, fonts/icons/wallpapers AND the
                       LibreWolf browser sqfs) → USB ~290 MB, first boot
                       works fully offline, no OTA needed post-install.
  usb-full /dev/sdX    Alias of `usb` (kept for back-compat).
  release              Sign kernel + modules + assets (ECDSA P-384)
  release-large <tag>  Upload release/assets/large/* to GitHub Releases
                       under <tag>, sign with update.key, write
                       manifest.large with url= overrides. The next
                       `release` merges those into the canonical asset
                       manifest. Use for files too big for raw-content
                       (Alpine+Mesa userspace bundles, >30 MB).

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
        run_qemu_generic serial kvm normal
        ;;
    qemu-gui)
        check_deps
        build
        run_qemu_generic gui kvm normal
        ;;
    # Boot the EXISTING installed disk without rebuilding the kernel.
    # Normal-mode boot runs straight from disk.img's ESP — the fresh
    # kernel.efi is only used by the installer — so `build` is wasted work
    # when you just want to boot what's already installed. Use these for a
    # fast relaunch; use OTA `update` inside the OS (or a fresh installer
    # run) to change the installed kernel.
    qemu-gui-boot|boot)
        check_deps
        require_installed_disk
        run_qemu_generic gui kvm normal
        ;;
    qemu-boot)
        check_deps
        require_installed_disk
        run_qemu_generic serial kvm normal
        ;;
    qemu-intel)
        check_deps
        build
        run_qemu_generic serial tcg-intel normal
        ;;
    qemu-amd)
        check_deps
        build
        run_qemu_generic serial tcg-amd normal
        ;;
    qemu-installer)
        # Two-pass installer build: produces $KERNEL_EFI containing the
        # installer logic, then wraps it in an ESP-formatted disk image
        # ($INSTALLER_DISK) that QEMU attaches as a USB stick. NVMe is
        # wiped so OVMF can't boot from it and must fall through to
        # the installer USB.
        check_deps
        build_installer
        build_installer_disk
        wipe_disk_img
        run_qemu_generic serial kvm installer
        ;;
    qemu-installer-gui)
        check_deps
        build_installer
        build_installer_disk
        wipe_disk_img
        run_qemu_generic gui kvm installer
        ;;
    qemu-installer-full)
        # Same as qemu-installer but bakes the LibreWolf userspace sqfs
        # (~261 MB) into the installer kernel so the QEMU NVMe is fully
        # seeded with the browser bundle — no `update` needed before
        # `browser` works post-install. Use for testing the bundled-
        # browser flow end-to-end.
        check_deps
        BUNDLE_USERSPACE=1 build_installer
        build_installer_disk
        wipe_disk_img
        run_qemu_generic gui kvm installer
        ;;
    debug)
        check_deps
        build
        log "Launching QEMU with GDB stub on :1234..."
        warn "In another terminal: gdb $KERNEL_BIN -ex 'target remote :1234'"
        run_qemu_generic serial kvm normal -s -S
        ;;
    installer)
        check_deps
        build_installer
        ;;
    usb)
        # Bootable install stick (~15 MB EFI). Bundles microVM Linux +
        # initramfs, all WASM modules, fonts/icons/wallpapers — but NOT
        # the 261 MB LibreWolf sqfs: embedding it makes a ~290 MB EFI
        # image that some laptop firmware (e.g. HP) cannot LoadImage (it
        # can't allocate ~294 MB contiguous), so it bootloops. The
        # browser bundle arrives via OTA `update` post-install, or use
        # `usb-full` for the (large) embedded variant on firmware that
        # tolerates it.
        check_deps
        build_installer
        write_usb "${2:-}"
        ;;
    usb-full)
        # Same as `usb` but embeds the LibreWolf sqfs (~261 MB) so a
        # fresh install has the browser with no OTA. WARNING: the
        # resulting ~290 MB EFI fails to load on some firmware (HP
        # laptops) — use `usb` there. See [[project_uefi_relocatable]].
        check_deps
        BUNDLE_USERSPACE=1 build_installer
        write_usb "${2:-}"
        ;;
    build-linux)
        # Build the custom MicroVM Linux kernel — see microvm-linux/.
        # Output: release/assets/linux-virt.bzImage. Idempotent (skips
        # configure if config fragment unchanged + bzImage already up
        # to date in the source dir).
        bash "$PROJECT_DIR/microvm-linux/build.sh"
        ;;
    release)
        check_deps
        build
        log "Creating release artifacts..."
        RELEASE_DIR="$PROJECT_DIR/release"
        mkdir -p "$RELEASE_DIR"

        # Copy kernel.efi (the PE+ UEFI app — what OTA `update` fetches
        # and writes to /EFI/BOOT/BOOTX64.EFI).
        cp "$KERNEL_EFI" "$RELEASE_DIR/kernel.efi"

        # Read version from Cargo.toml
        VERSION=$(grep '^version' "$PROJECT_DIR/kernel/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
        SIZE=$(stat -c%s "$RELEASE_DIR/kernel.efi")
        SHA384=$(openssl dgst -sha384 -hex "$RELEASE_DIR/kernel.efi" 2>/dev/null | awk '{print $NF}')

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
            openssl dgst -sha384 -sign "$KEY_FILE" -out "$RELEASE_DIR/kernel.sig" "$RELEASE_DIR/kernel.efi"
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
            # .ttf plus the OFL licence texts — SIL OFL requires the
            # licence to travel with every copy of the font, including
            # to systems that only ever see it via OTA.
            for font_src in "$PROJECT_DIR/sys/fonts/"*.ttf "$PROJECT_DIR/sys/fonts/"LICENSE-*.txt; do
                [ -f "$font_src" ] || continue
                FONT_NAME=$(basename "$font_src")
                cp "$font_src" "$RELEASE_DIR/assets/$FONT_NAME"
                ASSET_SIZE=$(stat -c%s "$RELEASE_DIR/assets/$FONT_NAME")
                ASSET_SHA=$(openssl dgst -sha384 -hex "$RELEASE_DIR/assets/$FONT_NAME" 2>/dev/null | awk '{print $NF}')
                SECTION="${FONT_NAME%.ttf}"; SECTION="${SECTION%.txt}"

                ASSET_MANIFEST="${ASSET_MANIFEST}[font:${SECTION}]
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

        # System wallpapers — every file in release/assets/wallpapers/ is
        # signed and listed as `[wallpaper:<filename>]`. The kernel needs no
        # entry for these: it maps that section straight to
        # `sys/wallpapers/<filename>` (intent/update.rs), so shipping a new
        # wallpaper — or REPLACING one — is dropping a file here and running
        # `release`. Users get it with `update`, no reinstall.
        if [ -d "$RELEASE_DIR/assets/wallpapers" ]; then
            for wp in "$RELEASE_DIR/assets/wallpapers/"*; do
                [ -f "$wp" ] || continue
                case "$wp" in *.sig) continue;; esac
                WP_NAME=$(basename "$wp")
                WP_SIZE=$(stat -c%s "$wp")
                WP_SHA=$(openssl dgst -sha384 -hex "$wp" 2>/dev/null | awk '{print $NF}')

                ASSET_MANIFEST="${ASSET_MANIFEST}[wallpaper:${WP_NAME}]
size=${WP_SIZE}
sha384=${WP_SHA}

"
                if [ -f "$KEY_FILE" ]; then
                    openssl dgst -sha384 -sign "$KEY_FILE" -out "${wp}.sig" "$wp"
                    ok "Signed wallpaper: $WP_NAME ($WP_SIZE bytes)"
                fi
            done
        fi

        # Root CA anchors — every DER in release/assets/certs/ is signed and
        # listed as `[cert:<filename>]`, mapped to `sys/certs/<filename>`
        # (intent/update.rs). Same deal as wallpapers: adding or replacing an
        # anchor is dropping a file here, no kernel change.
        #
        # Adding a root CA means every certificate it signs is trusted by the
        # whole system — a security boundary, not a bugfix. Check the SHA-256
        # against the CA's published value before putting a file here:
        #   openssl x509 -inform DER -in <file> -noout -subject -enddate \
        #       -fingerprint -sha256
        # The anchors needed to reach the update host itself are compiled
        # into the kernel (crypto/tls/certstore.rs), so a mistake here can
        # never lock a machine out of its own updates.
        if [ -d "$RELEASE_DIR/assets/certs" ]; then
            for ca in "$RELEASE_DIR/assets/certs/"*; do
                [ -f "$ca" ] || continue
                case "$ca" in *.sig|*.md) continue;; esac
                CA_NAME=$(basename "$ca")
                CA_SIZE=$(stat -c%s "$ca")
                CA_SHA=$(openssl dgst -sha384 -hex "$ca" 2>/dev/null | awk '{print $NF}')

                # Refuse to ship something that is not a certificate: a
                # malformed anchor is skipped at load time on the device,
                # which looks like the update silently not working.
                if ! openssl x509 -inform DER -in "$ca" -noout >/dev/null 2>&1; then
                    err "Not a DER certificate, refusing to ship: assets/certs/$CA_NAME"
                    exit 1
                fi

                ASSET_MANIFEST="${ASSET_MANIFEST}[cert:${CA_NAME}]
size=${CA_SIZE}
sha384=${CA_SHA}

"
                if [ -f "$KEY_FILE" ]; then
                    openssl dgst -sha384 -sign "$KEY_FILE" -out "${ca}.sig" "$ca"
                    CA_SUBJ=$(openssl x509 -inform DER -in "$ca" -noout -subject 2>/dev/null | sed 's/^subject=//')
                    ok "Signed cert: $CA_NAME ($CA_SUBJ)"
                fi
            done
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

        # MicroVM userspace bundle — produced by
        # microvm-userspace/build.sh. Optional. Currently
        # alpine-wayland; future iterations add Mesa/LibreWolf on
        # top via Alpine APKBUILD. NOT bundled into the kernel
        # binary (BUNDLED_ASSETS) — pure OTA asset.
        #
        # Small enough to live on raw.githubusercontent? Treat as a
        # regular release asset (path under release/assets/). When
        # the bundle grows past raw-content limits (~50 MB warning,
        # 100 MB hard cap) move it to release/assets/large/ and ship
        # via `./build.sh release-large <tag>` — that path adds a
        # `url=` line pointing at GitHub Releases and the kernel
        # follows the 302 redirect chain transparently.
        USERSPACE_FILE="$RELEASE_DIR/assets/microvm-userspace.cpio.gz"
        if [ -f "$USERSPACE_FILE" ]; then
            US_SIZE=$(stat -c%s "$USERSPACE_FILE")
            US_SHA=$(openssl dgst -sha384 -hex "$USERSPACE_FILE" 2>/dev/null | awk '{print $NF}')

            ASSET_MANIFEST="${ASSET_MANIFEST}[microvm:userspace]
size=${US_SIZE}
sha384=${US_SHA}

"
            if [ -f "$KEY_FILE" ]; then
                openssl dgst -sha384 -sign "$KEY_FILE" \
                    -out "${USERSPACE_FILE}.sig" "$USERSPACE_FILE"
                ok "Signed userspace bundle: microvm-userspace.cpio.gz ($US_SIZE bytes)"
            fi
        fi

        # Squashfs form of the userspace bundle — read-only, PID-1
        # mounts it from /dev/vdb. Same provenance as the cpio above
        # but RAM-efficient. Optional; ship whichever form exists.
        USERSPACE_SQFS="$RELEASE_DIR/assets/microvm-userspace.sqfs"
        if [ -f "$USERSPACE_SQFS" ]; then
            USQ_SIZE=$(stat -c%s "$USERSPACE_SQFS")
            USQ_SHA=$(openssl dgst -sha384 -hex "$USERSPACE_SQFS" 2>/dev/null | awk '{print $NF}')

            ASSET_MANIFEST="${ASSET_MANIFEST}[microvm:userspace-sqfs]
size=${USQ_SIZE}
sha384=${USQ_SHA}

"
            if [ -f "$KEY_FILE" ]; then
                openssl dgst -sha384 -sign "$KEY_FILE" \
                    -out "${USERSPACE_SQFS}.sig" "$USERSPACE_SQFS"
                ok "Signed userspace bundle: microvm-userspace.sqfs ($USQ_SIZE bytes)"
            fi
        fi

        # Pick up any pre-existing url= overrides from a previous
        # `release-large` invocation. The large-asset manifest is
        # written to release/assets/manifest.large by `release-large`
        # and merged in here so a regular `./build.sh release` after
        # an upload doesn't clobber the url= entries.
        LARGE_MANIFEST="$RELEASE_DIR/assets/manifest.large"
        if [ -f "$LARGE_MANIFEST" ]; then
            ASSET_MANIFEST="${ASSET_MANIFEST}$(cat "$LARGE_MANIFEST")

"
            ok "Merged large-asset overrides from manifest.large"
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
    release-large)
        # Upload large OTA assets to GitHub Releases and emit a
        # `manifest.large` snippet that the next `release` merges
        # into the canonical `release/assets/manifest`.
        #
        # Anything bigger than ~50 MB cannot ship via
        # raw.githubusercontent.com reliably (warnings start there,
        # hard cap ~100 MB). Drop those into release/assets/large/
        # (gitignored) and run `./build.sh release-large <tag>`. We:
        #
        #   1. sha384 + sign each file with update.key (same key
        #      as kernel + module OTA).
        #   2. `gh release create <tag>` (idempotent — if it exists
        #      we just upload more assets).
        #   3. `gh release upload <tag> <file> <file>.sig`.
        #   4. write `release/assets/manifest.large` with each
        #      asset's INI section including a `url=` line that
        #      points at the github.com/.../releases/download URL.
        #      The kernel follows the 302 redirect chain via the
        #      `https_get_streaming` path.
        TAG="${2:-}"
        if [ -z "$TAG" ]; then
            err "usage: ./build.sh release-large <tag>"
            err "example: ./build.sh release-large assets/alpine-wayland-mesa-0.1.0"
            exit 1
        fi
        # These are set in the `release)` arm but not here — this arm
        # ran with `set -u` against unset vars (never exercised before).
        RELEASE_DIR="$PROJECT_DIR/release"
        KEY_FILE="$PROJECT_DIR/update.key"
        LARGE_DIR="$RELEASE_DIR/assets/large"
        if [ ! -d "$LARGE_DIR" ]; then
            err "no $LARGE_DIR directory found"
            err "drop large assets there (gitignored), then re-run"
            exit 1
        fi
        if ! command -v gh >/dev/null 2>&1; then
            err "gh (GitHub CLI) not found — install via 'pacman -S github-cli'"
            exit 1
        fi
        if [ ! -f "$KEY_FILE" ]; then
            err "$KEY_FILE missing — cannot sign assets"
            exit 1
        fi

        # Determine GitHub repo from origin remote.
        # Host-agnostic owner/repo extraction. Handles github.com as
        # well as custom SSH host aliases (e.g. this repo's
        # `git@github-nopeekOS:fnopeek/nopeekOS.git`) and ssh:// URLs:
        # strip any scheme + `user@host:` / `host/` prefix, drop the
        # trailing `.git`, keep the last `owner/repo` pair. Overridable
        # via `GH_REPO` for non-standard setups.
        REPO_URL=$(git config --get remote.origin.url 2>/dev/null || echo "")
        REPO="${GH_REPO:-}"
        if [ -z "$REPO" ]; then
            REPO=$(echo "$REPO_URL" | sed -E '
                s,^[a-z]+://,,;            # strip scheme://
                s,^[^@]+@,,;               # strip user@
                s,^[^/:]+[:/],,;           # strip host: or host/
                s,\.git$,,;                # strip trailing .git
            ' | grep -oE '[^/]+/[^/]+$' || echo "")
        fi
        if [ -z "$REPO" ] || ! echo "$REPO" | grep -q '/'; then
            err "could not parse origin remote: $REPO_URL"
            err "override with: GH_REPO=owner/repo ./build.sh release-large <tag>"
            exit 1
        fi
        log "Repo: $REPO  Tag: $TAG"

        # Create the release if missing. `gh release view` returns
        # non-zero when it doesn't exist; suppress so we can branch.
        if ! gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
            log "Creating release $TAG..."
            gh release create "$TAG" --repo "$REPO" \
                --title "$TAG" \
                --notes "Large OTA assets. Signed with the same ECDSA P-384 update key as kernel + modules. Fetched by the kernel via \`update\` with redirect-following \`https_get\`." \
                --target main
        else
            log "Release $TAG exists, uploading additional assets"
        fi

        LARGE_MANIFEST=""
        UPLOADED=0
        for asset in "$LARGE_DIR"/*; do
            [ -f "$asset" ] || continue
            # Skip any pre-existing .sig — we regenerate fresh.
            case "$asset" in *.sig) continue;; esac

            NAME=$(basename "$asset")
            SIZE=$(stat -c%s "$asset")
            SHA=$(openssl dgst -sha384 -hex "$asset" 2>/dev/null | awk '{print $NF}')

            openssl dgst -sha384 -sign "$KEY_FILE" \
                -out "${asset}.sig" "$asset"

            # Section name follows the AssetSpec table convention:
            # microvm-userspace.cpio.gz → [microvm:userspace]
            case "$NAME" in
                microvm-userspace.cpio.gz) SECTION="microvm:userspace" ;;
                microvm-userspace.sqfs)    SECTION="microvm:userspace-sqfs" ;;
                *)
                    err "unknown large asset filename: $NAME"
                    err "add an AssetSpec entry in kernel/src/intent/update.rs and a case here"
                    continue
                    ;;
            esac

            URL="https://github.com/${REPO}/releases/download/${TAG}/${NAME}"

            LARGE_MANIFEST="${LARGE_MANIFEST}[${SECTION}]
size=${SIZE}
sha384=${SHA}
url=${URL}

"

            log "Uploading $NAME ($SIZE bytes) + .sig..."
            gh release upload "$TAG" --repo "$REPO" --clobber "$asset" "${asset}.sig"
            ok "Uploaded $NAME"
            UPLOADED=$((UPLOADED + 1))
        done

        if [ "$UPLOADED" = 0 ]; then
            err "no assets uploaded — $LARGE_DIR is empty or contains only .sig files"
            exit 1
        fi

        # Make sure the assets dir exists before writing manifest.large
        mkdir -p "$RELEASE_DIR/assets"
        printf '%s' "$LARGE_MANIFEST" > "$RELEASE_DIR/assets/manifest.large"
        ok "Wrote $RELEASE_DIR/assets/manifest.large ($UPLOADED entries)"
        log "Run ./build.sh release to merge into the canonical asset manifest, then commit + push."
        ;;
    help|-h|--help)
        usage
        ;;
    *)
        check_deps
        build
        run_qemu_generic serial kvm normal
        ;;
esac

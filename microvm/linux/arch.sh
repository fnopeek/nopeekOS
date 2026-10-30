# Sourced by the guest builds: a throwaway Arch Linux root on the pinned
# snapshot (microvm-userspace/arch.env), driven without root on the host.
#
# The root lives in a user namespace with the caller's subordinate ids
# mapped (`unshare --map-auto`), so pacman can give files their real
# owners. Those files belong to sub-ids on the host: everything that
# touches the root, removing it included, runs through `arch_run`.

ARCH_SH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$ARCH_SH_DIR/../../microvm-userspace/arch.env"

ARCH_CACHE="${HOME}/.cache/nopeekos/arch"
ARCH_TARBALL="archlinux-bootstrap-${ARCH_BOOTSTRAP}-x86_64.tar.zst"

# Download the bootstrap tarball once; check it against the pinned sha256
# and Arch's release signature on every run.
arch_fetch() {
    mkdir -p "$ARCH_CACHE/pkg"
    local base="https://archive.archlinux.org/iso/${ARCH_BOOTSTRAP}"
    if [ ! -f "$ARCH_CACHE/$ARCH_TARBALL" ]; then
        curl -L --fail --max-time 600 -o "$ARCH_CACHE/$ARCH_TARBALL.part" "$base/$ARCH_TARBALL"
        mv "$ARCH_CACHE/$ARCH_TARBALL.part" "$ARCH_CACHE/$ARCH_TARBALL"
    fi
    [ -f "$ARCH_CACHE/$ARCH_TARBALL.sig" ] \
        || curl -L --fail --max-time 60 -o "$ARCH_CACHE/$ARCH_TARBALL.sig" "$base/$ARCH_TARBALL.sig"
    echo "${ARCH_BOOTSTRAP_SHA256}  $ARCH_CACHE/$ARCH_TARBALL" | sha256sum -c - >/dev/null \
        || { echo "sha256 mismatch on $ARCH_TARBALL" >&2; return 1; }
    local gnupg="$ARCH_CACHE/gnupg"
    mkdir -p -m 700 "$gnupg"
    GNUPGHOME="$gnupg" gpg -q --list-keys "$ARCH_RELEASE_KEY" >/dev/null 2>&1 \
        || GNUPGHOME="$gnupg" gpg -q --auto-key-locate clear,wkd --locate-external-key pierre@archlinux.org >/dev/null 2>&1 \
        || { echo "cannot fetch Arch's release key" >&2; return 1; }
    GNUPGHOME="$gnupg" gpg --status-fd 1 --verify "$ARCH_CACHE/$ARCH_TARBALL.sig" "$ARCH_CACHE/$ARCH_TARBALL" 2>/dev/null \
        | grep -q "^\[GNUPG:\] VALIDSIG $ARCH_RELEASE_KEY " \
        || { echo "bad signature on $ARCH_TARBALL" >&2; return 1; }
}

# arch_run ROOT SCRIPT: run the bash SCRIPT in the namespace. ROOT is
# created from the bootstrap tarball if it does not exist yet. Inside
# SCRIPT, $R is the root, `in_root CMD...` runs CMD chrooted into it, and
# pacman's package cache is shared across builds.
arch_run() {
    local root="$1" script="$2"
    unshare --user --map-root-user --map-auto --mount --pid --fork --mount-proc \
        bash -c '
            set -euo pipefail
            R="$1"; CACHE="$2"; TARBALL="$3"; SNAPSHOT="$4"
            if [ ! -d "$R/usr" ]; then
                mkdir -p "$R"
                bsdtar -xpf "$CACHE/$TARBALL" -C "$R" --numeric-owner --strip-components 1 root.x86_64
                echo "Server = https://archive.archlinux.org/repos/$SNAPSHOT/\$repo/os/\$arch" > "$R/etc/pacman.d/mirrorlist"
                # The namespace has no second user for downloads and no
                # landlock for the sandbox; free space is the host'"'"'s.
                sed -i "s/^DownloadUser/#DownloadUser/; s/^CheckSpace/#CheckSpace/" "$R/etc/pacman.conf"
                sed -i "/^\[options\]/a DisableSandboxFilesystem\nDisableSandboxSyscalls" "$R/etc/pacman.conf"
                FRESH=1
            fi
            echo "nameserver 1.1.1.1" > "$R/etc/resolv.conf"
            mount --rbind /dev "$R/dev"
            mount -t proc proc "$R/proc"
            mount -t tmpfs tmpfs "$R/tmp"
            mount --bind "$CACHE/pkg" "$R/var/cache/pacman/pkg"
            in_root() { chroot "$R" "$@"; }
            if [ "${FRESH:-0}" = 1 ]; then
                in_root pacman-key --init >/dev/null 2>&1
                in_root pacman-key --populate archlinux >/dev/null 2>&1
            fi
            eval "$5"
        ' arch "$root" "$ARCH_CACHE" "$ARCH_TARBALL" "$ARCH_SNAPSHOT" "$script"
}

# arch_rm DIR: remove a tree that holds files owned by sub-ids.
arch_rm() {
    [ -e "$1" ] || return 0
    unshare --user --map-root-user --map-auto rm -rf "$1"
}

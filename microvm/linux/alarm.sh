# Sourced by the aarch64 guest builds: a throwaway Arch Linux ARM root
# (microvm-userspace/alarm.env), driven without root on an x86_64 host.
#
# The namespace is arch.sh's, plus a binfmt_misc instance of its own that
# runs aarch64 ELF files through Arch's static qemu-aarch64. The host's
# pacman installs into the root; its scriptlets and hooks, and every
# `in_root` command, then run inside the root as they would on the target.

ALARM_SH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$ALARM_SH_DIR/arch.sh"
source "$ALARM_SH_DIR/../../microvm-userspace/alarm.env"

ALARM_CACHE="${HOME}/.cache/nopeekos/alarm"
ALARM_QEMU="$ALARM_CACHE/qemu-aarch64-static"

# The keyring once per pin, checked against its sha256 and the build key;
# pacman's own key store is made from it. qemu-aarch64-static comes from
# Arch's qemu-user-static on the pinned x86_64 snapshot.
alarm_fetch() {
    mkdir -p "$ALARM_CACHE/pkg"
    local kr="$ALARM_CACHE/$ALARM_KEYRING"
    [ -f "$kr" ] || curl -L --fail --max-time 120 -o "$kr" "$ALARM_MIRROR/core/$ALARM_KEYRING"
    [ -f "$kr.sig" ] || curl -L --fail --max-time 60 -o "$kr.sig" "$ALARM_MIRROR/core/$ALARM_KEYRING.sig"
    echo "${ALARM_KEYRING_SHA256}  $kr" | sha256sum -c - >/dev/null \
        || { echo "sha256 mismatch on $ALARM_KEYRING" >&2; return 1; }
    local gnupg="$ALARM_CACHE/gnupg"
    mkdir -p -m 700 "$gnupg"
    GNUPGHOME="$gnupg" gpg -q --list-keys "$ALARM_BUILD_KEY" >/dev/null 2>&1 \
        || GNUPGHOME="$gnupg" gpg -q --keyserver "$ALARM_KEYSERVER" --recv-keys "$ALARM_BUILD_KEY" >/dev/null 2>&1 \
        || { echo "cannot fetch the Arch Linux ARM build key" >&2; return 1; }
    GNUPGHOME="$gnupg" gpg --status-fd 1 --verify "$kr.sig" "$kr" 2>/dev/null \
        | grep -q "^\[GNUPG:\] VALIDSIG $ALARM_BUILD_KEY " \
        || { echo "bad signature on $ALARM_KEYRING" >&2; return 1; }

    local pk="$ALARM_CACHE/pacman-gnupg"
    if [ "$(cat "$pk/.keyring" 2>/dev/null)" != "$ALARM_KEYRING_SHA256" ]; then
        rm -rf "$pk" "$ALARM_CACHE/keyrings"
        mkdir -p "$pk" "$ALARM_CACHE/keyrings"
        bsdtar -xf "$kr" -C "$ALARM_CACHE/keyrings" --strip-components 4 usr/share/pacman/keyrings
        unshare --user --map-root-user bash -c '
            pacman-key --gpgdir "$1" --init >/dev/null 2>&1
            pacman-key --gpgdir "$1" --populate-from "$2" --populate archlinuxarm >/dev/null 2>&1
        ' alarm "$pk" "$ALARM_CACHE/keyrings" \
            || { echo "cannot set up the pacman keyring" >&2; return 1; }
        echo "$ALARM_KEYRING_SHA256" > "$pk/.keyring"
        gpgconf --homedir "$pk" --kill all 2>/dev/null || true
    fi
    gpgconf --homedir "$gnupg" --kill all 2>/dev/null || true

    if [ ! -x "$ALARM_QEMU" ]; then
        arch_fetch
        local stage
        stage="$(mktemp -d)"
        arch_run "$stage/root" "
            in_root pacman -Syw --noconfirm --nodeps qemu-user-static >/dev/null
            f=\$(in_root pacman -Sp --nodeps qemu-user-static | sed 's,.*/,,')
            bsdtar -xOf \"\$R/var/cache/pacman/pkg/\$f\" usr/bin/qemu-aarch64-static > '$stage/qemu'
        " || { arch_rm "$stage"; return 1; }
        install -m 0755 "$stage/qemu" "$ALARM_QEMU"
        arch_rm "$stage"
    fi
}

# alarm_run ROOT SCRIPT: run the bash SCRIPT in the namespace. Inside
# SCRIPT, $R is the root, `alarm_pacman ARGS...` is the host's pacman on it,
# `in_root CMD...` runs CMD chrooted into it, and the package cache is
# shared across builds. A missing root gets `base` from the mirror.
alarm_run() {
    local root="$1" script="$2"
    unshare --user --map-root-user --map-auto --mount --pid --fork --mount-proc \
        bash -c '
            set -euo pipefail
            R="$1"; CACHE="$2"; QEMU="$3"; MIRROR="$4"; REPOS="$5"
            mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc
            # F: the interpreter is opened now, so it need not exist in the root.
            printf "%s" ":aarch64:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\xb7\x00:\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:$QEMU:F" \
                > /proc/sys/fs/binfmt_misc/register
            mkdir -p "$R/etc/pacman.d/hooks" "$R/var/lib/pacman" "$R/var/log" "$R/dev" "$R/proc" "$R/tmp" "$R/var/cache/pacman/pkg"
            CONF="$R/.alarm-pacman.conf"
            {
                printf "%s\n" "[options]" "Architecture = aarch64" "SigLevel = Required DatabaseOptional" \
                    "LocalFileSigLevel = Required" "DisableSandbox" "ParallelDownloads = 8"
                for repo in $REPOS; do printf "[%s]\nServer = %s/\$repo\n" "$repo" "$MIRROR"; done
            } > "$CONF"
            alarm_pacman() {
                pacman --config "$CONF" --root "$R" --dbpath "$R/var/lib/pacman" \
                    --cachedir "$CACHE/pkg" --gpgdir "$CACHE/pacman-gnupg" \
                    --logfile "$R/var/log/pacman.log" --hookdir "$R/etc/pacman.d/hooks" "$@"
            }
            echo "nameserver 1.1.1.1" > "$R/etc/resolv.conf"
            mount --rbind /dev "$R/dev"
            mount -t proc proc "$R/proc"
            mount -t tmpfs tmpfs "$R/tmp"
            mount --bind "$CACHE/pkg" "$R/var/cache/pacman/pkg"
            in_root() { chroot "$R" "$@"; }
            if [ ! -e "$R/usr/bin/bash" ] && [ "${ALARM_NO_BASE:-0}" != 1 ]; then
                alarm_pacman -Sy --noconfirm base >/dev/null
            fi
            eval "$6"
        ' alarm "$root" "$ALARM_CACHE" "$ALARM_QEMU" "$ALARM_MIRROR" "$ALARM_REPOS" "$script"
}

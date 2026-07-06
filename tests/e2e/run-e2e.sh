#!/usr/bin/env bash
# QEMU end-to-end install test: boot the cosmic-build-meta live ISO,
# drive cosmonaut-installer-cli over the ttyS1 debug shell, then boot
# the target disk and assert it reaches a login prompt.
#
# Usage:
#   tests/e2e/run-e2e.sh <variant> [iso-path]
#     variant: none | luks-passphrase | tpm2-luks | tpm2-luks-passphrase
#     iso-path: live ISO (default $COSMIC_ISO or build/e2e/live.iso)
#
# Requirements: qemu-system-x86_64 with KVM, OVMF, socat; swtpm for the
# tpm2 variants. The ISO must carry the systemd debug shell on ttyS1
# (cosmic-build-meta live images do: systemd.debug_shell=ttyS1).
#
# The passphrase used for LUKS variants is "e2e-test-passphrase".

set -euo pipefail

VARIANT="${1:?variant required: none|luks-passphrase|tpm2-luks|tpm2-luks-passphrase}"
ISO="${2:-${COSMIC_ISO:-build/e2e/live.iso}}"
# /var/tmp, not /tmp: the target disk image must live on disk-backed
# storage (tmpfs can't hold a 40G sparse file's written extents).
WORK="$(mktemp -d "${COSMONAUT_E2E_WORK:-/var/tmp}/cosmonaut-e2e.XXXXXX")"
TARGET="$WORK/target.raw"
SERIAL_LOG="$WORK/serial.log"
DBGSHELL="$WORK/dbgshell.sock"
MONITOR="$WORK/monitor.sock"
PASSPHRASE="e2e-test-passphrase"
OVMF="${OVMF_CODE:-/usr/share/edk2/ovmf/OVMF_CODE.fd}"
[ -f "$OVMF" ] || OVMF=/usr/share/OVMF/OVMF_CODE.fd

BOOT_TIMEOUT="${BOOT_TIMEOUT:-420}"
INSTALL_TIMEOUT="${INSTALL_TIMEOUT:-1200}"
# The engine's skopeo scratch cache lives on /run (tmpfs), so the live
# VM needs RAM > compressed image size + ~6G for the system. 24G covers
# the current ~16G nvidia image; the plain cosmic image needs less.
VM_MEM="${VM_MEM:-24576}"

log() { echo "==> $*" >&2; }
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
    [ -n "${QEMU_PID:-}" ] && kill "$QEMU_PID" 2>/dev/null || true
    pkill -f "swtpm.*$WORK" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

[ -f "$ISO" ] || fail "live ISO not found: $ISO"
truncate -s 40G "$TARGET"

TPM_FLAGS=()
case "$VARIANT" in
    tpm2-*)
        command -v swtpm >/dev/null || fail "swtpm required for $VARIANT"
        mkdir -p "$WORK/tpm"
        swtpm socket --tpm2 --tpmstate "dir=$WORK/tpm" \
            --ctrl "type=unixio,path=$WORK/tpm.sock" --daemon
        TPM_FLAGS=(-chardev "socket,id=chrtpm,path=$WORK/tpm.sock"
                   -tpmdev emulator,id=tpm0,chardev=chrtpm
                   -device tpm-crb,tpmdev=tpm0)
        ;;
esac

start_qemu() { # args: extra drive flags...
    qemu-system-x86_64 \
        -m "$VM_MEM" -accel kvm -cpu host -smp 4 \
        -bios "$OVMF" \
        "$@" \
        -device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
        "${TPM_FLAGS[@]}" \
        -display none \
        -serial "file:$SERIAL_LOG" \
        -chardev "socket,id=dbgshell,path=$DBGSHELL,server=on,wait=off" \
        -serial chardev:dbgshell \
        -monitor "unix:$MONITOR,server,nowait" \
        -daemonize -pidfile "$WORK/qemu.pid"
    QEMU_PID=$(cat "$WORK/qemu.pid")
}

stop_qemu() {
    echo quit | socat - "UNIX-CONNECT:$MONITOR" >/dev/null 2>&1 || true
    sleep 3
    kill -9 "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""
}

wait_serial() { # args: pattern timeout
    local pattern="$1" deadline=$(( $(date +%s) + $2 ))
    until grep -aq "$pattern" "$SERIAL_LOG" 2>/dev/null; do
        [ "$(date +%s)" -ge "$deadline" ] && return 1
        sleep 5
    done
}

# Send a command to the live env's debug shell and wait for a marker in
# an output file the command writes.
shell_cmd() { # args: command
    { printf '%s\n' "$1"; sleep 2; } | socat -T 15 - "UNIX-CONNECT:$DBGSHELL" >/dev/null 2>&1 || true
}

# ---- Phase 1: boot live ISO, run the install --------------------------
log "booting live ISO ($VARIANT)"
: > "$SERIAL_LOG"
start_qemu \
    -drive "file=$ISO,if=virtio,format=raw,readonly=on" \
    -drive "file=$TARGET,if=virtio,format=raw"

wait_serial "login:" "$BOOT_TIMEOUT" || fail "live env did not reach login"
log "live env up; launching install"
# Wake the ttyS1 debug shell (first connect spawns it).
shell_cmd "true"

ENC_FLAGS=""
case "$VARIANT" in
    none) ;;
    luks-passphrase)       ENC_FLAGS="--luks-passphrase $PASSPHRASE" ;;
    tpm2-luks)             ENC_FLAGS="--tpm2-luks" ;;
    tpm2-luks-passphrase)  ENC_FLAGS="--tpm2-luks-passphrase $PASSPHRASE" ;;
esac

shell_cmd "nohup cosmonaut-installer-cli --disk /dev/vdb --image oci:/usr/lib/bootc/install-source/main $ENC_FLAGS > /tmp/cli.log 2>&1 &"

deadline=$(( $(date +%s) + INSTALL_TIMEOUT ))
STATUS=""
while [ -z "$STATUS" ]; do
    [ "$(date +%s)" -ge "$deadline" ] && fail "install timed out"
    sleep 20
    shell_cmd "grep -q '== install ok ==' /tmp/cli.log && touch /tmp/e2e-ok; grep -q 'completed: error' /tmp/cli.log && touch /tmp/e2e-fail; ls /tmp/e2e-* 2>/dev/null > /dev/ttyS0"
    if grep -aq "e2e-ok" "$SERIAL_LOG"; then STATUS=ok;
    elif grep -aq "e2e-fail" "$SERIAL_LOG"; then
        shell_cmd "tail -40 /tmp/cli.log > /dev/ttyS0"
        sleep 5
        tail -60 "$SERIAL_LOG" >&2
        fail "install reported an error"
    fi
done
log "install ok"

# Serial console on the installed system, so phase 2 is observable.
shell_cmd "mkdir -p /mnt/esp && mount /dev/vdb1 /mnt/esp && sed -i 's/^options /options console=ttyS0,115200n8 /' /mnt/esp/loader/entries/*.conf && umount /mnt/esp && sync"
sleep 3
stop_qemu

# ---- Phase 2: boot the installed disk ---------------------------------
# ttyS0 is a socket chardev this time so the harness can both read the
# console and type the LUKS passphrase into it.
log "booting installed target"
: > "$SERIAL_LOG"
SER0="$WORK/ser0.sock"
qemu-system-x86_64 \
    -m "$VM_MEM" -accel kvm -cpu host -smp 4 \
    -bios "$OVMF" \
    -drive "file=$TARGET,if=virtio,format=raw" \
    -device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
    "${TPM_FLAGS[@]}" \
    -display none \
    -chardev "socket,id=ser0,path=$SER0,server=on,wait=off" \
    -serial chardev:ser0 \
    -monitor "unix:$MONITOR,server,nowait" \
    -daemonize -pidfile "$WORK/qemu.pid"
QEMU_PID=$(cat "$WORK/qemu.pid")

# Persistent bidirectional tap: console output -> SERIAL_LOG, input <- FIFO.
mkfifo "$WORK/ser0-in.fifo"
( exec 3<>"$WORK/ser0-in.fifo"
  socat -t "$BOOT_TIMEOUT" - "UNIX-CONNECT:$SER0" <&3 > "$SERIAL_LOG" 2>&1 ) &

case "$VARIANT" in
    none|tpm2-luks)
        # tpm2-luks must auto-unlock; none has nothing to unlock.
        wait_serial "login:" "$BOOT_TIMEOUT" || fail "target did not reach login"
        ;;
    luks-passphrase|tpm2-luks-passphrase)
        wait_serial "Please enter passphrase" "$BOOT_TIMEOUT" \
            || fail "no LUKS passphrase prompt"
        { for (( i=0; i<${#PASSPHRASE}; i++ )); do
              printf '%s' "${PASSPHRASE:$i:1}"; sleep 0.1; done
          printf '\n'; } > "$WORK/ser0-in.fifo"
        wait_serial "login:" "$BOOT_TIMEOUT" || fail "target did not reach login after unlock"
        ;;
esac

log "PASS: $VARIANT installed and booted"

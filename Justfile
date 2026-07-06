# cosmonaut-installer build recipes.
#
# Used by the cosmic-build-meta BST element via:
#   just build-release
#   just rootdir="..." prefix="/usr" install

prefix     := "/usr"
rootdir    := ""
bindir     := prefix / "bin"
libexecdir := prefix / "libexec"
datadir    := prefix / "share"
sysconfdir := "/etc"

# Local dev build (allows the network for first-time crate fetches).
build:
    cargo build --release --workspace

# Sandboxed build: requires Cargo.lock + vendored sources from cargo2.
# `--frozen` (not `--locked`) per cosmic-build-meta's rule for offline
# vendored builds.
build-release:
    cargo build --release --workspace --frozen --offline

install:
    install -Dm755 target/release/cosmonaut-installer        "{{ rootdir }}{{ bindir }}/cosmonaut-installer"
    install -Dm755 target/release/cosmonaut-installer-cli    "{{ rootdir }}{{ bindir }}/cosmonaut-installer-cli"
    install -Dm755 target/release/cosmonaut-installer-daemon "{{ rootdir }}{{ libexecdir }}/cosmonaut-installer-daemon"

    install -Dm644 data/dbus/dev.cosmonaut.Installer1.service \
        "{{ rootdir }}{{ datadir }}/dbus-1/system-services/dev.cosmonaut.Installer1.service"
    install -Dm644 data/dbus/dev.cosmonaut.Installer1.conf \
        "{{ rootdir }}{{ datadir }}/dbus-1/system.d/dev.cosmonaut.Installer1.conf"
    install -Dm644 data/systemd/dev.cosmonaut.Installer1.service \
        "{{ rootdir }}{{ prefix }}/lib/systemd/system/dev.cosmonaut.Installer1.service"

    install -Dm644 data/polkit/dev.cosmonaut.Installer1.policy \
        "{{ rootdir }}{{ datadir }}/polkit-1/actions/dev.cosmonaut.Installer1.policy"

    install -Dm644 data/autostart/dev.cosmonaut.Installer.desktop \
        "{{ rootdir }}{{ sysconfdir }}/xdg/autostart/dev.cosmonaut.Installer.desktop"

check:
    cargo check --workspace --all-targets

# Pure unit tests (no root, no block devices).
test:
    cargo test --workspace

# Loopback-device integration tests for the partition/format layers.
# Needs root: losetup, sfdisk, mkfs.*, mount against loop devices.
# Compiles as the invoking user (root under sudo would re-download its
# own toolchain and inherit RUSTC_WRAPPER), then sudo-runs only the
# test executable.
test-engine:
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTC_WRAPPER="${RUSTC_WRAPPER_OVERRIDE:-}"
    BIN=$(cargo test -p cosmonaut-engine --test loopback --no-run --message-format=json \
        | grep -o '"executable":"[^"]*loopback[^"]*"' | cut -d'"' -f4 | head -1)
    test -n "$BIN" || { echo "loopback test binary not found"; exit 1; }
    echo "==> sudo $BIN --ignored --test-threads=1"
    sudo "$BIN" --ignored --test-threads=1

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

clean:
    cargo clean

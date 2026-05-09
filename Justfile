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

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

clean:
    cargo clean

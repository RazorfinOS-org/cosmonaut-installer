# cosmonaut-installer

A native libcosmic installer for COSMIC bootc images. Wraps a small Rust
install engine that drives `bootc install to-filesystem` against an
opinionated, hardcoded install profile (btrfs + composefs + systemd-boot,
optional LUKS).

Designed for [cosmic-build-meta](https://github.com/razorfinos-org/cosmic-build-meta)
and projects that junction off it.

## Status

Phase 0 — libcosmic + cargo2 spike. The GUI binary opens an empty COSMIC
window. The daemon, CLI, and engine crates are stubs.

## Layout

| Crate                          | Output                                          | Purpose                              |
|--------------------------------|-------------------------------------------------|--------------------------------------|
| `cosmonaut-installer`          | `/usr/bin/cosmonaut-installer`                  | libcosmic GUI                        |
| `cosmonaut-installer-daemon`   | `/usr/libexec/cosmonaut-installer-daemon`       | DBus system service (root)           |
| `cosmonaut-installer-cli`      | `/usr/bin/cosmonaut-installer-cli`              | Headless recipe driver               |
| `cosmonaut-engine` (lib)       | linked into daemon                              | Install orchestrator                 |

## Build

```sh
just build           # local dev build (uses network)
just build-release   # sandboxed build (--frozen --offline; expects vendored deps)
just install rootdir=/some/staging prefix=/usr
```

## License

GPL-3.0-only.

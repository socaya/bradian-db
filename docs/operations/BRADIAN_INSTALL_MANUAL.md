# Bradian — manual installation guide (ADR 0105)

Covers the three OS paths at the source-build level. Production
packaging (signed installers, system-service integration,
upgrade channels) is OPEN per G1-INSTALL / G1-PORTABILITY and
requires §13 owner decisions.

## Common prerequisites

- Rust stable toolchain (currently 1.89+ per `rust-version`).
- Git.
- A directory that will host the Bradian data root
  (default: `${HOME}/.bradian`).

## Linux

```sh
git clone https://github.com/socaya/bradian-db.git
cd bradian-db
cargo build --release --bin bradian-db --bin bradian-admin
install -m 0755 target/release/bradian-db ~/.local/bin/
install -m 0755 target/release/bradian-admin ~/.local/bin/
```

The `bradian-db` binary is the canonical operator entry point.
`bradian-admin` is retained for legacy scripts per CLAUDE.md
§4.

**Known good**: Ubuntu 22.04+, Debian 12+, Fedora 40+.

## macOS

```sh
git clone https://github.com/socaya/bradian-db.git
cd bradian-db
cargo build --release --bin bradian-db --bin bradian-admin
install -m 0755 target/release/bradian-db /usr/local/bin/
install -m 0755 target/release/bradian-admin /usr/local/bin/
```

Code-signing + notarisation are OPEN. Running the unsigned
binary requires the Gatekeeper override:

```sh
xattr -d com.apple.quarantine /usr/local/bin/bradian-db
```

## Windows

```powershell
git clone https://github.com/socaya/bradian-db.git
cd bradian-db
cargo build --release --bin bradian-db --bin bradian-admin
Copy-Item target\release\bradian-db.exe "$env:LOCALAPPDATA\Programs\bradian-db\bradian-db.exe"
```

Authenticode signing + MSI/MSIX packaging are OPEN. The POSIX
advisory-lock semantics described in ADR 0092 §2 / the
fault-recovery extension are UNQUALIFIED on Windows's
`LockFileEx`; operators must not share a data root across
Windows processes until §13 advisory-lock verification lands.

## What is NOT installed

- System-service / systemd / launchd integration.
- Start-on-boot.
- Rotation / retention cron jobs.
- Operator UI auto-launch.
- Signed release artifacts.
- Automatic updates.

These remain G1-INSTALL / G1-PORTABILITY open work.

# ADR 0106 — Device-fault lab plan (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a planning deliverable.**

## 1. Honest scope

Shipped in-repo:
- `crates/bradian-relational/tests/device_fault_simulation.rs`
  — 4 black-box scenarios reproducible on any dev machine:
  - Truncation mid-WAL frame → recovery at last complete.
  - Zero-overwrite of WAL payload → Corruption.
  - Hole-punch (zeroed bytes mid-frame) → Corruption (POSIX).
  - Lock-file removal → single-writer invariant breaks per
    ADR 0092 §2 but data files survive.

## 2. Out of scope (PROCUREMENT)

Real device-level qualification requires a physical fault lab
+ §13 procurement. Items that CANNOT be reproduced in-repo:
- **Power-loss across fsync** — requires an actual power-
  interruptible test rig.
- **Cable-pull mid-write** — physical.
- **Disk ECC flips at the sector level** — requires fault-
  injecting block devices or storage emulators (e.g., LKVM
  with a drive-fault driver).
- **Multi-device crash consistency** — RAID failure modes.
- **File-system-specific guarantees** on ext4 / xfs / zfs /
  btrfs / NTFS / APFS.
- **Battery-backed cache write ordering** on HBAs.

## 3. Compliance

1. Simulation tests pass on Linux + macOS (4 tests).
2. POSIX-only hole-punch test is tagged `#[cfg(unix)]`.
3. Documented out-of-scope items match CLAUDE.md §2 "cross-
   platform recovery qualification remain incomplete".
4. Each in-repo scenario corresponds to a real-world fault
   pattern — the simulation is the cheapest honest proxy.

The published results of running these simulations become
part of the eventual G1-FAULTS evidence package once a real
lab is procured.

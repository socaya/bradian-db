# Master Gate 1 — Metadata I/O limits verification

13 November 2026. Baseline: `e61268afe0816c8b803487d68029397dc579d489`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**757 Rust tests passed, 0 failed, 0 ignored**, including **487 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and fixed-roadmap checks passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0, Node 24.22.1 and npm 11.9.0.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 757 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

Checkpoint manifests and native backup receipts use exact 40-byte arrays and one
trailing-byte probe. Invalid lengths remain corruption; missing checkpoint manifests
remain optional. Historical/lifecycle JSON archive manifests use MetadataLimits,
defaulting to 1 MiB of serialized input/output. Actual reads are bounded rather than
relying on file length. Oversized reads refuse before parse/component CRC/staging.

Canonical CRC calculation now streams through a checked byte counter. Manifest
serialization borrows reports/table lists and retains only a bounded envelope buffer;
its limit is checked before manifest-file creation. Native backup/restore APIs accept
explicit metadata policy, and existing APIs use defaults. A write refusal cannot
publish an archive its matching read policy would refuse. Formats/CRC bytes remain
compatible; source logical rows and prior incomplete-stage handling are preserved.

Five new unit tests cover exact/short/trailing fixed reads, the one-byte overflow
probe, zero/max input policies, interrupted fragmented readers, canonical escaping/
UTF-8/CRC equivalence, exact serialization boundaries, pre-extension refusal and
counter overflow. Six new integration tests cover short/trailing/1 GiB sparse binary
metadata, exact binary recovery, default sparse oversized JSON refusal before staging,
history and lifecycle archive write/read policy parity, exact-boundary backup/restore,
source row preservation, raised-policy retry, malformed JSON corruption and I/O error
classification. A source scan confirms remaining fs::read calls in the native relational
source are test fixtures, rather than these production metadata paths.

Existing checkpoint/index/build, corruption, crash/recovery, migration, backup,
SQL/history, analytical, CLI and HTTP suites still pass.

## Remaining scope

These are serialized metadata limits, not total RSS or decoded-object reservations.
Allocator/vector slack, report/catalog objects, parsed JSON structures, directory
inputs, archive payload copying/hashing, caches, aggregate transactions/pins, SQL JSON
output, alternate engines and global admission remain open. JSON serialization limits
do not roll back component checkpoints or remove incomplete write stages.

No OOM/allocation-failure, cancellation, failed I/O, disk-full, physical power-loss,
browser or macOS/Windows qualification is claimed. Gate 1 remains active; see the
[policy](../operations/METADATA_LIMITS.md), [ADR 0048](../adr/0048-bounded-archive-metadata.md)
and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).

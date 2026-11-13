# Master Gate 1 — Index recovery entry limits verification

13 November 2026. Baseline: `d3cf22c52e6a5fcf1eaa9ace7cb3e40ebf357e14`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**737 Rust tests passed, 0 failed, 0 ignored**, including **467 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and the fixed-roadmap check passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0, Node 24.22.1 and npm 11.9.0.
The workspace was restored from the published baseline before implementation.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 737 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

IndexLimits admits the retained checkpoint key/pointer vector before copying each
key, with defaults of 2000000 entries and 256 MiB of accounted entry bytes. Recovery
no longer creates an unused complete value map. It validates values and row structure
in their decoded physical page. Leaf reconciliation uses incremental traversal rather
than a second whole-index range result. Existing owned native range results retain
their contract. All expected point lookups are still checked against physical pointers.
Impossible leaf counts are checked against page capacity before vector reservation.

Native open_with_recovery_limits retains selected policy for missing-index recovery,
ordinary audits/rebuilds and backup verification. Explicit audit/rebuild methods accept
per-operation overrides; restore_backup_with_recovery_limits verifies its stage under
explicit policy. Existing APIs use defaults. Entry refusal precedes temporary-file
cleanup/build/publication and, during open, normal incomplete-WAL-tail repair.

Nine new integration tests cover exact entry/byte boundaries independent of large
checkpoint values, quota refusal without altering authoritative/index files or prior
temporary data, raised-policy retry/idempotence, zero-entry checkpoints, multi-level
binary-key traversal/rebuild, valid-CRC prematurely terminated leaf chains, impossible
leaf counts, invalid checkpoint value lengths, open refusal preserving a WAL tail and
backup/restore policy propagation before destination publication. One new unit test
checks count/byte overflow refuses instead of wrapping. Existing corruption, atomic
batch, index/range/cache, recovery, migration, backup, SQL/history, CLI, analytical
and HTTP tests still pass.

## Remaining scope

The limit charges key lengths and native key/pointer tuple headers, not allocator/
vector slack, one previous key, page/cache buffers or index-builder descriptors.
Checkpoint pointer construction, builder/checkpoint/metadata workspaces, higher-level
catalogs, global admission, aggregate transactions/pins, formatted output and alternate
engines remain open. No total RSS, cancellation, allocation-failure, disk-full, failed
I/O, physical power-loss, browser or macOS/Windows qualification is claimed.

Quota refusal is not an integrity certificate for unvisited data. Existing published
indexes are never silently repaired; incomplete staging keeps its previous explicit
cleanup/fresh-destination contract. backup_to may checkpoint before later backup
verification refuses. Gate 1 remains active; see the [policy](../operations/INDEX_RECOVERY_LIMITS.md),
[ADR 0046](../adr/0046-bounded-index-recovery-entries.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).

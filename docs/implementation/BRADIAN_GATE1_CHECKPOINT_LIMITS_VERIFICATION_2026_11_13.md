# Master Gate 1 — Checkpoint and index build admission verification

13 November 2026. Baseline: `33e856cadf92a540276b9915315b7e68a2fb108e`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.
Implementation began at `643fb5fd16ccf7d113996615a8d49c7387d7d2bf`; the two newer BACE documentation commits were then incorporated. Their diff is documentation-only, so tested runtime sources remain identical.

## Executed result

**746 Rust tests passed, 0 failed, 0 ignored**, including **476 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and fixed-roadmap checks passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0, Node 24.22.1 and npm 11.9.0.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 746 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

IndexBuildLimits defaults to 2000000 inputs and 512 MiB of conservative reservation:
key bytes, owned key/pointer headers and two borrowed descriptor headers per key.
Index separator descriptors now borrow stable input keys rather than cloning them.
Checkpoint admission visits live keys under the mutation mutex before quarantine,
temporary writes, generation publication or WAL truncation. Direct builder admission
precedes target creation; rebuild admission precedes prior temporary-file cleanup.
A quota error preserves authoritative state and healthy-owner usability.

StorageLimits composes row-map, index recovery and build policies. Native
open_with_policies retains them; backup verification carries source policies and
restore_backup_with_policies accepts destination policies. Existing APIs choose default
build policy. checkpoint_with_limits overrides only its operation. Existing current
checkpoints need no new build reservation. No policy is persisted.

Eight new integration tests exercise count/byte refusal before quarantine/temp/WAL/
manifest/usage changes, writable-owner recovery, exact reservation boundaries,
idempotent zero-budget retry, direct builder refusal before target creation, identical
index bytes under explicit/default policy, owner policy versus operation override,
automatic rebuild refusal preserving prior temporary data and incomplete WAL tail,
raised-policy recovery, multi-level binary-key index/rebuild equivalence, composed
backup/restore policy use without persisted limits and empty zero-budget builds.
One new unit test covers key-size and cumulative reservation overflow. Existing
checkpoint publication, corruption, index/range/cache, crash/recovery, migration,
backup, SQL/history, analytical, CLI and HTTP suites still pass.

## Remaining scope

The reservation is conservative accounted working state, not allocator reservation
or total RSS. Vector/container slack, fixed page/I/O buffers, source maps and caches
are separate. Metadata/directory input, higher-level catalogs, global admission,
aggregate transactions/pins, formatted output and alternate engines remain open.

Resource refusal occurs before mutating checkpoint/rebuild files; actual I/O failures
retain existing poisoning/fencing behavior. Current checkpoint reuse and valid-index
restore need no build reservation. This does not qualify allocation failure, OOM,
cancellation, disk-full, failed sync/rename, physical power loss, browser or macOS/
Windows operation. Gate 1 remains active. See the [policy](../operations/CHECKPOINT_BUILD_LIMITS.md),
[ADR 0047](../adr/0047-checkpoint-build-admission.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).

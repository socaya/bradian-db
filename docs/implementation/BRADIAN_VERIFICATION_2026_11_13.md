# Bradian executed verification — 13 November 2026

## Result

**588 passed, 0 failed, 0 ignored** across all Rust workspace features, including
**333 relational tests**. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. This supersedes the earlier G35
report for this date; earlier gate counts remain historical evidence.

Baseline: `f50bc77c5051f70f5829b7f992c472e89539338c` (published G35). G36–G38 add
26 tests: eight schema SQL integrations, ten recovery/integrity unit tests, six
legacy retirement integrations and two archive consistency integrations.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
Rust dependencies use the committed Cargo.lock with `--locked`. Studio dependencies
use the committed package-lock.json. Automatic CI remains manual-only.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 588 passed; no failed or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed; 99 modules transformed |
| `git diff --cached --check` | Passed before publication |

Cargo emits existing unused manifest dependency/field advisories. There are no
failing compiler or Clippy diagnostics. The Rust run includes the complete earlier
catalog, SQL, MVCC, migration, storage, recovery, analytical, security primitive,
adapter and HTTP suites.

## G36 — Actual nullable-column ALTER

The historical engine can execute ALTER TABLE ... ADD [COLUMN] ... [NULL] through
exclusive mutable ownership. A durable intent precedes catalog proposal/activation,
then a history activation commit becomes the new schema epoch. Reopening rolls
forward only exact interrupted states. Original pins and row bytes remain intact.
Legacy and epoch-tagged rows project through immutable schema timelines, and AS OF
binds SELECT columns to the schema that existed at the requested LSN.

Tests cover repeated revisions, multi-epoch CRUD, untouched legacy NULL projection,
invalid syntax/definitions without WAL changes, historical SELECT */DESCRIBE,
backup/restore, retention and independent catalog divergence. Recovery tests inject
interruptions at all four durable boundaries and truncate sixteen partial WAL tails.
They verify idempotent roll-forward, live-owner fencing, standalone adapter refusal,
no repair on divergent stores, forged intents/activation/retirement records,
immutable record deletion/rewrites and complete-WAL CRC corruption refusal.

## G37 — Explicit legacy intent upgrade

Inactive G28–G31 work may be explicitly retired without applying a proposal or
changing the catalog. The receipt binds its exact catalog horizon, full ordered
metadata digest and record count. Old records remain archival, legacy mutation APIs
are fenced, and G36 takes over the horizon through its checked schema chain.

Six integrations cover readiness/cancellation states, empty/duplicate refusal,
unchanged data/catalog, fresh ALTER after retirement, restored/pruned roots,
metadata tampering, five torn retirement-frame retries and catalog divergence.

## G38 — Archive metadata consistency

The G24 manifest now uses receipt statistics captured under the same MVCC mutation
lock as the archived bytes. Concurrent writes after capture cannot change that
manifest. Tests demonstrate stable archive statistics after source mutation and
three independently restored backups alongside continuous epoch-tagged SQL writes.
Restore verification also checks the catalog/history DDL horizon before publication.

DDL compatibility and retirement inspect only prefix-bounded metadata, avoiding
full row-history copies in those operations. Full retained payload validation
remains mandatory on reopen.

## Publication status

G35 is published as the baseline commit above. The G36–G38 implementation and
verification in this report are complete locally; publication of this batch to
the public repository is pending explicit approval after automatic review blocked
the upload. No deployment or automatic CI activation occurred.

## Remaining boundaries

See the [implementation gap register](BRADIAN_IMPLEMENTATION_GAPS.md). This batch
closes the local schema activation, historical decoding, legacy upgrade and archive
receipt gaps. It does not provide general SQL conformance, concurrent online DDL,
NOT NULL/default backfills, serializable isolation, production HTAP/distributed SQL,
complete enterprise identity/TLS or relational service/driver/Studio integration.

Fault injection used local WAL files and deterministic error hooks after durable
steps. The run does not certify physical device power loss, disk-full faults,
Windows/macOS recovery, long-duration workloads or browser interaction flows.

# Bradian executed verification — 12 November 2026

## Result

**550 passed, 0 failed, 0 ignored** across the Rust workspace with all declared
features enabled. Of these, **295** are in `bradian-relational`. Workspace compilation,
strict Clippy and Studio typecheck/build also passed. All commands below exited 0.

Environment: Debian 13 Linux x86_64, rustc/Cargo 1.100.0 stable, Node 24.22.1,
npm 11.9.0. Rust dependencies were resolved from the committed Cargo.lock with
`--locked`. Studio's newly committed package-lock.json records the tested dependency
resolution. Automatic GitHub Actions triggers remain disabled.

| Check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 550 passed; no failures/ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm ci --no-audit --no-fund` in `apps/dap-ui` | Passed; tested lockfile reproduced |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed; 99 modules built |

Cargo 1.100 emits unused manifest dependency/field advisories. There are no failing
Rust compiler or Clippy diagnostics. Studio's build is static verification; there is
no browser interaction test suite in this result.

## Corrections made during execution

- Fixed the existing backup descriptor checksum encoder's overlapping mutable and
  immutable borrow; compute the CRC before assigning its bytes.
- Repaired the historical corruption test's private encoder import by selecting the
  seeded persisted key through the public transaction scan and typed decoder.
- Corrected G28's backup/restore assertion: the seeded movements table has three
  columns. The test now compares the full original definition after restoration.
- Removed unused imports and needless borrows, made lock/WAL preservation explicit
  with `truncate(false)`, introduced descriptive snapshot/index type aliases, and
  applied equivalent map iteration/entry updates for strict Clippy.
- Updated fixed-width analytical codec iteration to checked array chunks and
  equivalent divisibility checks. Existing codec and round-trip suites passed.
- Added G34 revision decisions and eleven tests, including process exit, concurrent
  decisions, backup/restore, stale receipt rejection and twelve torn-frame cases.

## Exercised components

The run includes catalog validation, page checksums, WAL persistence, checkpoint
publication, derived B+tree repair, cache behavior, backup/restore, typed CRUD,
single/multi-table transactions, SQL rollback and conflict checks, lifecycle recovery,
offline migrations, durable MVCC and retention, historical SQL/schema inspection,
G28–G31 intents/cancellation/fences/readiness, G32 epoch codec, G33 proposals,
G34 decisions, analytical storage/operators, planning, ingestion, views, adapters,
federation primitives, security primitives and HTTP request/compatibility tests.

## Limits and remaining implementation

Passing existing tests is not evidence that every master-SRS requirement exists.
Coordinated catalog/history schema activation, live SQL ALTER, mixed-epoch live SQL
writes, serializable isolation, production distributed SQL/HTAP, complete enterprise
identity/TLS and operational recovery certification remain open. G34 changes catalog
metadata only. Cross-platform runs, actual device power loss, disk-full faults,
long-running soak tests and production workload benchmarks were not executed.

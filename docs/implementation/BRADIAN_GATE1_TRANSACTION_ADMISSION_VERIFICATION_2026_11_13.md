# Master Gate 1 — Historical transaction admission verification

13 November 2026. Baseline: `a6a914bdff8b8402424dfede9cc187c1c823105e`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**772 Rust tests passed, 0 failed, 0 ignored**, including **497 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and fixed-roadmap checks passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0, Node 24.22.1 and npm 11.9.0.
The checkout/toolchain were restored from published main before implementation.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 772 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

TransactionLimits defaults to 128 active handles per durable history owner. SQL BEGIN
and BEGIN WITH READ VALIDATION use fallible typed/raw admission. All existing pin
references, including same-LSN handles and administrative pins, are counted with
checked arithmetic under the same state mutex that creates the new pin. Refusal
adds no pin/staged state/WAL record and does not fence the owner.

Native durable/typed try_begin variants and open_with_resource_limits expose explicit
policy. Existing open APIs use defaults. CLI --max-active-transactions requires
--history. Zero can open a valid owner but refuses SQL/fallible BEGIN. Existing pin
cleanup returns capacity on completion/error/drop/unwind. Savepoint recovery preserves
its outer slot and the original retention/conflict behavior.

Six new native tests cover same/different snapshot horizons, unchanged state/WAL on
refusal, rollback/drop/read-only/validating/write completion, commit conflicts and
retained-history quota errors, retention protection, sixteen concurrent callers
competing for four slots, independent domains, nonpersisted policy and administrative
compatibility pins. Three SQL tests cover zero-policy refusal, nested sessions sharing
capacity, failed savepoint ownership/rollback, commit conflicts, statement abort and
callback unwind without leaks. One new unit test covers pin-count/next-slot overflow.
Existing CLI process tests now exercise history-only flag rejection, zero refusal
and positive validating-BEGIN admission. Earlier SQL, analytical, recovery, corruption,
backup/migration, CLI and HTTP suites still pass.

## Remaining scope

The existing infallible native BEGIN variants remain trusted administrative
compatibility APIs and do not enforce admission. Their pins count when subsequent
SQL/fallible calls check capacity, but they can exceed policy themselves. SQL has no
such fallback. Other engines and standalone nontransactional reads remain separate.

The limit counts handles per domain. It does not reserve total bytes, cap all native
callers or provide a process-wide/cross-domain pool. Catalog/decoded state, allocator
slack, other output paths and global admission remain open. No cancellation, OOM,
failed-I/O, physical power-loss, browser or macOS/Windows qualification is claimed.
Gate 1 remains active; see the [policy](../operations/HISTORY_TRANSACTION_ADMISSION.md),
[ADR 0050](../adr/0050-historical-transaction-admission.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).

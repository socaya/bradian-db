# Bradian Database Systems v1.1 — source audit and implementation roadmap

**Baseline:** `docs/Bradian_Master_Architecture_SRS_v1.1.md`. **Reviewed source:** the original Rust DAP analytical workspace. This document reports implementation status, not a completed DBMS.

## Current roadmap reconciliation (13 November 2026)

Use the [fixed master Gate 0–9 sequence](BRADIAN_MASTER_ROADMAP.md),
[master requirement matrix](BRADIAN_MASTER_TRACEABILITY.md),
[source/compatibility inventory](BRADIAN_SOURCE_INVENTORY.md) and
[threat model](../security/BRADIAN_THREAT_MODEL.md). The numbered BRD-G records below
are historical implementation slices. They are not master gate acceptance.
The canonical local SQL command addresses a Gate 1 task; Gate 1 remains open for
runtime, identity and failure/platform qualification. Current test/installer evidence
is in [Gate 1 local CLI verification](BRADIAN_GATE1_LOCAL_CLI_VERIFICATION_2026_11_13.md).
All dated sections below retain their original scope rather than asserting current
absence of features subsequently implemented.

## Updated implementation status (13 November 2026)

The implemented relational engine provides a persistent catalog, typed WAL rows,
immutable B+tree checkpoints, read-only caching, atomic batches and bounded SQL
transactions. Opt-in durable MVCC adds historical SQL, schema epoch row envelopes,
verified migrations and backup/restore. G36 now provides exclusive nullable-column
ALTER with an intent-driven catalog/history recovery protocol, immutable schema
history and schema-aware AS OF. G37 explicitly retires inactive legacy DDL work;
G38 captures backup receipts/statistics atomically as SQL writers advance.

These are bounded local implementations. They do not establish general SQL
conformance, serializable isolation, concurrent online DDL, mutable physical page
management, production HTAP/distributed SQL or complete enterprise security.
See the [gap register](BRADIAN_IMPLEMENTATION_GAPS.md), current gates and
[executed verification](BRADIAN_VERIFICATION_2026_11_13.md). Automatic CI remains
manual-only by owner request.

## G39 constraint evolution (13 November 2026)

The historical engine now supports non-key DROP NOT NULL through the exclusive G36
recovery protocol and NULL for omitted nullable INSERT fields. Older schema epochs,
original row bytes and the G33 additions-only proposal contract are preserved. The
[G39 verification report](BRADIAN_G39_VERIFICATION_2026_11_13.md) records 600 passing
workspace tests, including 345 relational tests, plus strict Clippy and Studio checks.
SET NOT NULL/backfill and non-NULL defaults remain open.

## Executed verification and catalog revisions (12 November 2026)

The [executed verification report](BRADIAN_VERIFICATION_2026_11_12.md) records 550 passing
Rust tests (295 relational), a passing all-feature workspace check, strict Clippy across
all targets/features, and passing Studio typecheck/build. Earlier statements that builds
could not run are historical. G34 adds WAL-backed catalog-only schema activation and
cancellation with exact proposal identity, replay validation and partial-write recovery.
Live ALTER across catalog and historical row WALs remains unimplemented; schema-pin
mismatches fail closed. These Linux checks do not certify production or platform power loss.

## Live epoch row integration (13 November 2026)

G35 connects the G32 codec to actual G23 SQL reads and writes after an explicit
one-way capability decision in the history WAL. Mixed BRRW/BRVE rows remain
queryable through live sessions and AS OF, backup/restore and restart. Each new
BRVE row uses the exact original schema-registration epoch; table definitions
remain immutable. This is a prerequisite for historical schema activation,
not an implementation of coordinated catalog/history ALTER. The
[13 November verification report](BRADIAN_VERIFICATION_2026_11_13.md) records the
new regression and recovery evidence.

## Schema activation, upgrade and backup gaps closed (13 November 2026)

G36–G38 close the previously recorded activation, mixed-schema decoding, AS OF
binding, interrupted transition recovery, legacy intent upgrade and archive-statistic
race gaps. The final executed result is 588 passing Rust tests, including 333
relational tests, plus strict Clippy, workspace compilation and Studio checks.
Earlier dated notes below describe the implementation at their original gate;
current capability boundaries are maintained in the gap register.

## Original audit baseline

The repository contains a proprietary analytical column format (`.derc`), WAL-backed analytical partitions, vectorized operators, DHIS2 semantic modules, an Axum analytical API, a React analytics administration interface, and test suites. It does **not** contain a verified independent general-purpose authoritative relational database with complete SQL DDL/DML, B+tree row pages, ACID/MVCC transactions, cross-shard atomic commits, distributed consensus, or strongly consistent HTAP projections.

Existing DAP traceability DONE statuses refer to analytical requirements only; they do not fulfill Bradian v1.1 transactional and enterprise requirements.

## Phase 0: secure and rename product-facing identity

Preserve legacy `dap-*` crates, Rust imports, `.derc` on-disk identifiers, DHIS2 wire compatibility and previously persisted objects. Present the product as **Bradian Database Systems**, CLI as `bradian-db`, and administration as **Bradian Studio**. Introduce a fail-closed, **loopback-only** bootstrap API administrator token. This is *not* full end-user authorization. Native TLS/mTLS, scoped principals and Studio login are release blockers.

**Proof required:** all protected API paths reject missing/invalid tokens; liveness remains public; full workspace tests, Rust lint and frontend typecheck pass before merge. Always re-test integrations because unauthenticated DHIS2 clients are intentionally rejected until their credentials are configured.

## Phase 1: native relational kernel (P0)

Add a generic versioned database/schema/table catalog separate from DHIS2 UID semantics. Specify portable row page layout, page IDs, B+tree indexes, transaction identities, stable SQL types, write-ahead log recovery, buffer management and constraints. Implement `CREATE TABLE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE`, `BEGIN`, `COMMIT`, `ROLLBACK`, primary/foreign keys and tested crash durability as incremental executable slices. Design formats and ADRs before writing bytes.

## Phase 2: ACID/MVCC and SQL correctness (P0)

Implement transaction visibility, isolation and locking, uniqueness races, phantom/write-skew detection under claimed isolation, savepoints, schema migrations and SQL conformance fixtures. Require concurrent schedule testing and differential result comparisons, not just CRUD smoke tests.

## Phase 3: authoritative HTAP (P0)

Only committed transactional state can publish columnar projections. Define commit-position, transaction-consistent row/column snapshots, delta replay, staleness modes and permission propagation. Reuse the existing analytical executor and segment code where correctness remains valid.

## Phase 4: distributed SQL and auto scaling (P1)

Implement trusted cluster membership, replicated log and quorums, shard placement, leader fencing, multi-gateway routing, online splitting/rebalancing and durable cross-shard commit/recovery. Keep autonomous geographic federations separate from strong clusters. Fault-inject network partitions, duplicated RPCs and coordinator failure.

## Phase 5: high-assurance security and operations (release blockers)

Implement identity provider, MFA-capable Studio sessions, SQL-level per-principal authorization, tenant separation, storage/WAL/backup encryption, managed key lifecycle, TLS/mTLS, privileged audit and incident response. Add cluster-consistent PITR, restore rehearsal, driver conformance and Windows/Linux/macOS CI before production qualification.

## Compatibility and security notes

The original analytics-only API and file formats are not to be bulk-renamed. Existing analytical namespaces are transitional until explicit migration tests. The bootstrap token environment variable is `BRADIAN_API_TOKEN`, and its presence cannot justify deployment on a public network. Database-admin permissions, direct query authorization, token revocation/expiry, browser login and secure internal service credentials are **not implemented** by the bootstrap layer.

## Verification commands

```sh
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/dap-ui
npm ci || npm install
npm run typecheck
npm run build
```

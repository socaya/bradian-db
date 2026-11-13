# Bradian master requirements traceability

Every requirement ID from Master SRS v1.1 Sections 20 and 31 appears exactly once.
Gate numbers refer to the fixed Section 32 roadmap. PARTIAL means source/evidence
covers a subset; INHERITED refers to the older analytical subsystem; OPEN means no
accepted implementation. No row claims complete production acceptance. Relevant
source links are an evidence starting point, not proof that the whole requirement
passes. See the current verification and fixed roadmap for executed scope.

| Requirement | Master gate | Status | Scope | Relevant source | Remaining acceptance |
|---|---|---|---|---|---|
| BRD-REL-001 | 1 | PARTIAL | Native databases, schemas, relational tables and stable catalog IDs | [Source](../../crates/bradian-relational/tests/catalog.rs) | Persistent native catalog exists; complete server/driver metadata contract remains open |
| BRD-REL-002 | 2 | PARTIAL | PK, FK, UNIQUE, CHECK, defaults and referential actions | [Source](../../crates/bradian-relational/tests/defaults_sql.rs) | PK/defaults enforced in supported paths; secondary/FK/CHECK and referential actions remain open |
| BRD-REL-003 | 1 | PARTIAL | DDL, DML, prepared statements and documented SQL subset | [Source](../../crates/dap-admin/tests/local_sql_cli.rs) | Native bounded DDL/DML and CLI; prepared statements/general SQL/reference conformance remain open |
| BRD-REL-004 | 2 | PARTIAL | Typed JSON, decimals, timezones and correct NULL semantics | [Source](../../crates/bradian-relational/tests/update_expressions.rs) | Typed decimals/NULL subset; JSON, temporal SQL/coercions and full golden corpus remain open |
| BRD-TXN-001 | 1 | PARTIAL | ACID, commit acknowledgement, rollback and savepoints | [Source](../../crates/bradian-relational/tests/savepoint_crash.rs) | Local atomicity/rollback/savepoints tested; device/I/O/platform qualification remains open |
| BRD-TXN-002 | 1 | PARTIAL | MVCC snapshots and implemented isolation levels | [Source](../../crates/bradian-relational/tests/read_validation.rs) | Opt-in snapshot/read validation only; full documented isolation levels remain open |
| BRD-TXN-003 | 1 | PARTIAL | Deadlock handling, lock timeouts and revision GC | [Source](../../crates/bradian-relational/tests/durable_mvcc.rs) | Pinned retention exists; lock waits/deadlines/deadlock handling and stress qualification remain open |
| BRD-STO-001 | 1 | PARTIAL | Native row pages, primary/secondary indexes and checksums | [Source](../../crates/bradian-relational/tests) | Mutable pages/allocator and platform failure matrix |
| BRD-STO-002 | 3 | INHERITED | Versioned native columnar segments and atomic manifests | [Source](../../crates/dap-storage) | Existing .derc analytical segments; authoritative transactional projection is not integrated |
| BRD-STO-003 | 1 | PARTIAL | WAL, checkpoints, restore, PITR and snapshot consistency | [Source](../../crates/bradian-relational/tests/backup_restore.rs) | Verified local WAL/checkpoint/restore subset; PITR and global snapshot consistency remain open |
| BRD-OLAP-001 | 3 | INHERITED | Vectorized and parallel SQL operators with bounded spills | [Source](../../crates/dap-runtime) | Reference/rewrite, statistics/spill and production workload qualification |
| BRD-OLAP-002 | 3 | INHERITED | CBO, statistics, EXPLAIN and plan equivalence | [Source](../../crates/dap-runtime) | Reference/rewrite, statistics/spill and production workload qualification |
| BRD-OLAP-003 | 3 | INHERITED | Incremental views with edits, deletes and replay safety | [Source](../../crates/dap-runtime) | Reference/rewrite, statistics/spill and production workload qualification |
| BRD-HTAP-001 | 3 | OPEN | Coherent committed snapshots across row/column stores | [Source](../../crates/dap-views) | Authoritative committed row/column watermarks and permission-equivalent snapshots |
| BRD-HTAP-002 | 3 | OPEN | Strong, bounded-staleness and latest-safe query contracts | [Source](../../crates/dap-views) | Authoritative committed row/column watermarks and permission-equivalent snapshots |
| BRD-HTAP-003 | 3 | OPEN | OLTP resource protection during concurrent analytics | [Source](../../crates/dap-views) | Authoritative committed row/column watermarks and permission-equivalent snapshots |
| BRD-CL-001 | 4 | OPEN | Secure bootstrap, join, membership and leader fencing | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-CL-002 | 4 | OPEN | Native multi-gateway routing and safe retry semantics | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-CL-003 | 4 | OPEN | Sharding, replicated write quorum and online rebalancing | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-CL-004 | 5 | OPEN | Distributed transaction and coordinator recovery | [Source](../../crates/dap-distributed) | No proven cross-shard atomic commitment/coordinator recovery contract |
| BRD-FED-001 | 7 | OPEN | Independent site operation and authorized federation | [Source](../../crates/dap-distributed) | Autonomous identities, authorized deltas, residency and conflict/replay proof |
| BRD-FED-002 | 7 | OPEN | Data residency and version-aware conflict resolution | [Source](../../crates/dap-distributed) | Autonomous identities, authorized deltas, residency and conflict/replay proof |
| BRD-SEC-001 | 1 | PARTIAL | Authentication, MFA-capable privileged login and secure sessions | [Source](../../docs/security/BOOTSTRAP_API_AUTH.md) | Bootstrap token/private CLI mode checks only; complete local identity and MFA/session design remain open |
| BRD-SEC-002 | 2 | PARTIAL | RBAC/ABAC, row/column/tenant policies end-to-end | [Source](../../crates/dap-security/src/authz.rs) | Policy helper exists; authoritative row/column/tenant enforcement across all paths remains open |
| BRD-SEC-003 | 4 | PARTIAL | TLS/mTLS and managed at-rest encryption | [Source](../../crates/dap-security/src/encryption.rs) | Crypto primitives exist; native transport/mTLS and storage key-custody integration remain open |
| BRD-SEC-004 | 8 | PARTIAL | Versioned secrets, keys, rotation and revocation | [Source](../../crates/dap-security) | Integrated identity, policy, transport, custody and negative-path qualification |
| BRD-SEC-005 | 8 | PARTIAL | Tamper-evident audit and protected export | [Source](../../crates/dap-security) | Integrated identity, policy, transport, custody and negative-path qualification |
| BRD-SEC-006 | 8 | PARTIAL | Fuzzed parsers, plugin isolation and secure host defaults | [Source](../../crates/dap-serving/tests/http_roundtrip.rs) | Bootstrap negative requests tested; parser fuzz/plugin isolation/SAST/DAST qualification remains open |
| BRD-SEC-007 | 8 | PARTIAL | Verified incident/backup/recovery security workflows | [Source](../../crates/dap-security) | Integrated identity, policy, transport, custody and negative-path qualification |
| BRD-STU-001 | 6 | PARTIAL | Browser database explorer, data editor, SQL workbench | [Source](../../apps/dap-ui) | Authenticated relational workflow and operational/browser E2E proof |
| BRD-STU-002 | 6 | PARTIAL | Cluster, security, backup and performance consoles | [Source](../../apps/dap-ui) | Authenticated relational workflow and operational/browser E2E proof |
| BRD-API-001 | 6 | PARTIAL | Versioned SQL drivers and secure administrative APIs | [Source](../../crates/dap-serving) | Relational drivers, secure sessions and versioned contract/load proof |
| BRD-OPS-001 | 1 | PARTIAL | Observability, limits, cancellation and upgrade support | [Source](../../crates/dap-admin) | Query, retained-history, staging and backing live/delta map limits exist; metadata/index/checkpoint workspace, catalog/global transaction admission, complete cancellation, upgrades and reliability workloads remain open |
| BRD-PORT-001 | 1 | PARTIAL | Native Windows/Linux/macOS profiles and storage portability | [Source](../../crates/dap-admin/tests/local_sql_cli.rs) | Windows/macOS installer, file-lock and recovery qualification |
| BRD-EXT-001 | 9 | PARTIAL | Controlled plugins and optional DER/DHIS2 adapter | [Source](../../crates/dap-dhis2) | Capability-controlled generic extension lifecycle and isolation proof |
| BRD-DTX-001 | 5 | OPEN | Atomic and recoverable cross-shard transaction decision | [Source](../../crates/dap-distributed) | Durable cross-shard decision, safe time, idempotent outcomes and crash/partition matrix |
| BRD-DTX-002 | 5 | OPEN | Global snapshot ordering and documented safe-time model | [Source](../../crates/dap-distributed) | Durable cross-shard decision, safe time, idempotent outcomes and crash/partition matrix |
| BRD-DTX-003 | 5 | OPEN | Idempotent outcome lookup and retry classifications | [Source](../../crates/dap-distributed) | Durable cross-shard decision, safe time, idempotent outcomes and crash/partition matrix |
| BRD-STO-004 | 1 | PARTIAL | WAL/page-LSN/checkpoint and torn-page protocol | [Source](../../crates/bradian-relational/tests/checkpoint_manifest.rs) | Torn-tail/manifest tests exist; failed sync/disk-full and physical-device proof remain open |
| BRD-STO-005 | 1 | PARTIAL | Crash-safe B+tree split, allocator and page reuse | [Source](../../crates/bradian-relational/tests/btree_index.rs) | Immutable B+tree checkpoints exist; in-place split/free-page reuse protocol remains open |
| BRD-STO-006 | 1 | PARTIAL | Verified format migration and corruption refusal | [Source](../../crates/bradian-relational/tests/legacy_migration.rs) | Explicit verified migration/corruption tests; reader fuzz/platform matrix remains open |
| BRD-SEC-008 | 2 | OPEN | Verified context across SQL, RPC, caches and async jobs | [Source](../../docs/security/BRADIAN_THREAT_MODEL.md) | No proven end-to-end verified context across SQL/RPC/cache/async execution |
| BRD-SEC-009 | 2 | PARTIAL | Safe diagnostics, inference and aggregate disclosure controls | [Source](../../crates/dap-admin/tests/local_sql_cli.rs) | CLI avoids SQL echo; role-aware metadata/plan/inference disclosure controls remain open |
| BRD-SEC-010 | 8 | PARTIAL | Separation of privileged administrator duties | [Source](../../crates/dap-security) | Integrated identity, policy, transport, custody and negative-path qualification |
| BRD-SEC-011 | 8 | PARTIAL | Revocation propagation and bounded enforcement delay | [Source](../../crates/dap-security) | Integrated identity, policy, transport, custody and negative-path qualification |
| BRD-SEC-012 | 9 | OPEN | Release-blocking security controls and key rotation | [Source](../../docs/security/BRADIAN_THREAT_MODEL.md) | Independent penetration review, release controls and key-rotation drills remain open |
| BRD-CL-005 | 4 | OPEN | Desired-state controllers, admission, epochs and fenced placement | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-CL-006 | 4 | OPEN | Safe online rebalance and replica lifecycle | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-CL-007 | 4 | OPEN | Gateway session/transaction routing and safe retries | [Source](../../crates/dap-distributed) | Secure consensus, identity/fencing, placement and adversarial failure proof |
| BRD-DDL-001 | 2 | PARTIAL | Recoverable multi-version online DDL state machine | [Source](../../crates/bradian-relational/src/schema_activation.rs) | Concurrent online jobs, leases and old-writer/lifecycle qualification |
| BRD-SQL-005 | 2 | PARTIAL | Versioned SQL profile, metadata and stable error classes | [Source](../../crates/bradian-relational/tests) | Driver metadata/error contracts, broader type/coercion/collation conformance |
| BRD-SQL-006 | 2 | PARTIAL | Collation, temporal, decimal and NULL semantics | [Source](../../crates/bradian-relational/tests) | Driver metadata/error contracts, broader type/coercion/collation conformance |
| BRD-JOB-001 | 6 | OPEN | Durable job state and idempotent resume | [Source](../../crates/dap-admin) | Durable jobs, cancellation, retry and restart idempotency |
| BRD-MIG-001 | 6 | PARTIAL | Compatibility assessment and migration validation | [Source](../../crates/bradian-relational/src/history_migration.rs) | External source assessment/CDC, resumable copy and reversible cutover |
| BRD-MIG-002 | 6 | PARTIAL | Source CDC catch-up and recoverable cutover | [Source](../../crates/bradian-relational/src/history_migration.rs) | External source assessment/CDC, resumable copy and reversible cutover |
| BRD-DR-001 | 8 | PARTIAL | Cluster-consistent backup and PITR | [Source](../../crates/bradian-relational/src/history_backup.rs) | Cluster-consistent PITR, authenticated immutable archives and key/region recovery |
| BRD-DR-002 | 8 | PARTIAL | Immutable backup, isolated restore and safe regional promotion | [Source](../../crates/bradian-relational/src/history_backup.rs) | Cluster-consistent PITR, authenticated immutable archives and key/region recovery |
| BRD-OPS-007 | 3 | OPEN | Global resource reservations and fair scheduling | [Source](../../crates/dap-runtime) | No enforced global mixed-workload reservations/fairness acceptance proof |
| BRD-OPS-008 | 9 | PARTIAL | Query regression and workload replay | [Source](../../crates/dap-benchmark) | Inherited benchmarks; seeded mixed-workload replay and external reproducibility remain open |
| BRD-EXT-002 | 9 | PARTIAL | Safe specialized index/type extensions | [Source](../../crates/dap-dhis2) | Capability-controlled generic extension lifecycle and isolation proof |

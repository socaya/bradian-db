# Bradian source inventory and compatibility map

Baseline: `388acc33340045b4b3ce99283c5785145c07e71f`. This inventory is a Gate 0
evidence artifact. Module existence is not evidence of full requirement completion.
The fixed roadmap and master traceability matrix govern subsequent work.

## Current ownership and evidence

| Source | Implemented responsibility | Important boundary |
|---|---|---|
| `bradian-relational` | Native catalog, typed rows, WAL, immutable checkpoint B+tree, caching, bounded SQL and opt-in durable historical transactions | Query/history/staging, backing/recovery maps and checkpoint/build reservations are bounded; metadata/I/O/cache workspace, catalog/aggregate transaction/global admission and alternate engines remain open; no production isolation qualification |
| `dap-admin` | Operator executable, canonical bradian-db wrapper, native local SQL scripts and existing analytical administration | Local SQL is not remote SQL; legacy analytical commands retain their existing behavior |
| `dap-core`, `dap-catalog` | Inherited analytical schemas, batches, predicates and metadata adapters | Distinct from the authoritative relational catalog |
| `dap-storage` | Inherited .derc segments, partition WAL, snapshots, compression and compaction | Not automatically a projection of relational commits |
| `dap-runtime`, `dap-planner`, `dap-views` | Analytical operators, planning and incremental data products | Full CBO/spill/permission/coherent HTAP requirements remain incomplete |
| `dap-serving` | Axum analytical/DHIS2 routes with loopback bootstrap token restriction | Not a native relational wire service or complete identity/session boundary |
| `dap-security` | Policy helpers, token, audit and standard-library-backed crypto primitives | Primitive tests do not demonstrate complete end-to-end enforcement or key custody |
| `dap-distributed`, `dap-sources` | Inherited distribution/source integration components | Not proven consensus, shard ownership, distributed ACID or secure membership |
| `dap-dhis2`, optional integration apps | Domain-specific adapters and analytical contracts | Must remain optional to the generic database kernel |
| `apps/dap-ui` | Bradian Studio analytical/pre-release interface | No claim of authenticated general relational database administration |
| `tests/dap-e2e`, crate test directories | Executed regression, recovery and analytical result fixtures | Linux synthetic evidence is not physical power-loss/platform certification |

## Compatibility commitments

| Format/interface | Owner and version | Upgrade rule |
|---|---|---|
| BRPG row pages | relational page codec | Preserve version/checksum checks; do not reinterpret invalid pages |
| BRWL row WAL | relational row store | Replay complete CRC/sequence-validated frames; only incomplete tail repair is automatic |
| BRIX checkpoint index | relational B+tree | Immutable derived index; missing-index reconstruction must verify authoritative pages |
| BRRW typed rows | relational typed table | Existing rows decode under their source schema; no implicit migration |
| BRVE epoch rows | relational epoch codec, version 1 | Explicit capability enablement; exact commit-time epoch validation |
| BRADIAN-DURABLE-MVCC-V1 | durable history namespace | Separate format from legacy/shared-only row stores; explicit verified migration |
| Schema pin/catalog JSON | persistent catalog and history adapters | Empty default metadata preserves legacy bytes; unknown semantic commands/versions fail closed |
| BRBKv001 and BRADIAN-HISTORY-BACKUP-V1 | native backup implementations | Receipt/manifest verification and staged new-root restore; CRC is not authentication |
| .derc, dap-* crate names, analytical paths | inherited analytical subsystem | Preserve identifiers and wire compatibility until a separately tested migration exists |
| dap-admin / bradian-db | operator command library | Legacy executable retained; canonical executable added; SQL output is a documented local JSON profile |

HistoryLimits and HistoryUsage in runtime_limits expose process-local retained-history policy/counters; durable MVCC, typed/SQL adapters, migration and restore propagate explicit limits. Policy is not persisted and no format or dependency changed. See [ADR 0043](../adr/0043-retained-history-admission.md).

RowStoreLimits/RowStoreUsage add independent live-row and delta map admission to open, replay and writes. Backup inherits source policy; native restore accepts explicit policy. Default higher-level SQL/history opens use backing defaults. No format or dependency changed; see [ADR 0045](../adr/0045-backing-row-map-admission.md).

IndexLimits separately admits retained checkpoint key/pointer entries for audit/rebuild. Recovery validates values without cloning them, and compares leaves incrementally. Explicit native open/restore and backup verification carry selected recovery policy. No format or dependency changed; see [ADR 0046](../adr/0046-bounded-index-recovery-entries.md).

IndexBuildLimits preflights conservative key/pointer and borrowed descriptor working state before checkpoint/rebuild file changes. StorageLimits composes row, recovery and build policy for native owners and backup/restore. Index bytes and all persistent formats remain unchanged; see [ADR 0047](../adr/0047-checkpoint-build-admission.md).

## Architecture decisions still requiring prototypes

| Decision | Evidence required before adopting/fixing a format | Owning gate |
|---|---|---|
| Mutable pages/allocator and dirty buffer writes | WAL/page-LSN ordering, split/reuse crash matrix and failed sync/rename/disk-full tests | 1 |
| Runtime quotas/cancellation | Query/history/staging, backing/recovery maps and checkpoint/build reservations are implemented; metadata/I/O/cache workspace, catalog/aggregate transaction/global limits, interruption cleanup and memory-pressure workload remain | 1 |
| Secure local identity and SQL policy context | ACL/root-owner/path threat analysis, every data path checked, revocation and diagnostics tests | 1–2 |
| Global transaction order/safe-time | Deterministic adversarial multi-participant schedules, clock uncertainty and coordinator recovery | 5 |
| Cluster node identity and consensus | Authenticated enrollment/fencing, quorum-loss and partition experiments | 4 |
| Authoritative HTAP projection | Same committed revision across row/column reads, retractions, lag/fallback and access-context propagation | 3 |
| Disaster/key recovery | Managed keys, encrypted archives, immutable copies and verified legal-cut restore | 8 |

Current kernel crash/rollback/read-validation experiments and bootstrap HTTP tests
are useful controlled evidence. They do not satisfy the future global-order or
complete authorization experiments above. Those protocols remain unfrozen and
unsupported; no placeholder may be presented as a production implementation.

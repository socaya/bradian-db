# Bradian fixed master roadmap

The gate IDs in Master SRS v1.1 Section 32 are fixed. Section 18 contains an earlier,
broader summary with different groupings; Section 32 is the working sequence because
it specifies the detailed dependency and exit conditions. ADR 0040 records this
reconciliation without rewriting historical specifications or implementation files.

BRD-G1 through BRD-G44 are historical implementation records, not 44 accepted master
gates. Their tests remain useful evidence. No new gate number is assigned to each
commit. New changes identify a task in the current master gate. Passing a subset of
tests does not accept a complete master requirement or permit skipping its exit checks.

| Master gate | Fixed scope | Current position |
|---|---|---|
| 0 | Evidence, code audit, risk and specification closure | Evidence baseline assembled; unresolved architecture decisions remain explicit blockers before their formats/protocols can be frozen |
| 1 | Portable kernel and correct single-node transaction | Active implementation; native local SQL command delivered; memory, identity and failure qualification still open |
| 2 | Relational compatibility and safe schema lifecycle | Not accepted; existing schema/SQL slices are supporting evidence only |
| 3 | Columnar engine and commit-consistent HTAP | Not accepted; inherited analytical engine is not an authoritative HTAP projection |
| 4 | Single-cluster replication and safe placement | Blocked on earlier gates and secure identity/protocol decisions |
| 5 | Distributed commit and global snapshot correctness | Blocked on proven replication and global-ordering prototype |
| 6 | Studio, durable operations and migration | Not accepted; existing UI and local migrations are partial evidence |
| 7 | Federation and site autonomy | Blocked on explicit installation identities and delta/conflict contracts |
| 8 | Cluster-consistent recovery and hardening | Blocked on complete trust, storage and replication designs |
| 9 | Qualification and external reproducibility | Not accepted; no production or cross-platform certification claimed |

## Gate 0 evidence baseline

The [source inventory and compatibility map](BRADIAN_SOURCE_INVENTORY.md),
[complete master requirement matrix](BRADIAN_MASTER_TRACEABILITY.md),
[threat model](../security/BRADIAN_THREAT_MODEL.md), ADRs and executed verification
reports supply the previously missing auditable baseline. The legacy analytics
traceability file remains explicitly scoped to the inherited product.

Current page/WAL/backup experiments are demonstrated in page codec, atomic batch,
checkpoint, process-exit, corruption, durable MVCC, catalog activation and backup
tests. HTTP tests demonstrate only the bootstrap token boundary. They do not prove
SQL authorization propagation, a secure cluster or global transaction ordering.
Those uncertainties remain design blockers, with prototype/evidence requirements in
the threat model and source inventory. No distributed timestamp, cluster identity
or authorization wire format may be frozen from the current stubs/partial modules.

Automatic CI remains disabled by the owner's instruction. Manual workspace checks
are the evidence path; this is an explicit exception to the earlier baseline-CI
wording, not an unreported skipped check. Gate 0's evidence baseline is reviewable;
this does not sign off unresolved architecture decisions or production security.

## Gate 1 task order and acceptance

| Stable task | Status | Evidence or remaining work |
|---|---|---|
| G1-CATALOG | Implemented subset | Persistent catalog and schema/row validation tests; unsupported secondary/FK constraints remain explicit |
| G1-STORAGE | Implemented subset | BRPG/BRWL codecs, immutable B+tree checkpoints, CRCs and exclusive store locks |
| G1-TRANSACTION | Implemented subset | WAL atomicity, rollback, snapshots, conflict tests, savepoints and read validation; not full isolation-level conformance |
| G1-CLI | Implemented | Canonical bradian-db and legacy dap-admin executables, local SQL scripts, explicit transaction/history modes and process tests |
| G1-LOCAL-ACCESS | Partial | SQL CLI private Unix root checks; library paths, ACL/hostile-filesystem and complete local identity policy remain unqualified |
| G1-INSTALL | Partial | Linux development install is exercised; portable release installers/signing/upgrades remain open |
| G1-MEMORY | Partial | Query buffers, retained history, historical staging and backing live/delta maps are controlled; metadata/index/checkpoint/backup workspace, catalogs, aggregate transactions/pins, alternate engines, formatted output and global budgets remain open |
| G1-FAULTS | Partial | Existing torn-frame/CRC/process-exit tests; disk-full, failed sync/rename and physical-device qualification remain open |
| G1-REFERENCE | Partial | Checked SQL/decimal result corpora; external reference SQL and full documented isolation qualification remain open |
| G1-PORTABILITY | Open | Windows/macOS installation, lock lifecycle and recovery runs |

Gate 1 stays active until its exit conditions have evidence. The next implementation
work must address G1-MEMORY, G1-LOCAL-ACCESS and G1-FAULTS before new Gate 2 features.
The [backing RowStore verification](BRADIAN_GATE1_ROW_STORE_LIMITS_VERIFICATION_2026_11_13.md) records the latest runtime slice. The [transaction staging verification](BRADIAN_GATE1_STAGING_LIMITS_VERIFICATION_2026_11_13.md) retains its previous evidence. The [retained-history admission verification](BRADIAN_GATE1_HISTORY_ADMISSION_VERIFICATION_2026_11_13.md) retains its previous evidence. The [single-table limit verification](BRADIAN_GATE1_SINGLE_TABLE_LIMITS_VERIFICATION_2026_11_13.md) retains its prior evidence.
The [history scan verification](BRADIAN_GATE1_SCAN_LIMITS_VERIFICATION_2026_11_13.md) retains its earlier evidence.
The [Gate 1 local CLI verification](BRADIAN_GATE1_LOCAL_CLI_VERIFICATION_2026_11_13.md)
records what was actually run. No existing analytical, schema or SQL work is deleted
merely because it was implemented before this ordering was reconciled.

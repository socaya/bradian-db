# BRD-G37 — Verified retirement of inactive schema intents

**Design:** [ADR 0032](../adr/0032-legacy-ddl-intent-retirement.md)

`HistorySqlEngine::retire_legacy_schema_intents(&mut self)` provides an explicit,
one-way upgrade path from inactive G28–G31 work to G36. It does not apply any
prepared proposal, change rows or advance the catalog. Operators can subsequently
submit a fresh ALTER through the recoverable protocol.

The singleton retirement receipt binds the current catalog horizon and a CRC32
of every ordered legacy metadata key, original LSN and payload. Reopening validates
that history before trusting retirement. Archived proposal/fence/readiness records
remain durable; prepared-proposal status becomes empty and old mutation APIs refuse
further writes. G36's first catalog horizon must match retirement, then its checked
chain tracks later changes. Old fences are not silently rebound or ignored.

Six integration tests cover live prepared/coordinated/ready and cancelled work,
unchanged rows and catalog, duplicate/empty retirement rejection, legacy API
fencing, upgrade after retirement, backup/restore, retention, three metadata
corruption cases, five partial retirement-frame retries and independent catalog
mutation refusal. G36's integrity unit tests additionally reject forged retirement
versions, counts, digests and horizons before catalog recovery.

G33 pending catalog proposals must be resolved through their existing explicit
catalog decision APIs before retirement. CRC provides structural corruption
checking, not a cryptographic attestation. Historical metadata is retained; there
is no metadata compactor in this increment. Automatic CI remains manual-only.

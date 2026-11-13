# ADR 0046 — Bounded index recovery entries (G1-MEMORY)

**Status:** Accepted for derived-index audit and missing-index reconstruction.
Gate 1 remains active.

Index recovery previously cloned all checkpoint values into an unused map and
collected a second full leaf sequence to compare with expected key/pointer entries.
Retain only one bounded vector of checkpoint keys and row pointers. Values remain
validated in their decoded physical page and are never copied into a recovery map.

IndexLimits defaults to 2000000 retained entries and 256 MiB of accounted key bytes
plus native key/pointer tuple headers. Check entry count and checked byte growth
before copying each key. Empty snapshots fit a zero policy. Allocation/container
slack, one previous key, page decode/cache storage and index-builder descriptors
are separate; this is not total RSS or a complete checkpoint workspace reservation.

Reconcile actual leaves incrementally against expected entries using the same leaf
traversal as native range reads. Preserve strict global leaf order, successor/cycle
checks, sequence/count equality and every point lookup's pointer check. Rebuild
validates the generated index before publication. Impossible leaf counts are refused
against physical payload capacity before allocating their entry vector.

RowStore default opens retain default recovery policy. Native open_with_recovery_limits
accepts both backing-map and index recovery policy; its ordinary audits/rebuilds and
backup verification retain the selected policy. Explicit audit/rebuild methods can
override policy per operation. restore_backup_with_recovery_limits verifies the
staged destination under an explicit policy. Existing APIs use defaults.

Recovery entry admission precedes temporary-file cleanup/build/publication. An
over-policy missing index stays missing; snapshot and WAL bytes are not changed.
During open, refusal occurs before normal incomplete-tail repair. Existing indexes
are audited rather than overwritten. A quota refusal is a resource error, not an
integrity certificate for unvisited bytes. Failed I/O and incomplete-stage handling
retain their existing contracts. No file format, dependency or isolation change is
introduced. Global admission and checkpoint/build/metadata working-memory controls
remain separate Gate 1 work.

# ADR 0047 — Checkpoint and index build admission (G1-MEMORY)

**Status:** Accepted for checkpoint pointer construction and immutable index builds.
Gate 1 remains active.

Checkpoint row pages already stream, but their owned key/pointer list had no separate
admission and the index builder copied every descriptor's separator key. Add
IndexBuildLimits, defaulting to 2000000 input entries and 512 MiB of conservative
accounted reservation. Each key reserves its bytes, a native key/pointer tuple header
and two native borrowed descriptor headers. Two descriptor levels can coexist while
promoting a tree; their logical entry count is no greater than twice input count.
Descriptors now borrow separator bytes from the stable input keys, preserving files.

The reservation excludes allocator/vector capacity slack, fixed page/I/O buffers,
source maps, caches and metadata/directory workspace. It deliberately overestimates
logical descriptor storage for most trees. It is not total RSS or fallible allocation
reservation. Checked count/byte arithmetic refuses overflow.

Checkpoint preflight iterates borrowed live keys under the mutation mutex before
quarantine, temporary-file writes, snapshot/index publication or WAL truncation.
A quota refusal preserves data, delta usage, LSN, manifest, WAL and orphan/temp files
and does not poison the owner. A caller can supply a larger operation policy and
retry. Already-current checkpoints return without a build reservation. Legacy
manifest upgrades retain their separate index-audit policy.

Direct DiskBTree::build_with_limits validates inputs and preflights before creating
its target. Default build uses defaults. Missing-index rebuild preflights its builder
reservation before clearing a prior temporary file or writing a replacement. Open
quota refusal precedes normal WAL-tail repair. Post-admission I/O failures preserve
the existing poisoning/recovery protocol; no weaker I/O guarantee is introduced.

StorageLimits composes backing-row, recovery-entry and build policies. Native
RowStore::open_with_policies retains them; backup verification carries the complete
source policy; restore_backup_with_policies accepts an explicit destination policy.
Existing APIs remain available and choose default build policy. checkpoint_with_limits
is an operation override and does not change the owner policy. Policies are not
persisted and do not change archive/page/WAL/index formats or isolation.

Metadata buffers, global admission, aggregate transactions/pins, higher-level catalogs,
formatted output and alternate engines remain separate Gate 1 work. No OOM, failed-I/O,
physical-device or cross-platform qualification is claimed by these admission checks.

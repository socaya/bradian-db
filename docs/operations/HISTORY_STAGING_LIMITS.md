# Historical transaction staging policy

Fixed Master Gate 1 task G1-MEMORY now controls current staged mutations and saved
copies in each durable historical transaction. This policy is independent of query
buffers and committed retained-history limits.

| Budget | Default | Accounting |
|---|---|---|
| Current distinct staged keys | 256 | Inserted/replaced/deleted keys; repeated writes coalesce |
| Combined staged bytes | 8388608 | Current entries plus every saved map, marker header and name |
| Savepoint markers | Fixed maximum 16 | Duplicate names are separate markers until released |

Bytes charge key/payload lengths and native vector/option headers per entry.
Tombstones still charge the key and entry header. Marker bytes include their header,
name and all copied entries. The key ceiling cannot exceed the physical WAL maximum
of 256, even if a larger value is configured. Limits are not persisted.

## Configure

```sh
bradian-db sql --root ./private-history --history --max-staged-keys 128 --max-staged-bytes 2097152 --execute "BEGIN; UPDATE demo.public.items SET amount=150 WHERE id=1; COMMIT"
```

Both flags require --history. Zero keys/bytes permit read-only transactions while
refusing writes that need those resources. A zero byte budget also refuses an empty
savepoint because its marker/name require space. Other SQL modes keep their existing
contracts and reject these flags.

Native callers pass runtime_limits::StagingLimits to DurableMvccStore::begin_with_limits
or DurableTypedStore::begin_with_limits. The second argument explicitly selects read
validation. Existing begin and begin_with_read_validation use defaults. Historical
SQL uses HistorySqlEngine::with_session_limits(scan_control, staging_limits, callback).
Its with_session/with_session_control methods use default staging limits. The policy
is reused independently for each BEGIN in that session.

Native and typed staging_usage report current distinct keys and combined bytes;
staging_limits reports the selected policy. HistorySqlSession::staging_usage returns
None outside a transaction. Saved keys are charged through bytes, rather than added
to the current-key count.

## Refusal and recovery

Key/byte admission happens before copying the new pending entry or savepoint map.
The 257th distinct key is now refused at the mutation rather than only at COMMIT.
A native resource error preserves prior staged work and markers; the caller may
recover or commit earlier valid writes. No staged operation appends WAL. SQL keeps
its statement-error behavior: without markers it aborts all staged writes; with a
marker it requires ROLLBACK TO or full ROLLBACK before further work or COMMIT.

Replacing a staged value changes its current byte charge. RELEASE refunds discarded
marker/map copies. ROLLBACK TO drops descendants and current writes before restoring
the target map, preserving the target marker and read/scan conflict observations.
Full rollback/drop discards the transaction and releases its snapshot pin. Only a
successful outer COMMIT publishes the final write set; retained-history admission
and physical WAL size validation remain independent checks at commit.

This bounds accounted pending state per historical transaction, not total process
memory. Read observations/pins, allocator/container slack, fixed transaction headers,
statement parsing/typed encoding, transient point-read copies, scan/output buffers
and commit serialization are outside this budget. Active transaction count and global
admission, recovery working memory and other engines' staging remain open. [Backing-map limits](ROW_STORE_LIMITS.md) now separately control checkpoint/replay live and delta maps. No OOM,
disk-full, failed I/O or cross-platform qualification is claimed.

See [ADR 0044](../adr/0044-historical-transaction-staging-limits.md),
[retained-history limits](RETAINED_HISTORY_LIMITS.md) and
[executed verification](../implementation/BRADIAN_GATE1_STAGING_LIMITS_VERIFICATION_2026_11_13.md).

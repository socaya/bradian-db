# ADR 0043 — Retained history admission and recovery (G1-MEMORY)

**Status:** Accepted for per-history-domain retained-state admission; Gate 1 remains active.

HistoryLimits caps logical keys, retained versions (including tombstones/metadata)
and accounted retained bytes. Defaults are 1000000 keys, 1000000 versions and 256 MiB.
The byte estimate includes a logical-key entry, each derived version header/payload
and its backing physical history key/payload/header. It excludes fixed format/floor
markers, allocator/container slack, RowStore overlays/caches, active transactions,
typed catalog/epoch structures and temporary validation/output buffers. It is not
an RSS reservation or a global admission pool.

Maintain exact logical counters under the durable MVCC mutation mutex. Project a
whole transaction's final staged key set before encoding/appending its WAL frame.
Reject excess growth without publishing any rows, new LSN or counter changes. Pins
release when commit fails. Replacements and deletes add versions; deleting a value
is not a memory refund while its history/tombstone remains retained.

Reopen reconstructs/checks counters while visiting borrowed backing rows under one
lock, without a second full raw snapshot clone. An over-policy store refuses derived
state admission; it is never truncated or automatically pruned. RowStore open/replay
still precedes this check and can repair an incomplete WAL tail under its existing
contract. Raising policy explicitly can admit a larger valid store; policy is not
persisted or a physical format migration.

Retention keeps the existing durable-floor-before-delete protocol and baseline per
key. Bound its deletion worklist to 256 entries and resume a key cursor between
batches. Refund version/byte counters only after a synced deletion batch; retain
logical-key charges because baseline values/tombstones remain. Request release of unused version-vector capacity after pruning; allocator RSS reclamation is separate. Interrupted prune
reopen must reconstruct the committed prefix exactly and allow safe retry.

Exclusive historical ALTER preflights BOTH intent and activation additions before
its first durable intent. Coordinated recovery checks remaining activation capacity
before changing catalog state. A recoverable resource refusal must not become a
false corruption report or consume catalog state merely to discover a history cap.
Other I/O failures continue to use the existing durable intent/fencing protocol.

Native open_with_limits and SQL open_with_history_limits expose explicit policy. Typed open, shared/lifecycle migration and restore have policy-aware entry points; backup verification retains the source policy. Existing entry points use defaults. Failed staged restore/migration never publishes the destination and keeps its existing incomplete-stage recovery contract.
CLI history limits require --history; unsupported modes reject them instead of
ignoring them. No implicit retention, dropped history, changed schema epochs, weaker
conflict checks, third-party dependency or persistent format is introduced.

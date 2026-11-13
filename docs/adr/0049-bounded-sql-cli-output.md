# ADR 0049 — Bounded SQL CLI output (G1-MEMORY)

**Status:** Accepted for local native SQL CLI JSON lines. Gate 1 remains active.

The CLI previously converted complete typed result sets into another owned JSON tree
before serialization. Serialize borrowed rows/cells directly into one bounded line
buffer instead. Preserve the existing columns, type tags, string-encoded wide values,
UUIDs, escaping and lexical field order. Only fixed-size numeric/UUID conversions
need small temporary strings; no second result/value tree is retained.

--max-output-bytes applies to default, --table and --history modes, defaulting to
16777216 bytes per statement including the trailing newline. Charge the complete
encoded line: metadata, columns, cell tags, escaping and statement ordinal. Reset
for each statement. A zero policy is valid and refuses every nonempty line.

Check each serialization write before extending the buffer. Emit/flush only after
the complete line fits. Quota refusal emits none of that statement's line; earlier
complete lines remain. SQL execution precedes encoding, so an output refusal does
not undo an autocommit or earlier COMMIT. Scoped callbacks stop on refusal and drop
pending work/pins. This is an output failure, not corruption or a new isolation mode.

Actual output-device write/flush errors may still expose a partial line or lose an
acknowledgement after commit. Retain statement context and the existing no-blind-retry
contract. Buffer admission cannot reserve OS output writes or make them atomic.

This bounds encoded bytes per line, not cumulative script output, decoded SQL results,
allocator/vector slack, other CLI commands, remote service output or total RSS. Query,
staging, history and storage policies remain independent. No dependency, SQL grammar,
persistent format, native JSON profile or default isolation changes.

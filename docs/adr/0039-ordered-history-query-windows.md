# ADR 0039 — Ordered historical query windows

**Status:** Accepted for bounded single-table historical SELECT.
**Requirements:** BRD-REL-003 / BRD-SQL-006, documented query and ordering subset.

G44 adds [ORDER BY column [ASC|DESC] [NULLS FIRST|LAST], ...] [LIMIT n]
[OFFSET n] after the optional exact-primary-key WHERE clause. Names bind to the
requested schema epoch, including AS OF. Order columns need not be projected.
Expressions, ordinal positions, aliases, collations and general predicates remain
unsupported. Earlier SQL engines retain their existing SELECT contract.

ASC defaults to NULLS LAST; DESC defaults to NULLS FIRST. Explicit NULL placement
is independent of direction. Values compare by declared type: signed integers,
exact same-scale decimal coefficients, false before true, UTC timestamp microseconds,
UUID bytes and binary UTF-8 text. No locale, case folding or normalization is implied.
Equal explicit sort values are ordered by logical primary-key components ascending.
Queries with LIMIT/OFFSET but no ORDER BY also use logical primary-key order. Bare
SELECT without any new clause retains its original scan order for compatibility.

Parsing and all column/bound checks finish before reading rows. At most 16 explicit
sort keys, 10000 input rows and 10000 returned-limit rows are supported; OFFSET is at
most 1000000. Exceeding a bound fails explicitly rather than truncating the input
before sorting. These bounds apply to the new query-window path, not bare SELECT.
They bound sorting/window work, not the existing underlying domain scan, which
still materializes records before the row-count check. No streaming/index-seek or
bounded-memory full scan is claimed.

Rows come from the existing transaction or AS OF read path, then sort/window before
projection. LIMIT 0 still performs the normal read and validations; it cannot erase
scan/conflict observations. Uncommitted own inserts/updates/deletes are visible in
normal transaction queries. AS OF remains prohibited in G42 validating transactions.
The original snapshot pin and G41 statement-error/savepoint semantics remain intact.

No persistent format, WAL or optimizer change is introduced. OFFSET pages over
separate transactions can shift when writers commit; clients needing a consistent
multi-page view must retain the same transaction or explicit retained AS OF LSN.
This is not keyset pagination, distributed ordering or full SQL conformance.

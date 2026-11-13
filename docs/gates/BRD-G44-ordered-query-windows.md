# BRD-G44 — Ordered historical query windows

**Design:** [ADR 0039](../adr/0039-ordered-history-query-windows.md)
**Requirements:** BRD-REL-003 and BRD-SQL-006, bounded single-table SELECT subset.

Historical SQL supports ORDER BY with direction and NULL placement, LIMIT and OFFSET:

```sql
BEGIN;
SELECT id FROM bank.public.accounts ORDER BY balance DESC NULLS LAST LIMIT 25 OFFSET 0;
COMMIT;
```

Each `execute` accepts one statement. ORDER BY columns bind to the requested schema,
including AS OF, and need not appear in the projection. WHERE remains optional exact
primary-key equality. The clause order is WHERE, ORDER BY, LIMIT, OFFSET; LIMIT and
OFFSET are independently optional. Unsupported syntax fails before reading rows.

ASC uses NULLS LAST and DESC uses NULLS FIRST by default. Explicit NULLS placement
is independent of direction. Signed integer, decimal, boolean, UTC timestamp, UUID
and binary UTF-8 text comparisons follow declared types. There is no locale collation,
normalization or case folding. Equal sort keys use declared logical primary-key
components ascending, including composite keys. LIMIT/OFFSET without ORDER BY also
use logical PK order. Bare SELECT preserves its earlier scan order.

The new path supports at most 16 explicit sort columns and 10000 input rows. LIMIT
is at most 10000 and OFFSET at most 1000000. Input over the row budget fails, even
with LIMIT 0, rather than returning a misleading early subset. Sorting/window work
is bounded; underlying scans still materialize the domain before the input-count
check. No streaming, index-seek optimization or bounded-memory full scan is claimed.

Queries see their transaction's own staged inserts, updates and deletes. Sorting and
windowing happen before projection. LIMIT 0 still takes the normal read path and
preserves conservative scan conflicts. G42 validating sessions retain AS OF refusal;
G41 error recovery and snapshot pins remain unchanged. Use one transaction or one
retained AS OF LSN for consistent multi-page reads while other writers commit.

Nine SQL integrations and two native ordering tests verify typed/NULL ordering,
unprojected fields, signed/composite PK ties, page consistency under concurrent
writes, historical schemas/defaults, staged mutations and savepoint rollback,
invalid clauses, conflict preservation, exact parser/input limits, checkpoint and
verified archive restore, plus older-engine compatibility. See the
[executed report](../implementation/BRADIAN_G44_VERIFICATION_2026_11_13.md).

General predicates, joins, expression/ordinal/alias ORDER BY, collations, keyset
pagination and distributed sorting remain separate requirements. No persistent
format, lockfile, dependency, WAL protocol or older-engine SQL contract changed.

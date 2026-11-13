# ADR 0034 — Recoverable nullability relaxation and implicit INSERT NULL

**Status:** Accepted for exclusive historical SQL.

G39 extends G36's checked schema transition to relax a non-key column from NOT NULL
to nullable. Existing names, positions, data types, table identity and PK/unique/FK
definitions remain unchanged. Nullable trailing additions remain supported. A
transition must change at least one permitted definition; narrowing nullability,
renames, dropped columns, type changes and nullable primary keys are refused.

The G11 catalog gains a distinct `ProposeNullableRevision` command for the wider
compatibility rule. The existing G33 `ProposeTableRevision` keeps its strict
nullable-addition contract. Both activate through the existing exact proposal
receipt. G36's history intent and activation layout remains unchanged: its old and
proposed definitions identify the transition, and all four recovery boundaries are
reused. Older readers reject a relaxation intent or its new catalog command.
No existing row payload, schema epoch or page is rewritten.

Rows decode under their source epoch, then project only to a compatible later
schema. Relaxing nullability preserves existing values. A later NULL row cannot be
interpreted under an earlier NOT NULL epoch; AS OF selects the original schema and
G22 visibility selects the corresponding rows. Commit-time epoch validation remains
mandatory for all retained versions, including tombstoned data.

Historical SQL INSERT may omit nullable columns; these receive NULL. Omitted NOT
NULL columns, duplicate/unknown columns and list/value count mismatches fail the
statement and abort staged SQL work. This does not implement non-NULL defaults,
DEFAULT expressions, SET NOT NULL validation/backfill or DEFAULT VALUES syntax.
Other SQL engines retain their existing INSERT contracts.

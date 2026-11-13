# BRD-G43 — Checked historical UPDATE expressions

**Design:** [ADR 0038](../adr/0038-checked-history-update-expressions.md)
**Requirements:** BRD-REL-003 and BRD-SQL-006, bounded DML/numeric subset.

Historical SQL supports checked arithmetic and same-type column references in
exact-primary-key UPDATE assignments, plus standalone DEFAULT:

```sql
BEGIN;
UPDATE bank.public.accounts SET balance=balance+10 WHERE id=1;
UPDATE bank.public.accounts SET balance=(balance-5)*2 WHERE id=2;
COMMIT;
```

Each `execute` accepts one statement. All assignments in one UPDATE read the
original row, so swapping two same-type fields works and assignment order cannot
change the result. The full replacement is validated before staging one mutation.
Duplicate fields and primary-key changes remain errors.

The expression grammar includes literals, column references, parentheses, unary
signs, +, -, *, / and standalone DEFAULT. Multiplication/division have higher
precedence; binary operators associate left. Numeric column operands must exactly
match the target type. Implicit widening, cross-scale decimal coercion, floats,
functions, CASE, comparisons and subqueries remain unsupported.

Integer arithmetic checks each intermediate against its declared width; division
truncates toward zero. Decimal arithmetic is exact at declared precision and scale:
nonrepresentable fractions, division by zero and overflow are errors, never silently
rounded or wrapped. Factor cancellation avoids overflowing intermediates for valid
38-digit multiplication/division. NULL arithmetic propagates NULL; the final row
must still satisfy NOT NULL. DEFAULT resolves to the schema default, nullable NULL,
or an error for a required column without a default. It cannot be nested in math.

Parsing permits 128 expression nodes across assignments and 32 nested parentheses/
unary operations, under the existing statement token budget. SQL comments are
rejected. Negative literals and predicates, including signed integer minima, retain
their earlier behavior. Earlier SQL engines retain their literal-only grammar.

Binding occurs before row access; runtime evaluation occurs only for a matching row.
A runtime failure stages none of that statement's fields. G41 transaction-error and
savepoint recovery, G42 read validation, schema epochs and WAL atomicity remain in
force. No persistent format changes or multi-row UPDATE are introduced.

Eleven tests exercise precedence, signs, original-row assignment semantics, INT32/
INT64 extrema, exact decimal/NULL/default behavior, invalid binding and grammar,
statement/transaction rollback, savepoint recovery, conflict checks, parser budgets,
history, checkpoint/reopen, verified backup/restore and older-engine compatibility.
A 256-case decimal corpus uses an independent small rational reference. See the
[executed report](../implementation/BRADIAN_G43_VERIFICATION_2026_11_13.md).

General expression support in SELECT/INSERT/WHERE, joins, wider coercions, secondary/
foreign-key enforcement and full SQL conformance remain separate requirements.

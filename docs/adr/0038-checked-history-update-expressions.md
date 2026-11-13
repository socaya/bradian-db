# ADR 0038 — Checked historical UPDATE expressions

**Status:** Accepted for the exact-primary-key historical SQL profile.
**Requirement:** BRD-REL-003 / BRD-SQL-006 scalar-expression and numeric subset; full conformance remains open.

G43 permits UPDATE assignments containing numeric +, -, *, /, unary signs,
parentheses, same-type column references, literals and a standalone DEFAULT.
Multiplication/division bind more tightly than addition/subtraction; binary
operators associate left. All assignments evaluate against the original matching
row, and the complete replacement is validated before staging one row mutation.
Primary-key changes and duplicate assignments remain errors.

The expression lexer is selected only by historical SQL sessions. Earlier SQL
engines keep their literal-only grammar and original tokenization. Negative literals,
including signed integer minima, remain accepted in historical INSERT/predicates.
The parser allows at most 128 expression nodes across SET assignments and 32 nested
parentheses/unary operations, in addition to the existing statement token budget.

Numeric operands are bound to the target type. Referenced columns must have exactly
the target data type; implicit widening, float math and cross-scale decimal coercion
are unsupported. Each arithmetic intermediate must fit the declared integer width
or decimal precision. Integer division truncates toward zero. Fixed-scale decimal
operations must be exact at that scale: discarded fractional digits are errors,
never silently rounded. Multiplication/division cancel factors before checked i128
arithmetic so representable high-precision results do not require overflowing
intermediates. Division by zero and overflow return SQL type errors.

NULL arithmetic propagates NULL, subject to final row nullability validation. NULL
is allowed as an intermediate in a required target expression; the final NULL result
is rejected. DEFAULT is only a whole assignment and resolves to the immutable
schema default, nullable NULL, or a required-field error. Explicit NULL stays distinct.
All column/literal/operator binding happens before row access; runtime arithmetic
is evaluated only for a matching row. SQL comments, functions, comparisons, CASE, subqueries,
SELECT expressions and expressions in INSERT/WHERE remain unsupported.

A statement error retains G41 behavior: without markers all staged work aborts;
with markers explicit ROLLBACK TO/full ROLLBACK is required. G42 read validation and
original snapshot conflict checks remain in force. No persistent format changes,
new WAL protocol, multi-row UPDATE or implicit engine migration are introduced.

# Native SQL CLI output limits

This fixed Master Gate 1 / G1-MEMORY control applies to bradian-db sql and the legacy
dap-admin sql entry point in default, --table and --history modes.

| Budget | Default | Accounting |
|---|---|---|
| --max-output-bytes | 16777216 per statement | Complete UTF-8 JSON line, including metadata, columns, type tags, escaped values, statement ordinal and newline |

```sh
bradian-db sql --root ./private-data --max-output-bytes 1048576 --execute "SELECT amount FROM demo.public.items WHERE id=1"
```

The policy resets for each statement and is independent of scan/snapshot/result,
transaction staging and retained-history limits. Zero is valid but refuses all
nonempty output lines. A result that fits decoded query limits can still exceed its
encoded output limit, especially with escaped text or many type-tagged cells.

## Serialization and failure

The encoder borrows the existing typed result and serializes cells directly into a
bounded line buffer. It does not build another owned JSON result tree or clone text
cells. Wide integers, decimals and timestamps keep their string representation;
NULL, booleans, Int32 and canonical UUID behavior are unchanged. Exact output bytes
and field order remain compatible with the previous profile.

Admission occurs before extending each buffer chunk. The newline consumes budget.
Only a completely admitted line is written/flushed to standard output. A quota
failure emits no part of that statement's line and stops the script; previous
complete lines remain. Errors report the statement ordinal and limit without SQL
text. The CLI drops active scoped work/pins when output failure leaves its callback.

SQL execution happens before output encoding. Autocommits and explicit COMMITs
already completed remain durable even if their output cannot be emitted. An output
error is therefore not evidence that nothing committed. Do not blindly retry scripts;
inspect persisted state before deciding what to repeat. This is the existing output
failure contract, now also exercised for byte-limit refusal.

Actual output-device errors remain separate from quota admission. A write can expose
a partial line and a flush can fail after bytes have been handed to the sink. The
buffer does not provide atomic OS output or exactly-once acknowledgements.

## Scope

The limit bounds encoded line length, not allocator/vector capacity slack, decoded
result memory, cumulative script output, other CLI commands, HTTP/Studio responses
or process RSS. Fixed-size numeric/UUID formatting temporaries remain. Global
admission and broader runtime/failure qualification remain open. No SQL grammar,
file format, transaction semantics or dependencies change.

See [ADR 0049](../adr/0049-bounded-sql-cli-output.md) and
[executed verification](../implementation/BRADIAN_GATE1_SQL_OUTPUT_LIMITS_VERIFICATION_2026_11_13.md).

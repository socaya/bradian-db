# ADR 0040 — Native local SQL command and canonical roadmap tracking

**Status:** Accepted for Gate 1's local CLI slice; not a gate-completion claim.
**Requirements:** BRD-REL-003, BRD-TXN-001, BRD-PORT-001, BRD-SQL-005.

The master Section 32 Gate 0–9 sequence is the canonical development order.
Earlier BRD-G1–BRD-G44 files are historical implementation records, not a separate
roadmap. They retain their paths and identities for compatibility. New tasks belong
to the fixed master gate rather than receiving another G-number.

The existing dap-admin operator functions become a shared library used by both the
legacy dap-admin executable and a canonical bradian-db executable. Analytical commands
and on-disk identifiers remain unchanged. A new sql command embeds the existing native
relational engines; it does not call an external database or open a network listener.

--execute supplies SQL text; --file supplies a UTF-8 script, with '-' selecting stdin.
Input is at most 1 MiB, each statement at most 64 KiB and each script at most 256
statements. Splitting honors single-quoted strings and doubled quotes. Unterminated
strings and unsupported comments fail before opening a database. Empty scripts fail.
SQL binding remains the engine's responsibility; this splitter is not a second SQL
parser. No shell interpretation, interpolation or include commands are permitted.

Default mode executes bounded catalog DDL and autocommit DML through SqlEngine.
--table db.schema.table uses its scoped single-table SQL transaction session.
--history explicitly opens an already migrated history-domain root and uses its
multi-table historical session. It never auto-migrates or silently changes engines.
Transaction modes require explicit BEGIN/COMMIT/ROLLBACK as the chosen engine does.
An unfinished transaction at script end fails and drops staged work. Execution stops
at the first error; previous explicit/autocommit transactions remain committed.
Outputs for staged statements are not durable acknowledgements; only successful
COMMIT acknowledges their transaction. Output failures can occur after a durable
commit, so automatic retries are unsafe.

The SQL command is initially Unix-only. New roots are created with mode 0700;
existing roots with group/other permission bits, non-directories and final-component
symlinks are rejected. No existing permissions are silently changed. This is a local
OS-account boundary, not per-user SQL authorization, ACL/hostile-filesystem protection,
secure remote sessions or complete Gate 1 identity qualification. Library callers and
older analytical commands do not acquire new permission guarantees from this command.
Other platforms fail closed until qualified; help/version remain available.

One JSON object is emitted per successful statement, with a one-based statement
number, result kind and affected count or columns/typed rows. Int64, decimal unscaled
coefficients and timestamp microseconds are strings to avoid consumer precision loss.
Errors identify the statement number without echoing its SQL text. Files, logs and
terminal output may contain data; operators must handle them as database exports.
No new third-party runtime dependencies or persistent formats are introduced.

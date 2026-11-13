# Native local SQL command

The canonical executable is bradian-db. The existing dap-admin executable is retained
for compatibility and uses the same operator command implementation. The sql command
embeds native Rust relational engines and does not start an HTTP service.

## Build and development install

```sh
cargo build -p dap-admin --bin bradian-db --locked
cargo install --path crates/dap-admin --bin bradian-db --locked --debug --root /tmp/bradian-install
/tmp/bradian-install/bin/bradian-db version
```

This is a local development install. Signed release packages, platform installers
and upgrade qualification are not supplied by this command. SQL root permission
checks currently require Unix; other platforms fail closed for sql.

## Create and reopen a persisted database

```sh
bradian-db sql --root ./private-data --execute "CREATE DATABASE demo; CREATE SCHEMA demo.public; CREATE TABLE demo.public.items(id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id)); INSERT INTO demo.public.items(id,amount) VALUES(1,100)"
bradian-db sql --root ./private-data --execute "SELECT amount FROM demo.public.items WHERE id=1"
```

The command creates a missing root with mode 0700. Existing roots must be real
private directories with no group/other mode bits; final-component symlinks are
refused. It never changes an existing root's permissions automatically. These checks
are a limited local OS-account boundary, not SQL users/roles or complete hostile-path,
ACL or owner-identity qualification. Existing library/analytical paths are unchanged.

## Explicit single-table transactions

```sh
bradian-db sql --root ./private-data --table demo.public.items --execute "BEGIN; UPDATE demo.public.items SET amount=150 WHERE id=1; COMMIT"
```

Without --table or --history each statement uses the original bounded SqlEngine's
DDL/autocommit contract. --table opens one scoped transaction session for an existing
table. BEGIN is required for DML in that session. Cross-table statements and DDL are
rejected by the chosen engine. An error stops the script and rolls back remaining
staged work. An unfinished transaction at end is an error and rolls back. Earlier
autocommits or explicit COMMITs remain committed; a script is not one implicit atomic
transaction. Only a successful COMMIT acknowledges staged statement durability.

## Scripts and historical roots

--execute and --file are mutually exclusive. --file accepts a UTF-8 path; --file -
reads stdin. Semicolons inside single-quoted strings and doubled quote escapes are
preserved. Comments are unsupported. Bounds are 1 MiB input, 64 KiB per nonempty
statement and 256 statements. Lexical/size failures happen before database open;
SQL semantic failures can happen after earlier statements have committed.

--history explicitly selects an existing migrated history-domain root. It cannot be
combined with --table and never migrates a root implicitly. It exposes the existing
historical session SQL, including read validation, savepoints, checked UPDATE and
ordered query windows. Schema activation, migration and history provisioning remain
explicit native APIs; this command does not turn them into transactional script DDL.

```sh
bradian-db sql --root ./existing-private-history --history --execute "BEGIN WITH READ VALIDATION; SELECT id FROM demo.public.items ORDER BY amount DESC LIMIT 10; COMMIT"
```

A library-created/migrated root may need its owner to provision appropriate private
permissions before the CLI accepts it. Do not point one engine mode at another
mode's root or delete migration markers to bypass compatibility checks.

## Historical query budgets

--history uses default per-query scan row/byte/work limits. Optional --max-scan-rows,
--max-scan-bytes and --max-scan-work require that mode. See the [scan policy](HISTORY_QUERY_LIMITS.md)
for accounting, cancellation, compatibility and remaining resident-memory limits.

## Output and failure contract

One JSON line is emitted for each successfully executed statement. The statement
number is one-based and ignores empty separators. Commands return kind=command and
an affected count; queries return kind=rows, columns and typed values. Int64 values,
decimal unscaled coefficients and timestamp microseconds are strings, preserving
precision. A decimal's scale remains schema-defined; unscaled is not a formatted
currency value. NULL and booleans remain distinct. UUIDs use canonical hex grouping.

Errors identify the statement ordinal without echoing SQL text. Standard output is
a data export; operators must protect it and input scripts. Output can fail after
COMMIT is durable, so do not automatically retry a script on a lost/broken output
stream. The next command opens and verifies the existing persistent database rather
than assuming a prior failed command committed nothing.

See [fixed roadmap](../implementation/BRADIAN_MASTER_ROADMAP.md) and
[current verification](../implementation/BRADIAN_GATE1_LOCAL_CLI_VERIFICATION_2026_11_13.md).

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

Every SQL mode uses default scan/snapshot/result row, byte and work limits. Optional
--max-scan-rows, --max-scan-bytes and --max-scan-work apply to default, --table and
--history modes. See the [single-table policy](SINGLE_TABLE_QUERY_LIMITS.md) and
[history policy](HISTORY_QUERY_LIMITS.md) for accounting, cancellation, compatibility
and remaining resident-memory limits.

## Retained-history admission

In --history mode, --max-history-keys, --max-history-versions and --max-history-bytes configure per-domain retained-state admission. Defaults are 1000000 keys, 1000000 versions and 268435456 accounted bytes. These flags require --history. They apply to open and committed growth; they do not reset per query or automatically prune old versions. See the [retained-history policy](RETAINED_HISTORY_LIMITS.md) for accounting, reopen and explicit pruning.

## Historical transaction staging

--max-staged-keys and --max-staged-bytes require --history. Defaults are 256 distinct current keys and 8388608 combined accounted bytes for current mutations and all savepoint copies. The physical 256-key maximum cannot be raised. A refusal occurs before copying the new staged entry/marker and uses existing SQL error/rollback rules. See the [staging policy](HISTORY_STAGING_LIMITS.md) for accounting, refunds and native APIs.

All modes also use default [backing RowStore limits](ROW_STORE_LIMITS.md) during checkpoint/replay and writes. Query/history/staging flags do not override backing-map policy; explicit backing policy is currently a native RowStore API.

[Index recovery entry limits](INDEX_RECOVERY_LIMITS.md) also apply by default. Explicit recovery policy is currently a native API.

Default [checkpoint/build reservation limits](CHECKPOINT_BUILD_LIMITS.md) also apply. StorageLimits exposes composed policy to native callers; existing CLI flags do not configure it.

Native checkpoint and archive helpers now use [metadata byte limits](METADATA_LIMITS.md). Explicit JSON archive policy is available through native backup/restore APIs.

## Output byte budget

--max-output-bytes applies to every SQL mode, defaulting to 16777216 bytes per statement including the newline. The encoder borrows typed results and admits a complete line before emission; quota refusal emits no partial line and stops the script. It does not undo completed commits. See the [output policy](SQL_OUTPUT_LIMITS.md) for exact accounting and failure semantics.

## Output and failure contract

Each emitted JSON line represents one successfully executed statement. Output encoding or I/O can fail after execution and prevent that statement's line. The statement
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

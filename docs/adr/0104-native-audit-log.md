# ADR 0104 — Native audit log foundation (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a persistence primitive.**

## 1. Honest scope

`bradian_relational::audit_log::AuditLog` — append-only
persistent log with per-record CRC32. Records
`(timestamp_ms, principal_id, sql, outcome)` where outcome is
`Ok | Err(msg) | AuthzDenied(msg)`.

API:
- `AuditLog::open(path)` — creates the file if missing;
  opens for append.
- `append(&entry)` — writes + fsyncs one record. Serialises
  through a mutex for cross-thread safety.
- `read_all()` — replays the whole log; torn tail → end of
  log; CRC mismatch → `Corruption` error.

## 2. Out of scope (BRD-SEC-005 production obligations)

- **Tamper evidence** — no hash chaining between records.
- **Externally anchored integrity** — no signed digests.
- **Immutable storage** — plain file on local disk.
- **Access-controlled retention** — operator must enforce FS
  permissions.
- **Automatic integration with `SqlEngine::execute_as`** — a
  future slice wires the log into the authz-gated path.

## 3. Compliance

1. Append + read round-trip in order.
2. Over-size SQL rejected without writing.
3. Torn tail → treated as end-of-log.
4. CRC tamper → `Corruption`.
5. Concurrent appends from multiple threads serialise.
6. 5 unit tests in `src/audit_log.rs::tests`.

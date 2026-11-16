# ADR 0118 — Hash-chained audit log (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch I I1 — tamper
EVIDENCE primitive on top of ADR 0104.

## Scope

- `bradian_relational::audit_chain::AuditChain` — append-
  only persistent log where each record carries
  `prev_hash: [u8; 32]` and `this_hash = BLAKE3(prev_hash ||
  body)`.
- `append(entry) -> [u8; 32]` returns the new chain tip.
- `verify() -> Vec<ChainedRecord>` walks the whole chain
  and returns `Tamper { index }` on the first broken link.
- Separate from ADR 0104 (`AuditLog`): operators pick CRC32
  simplicity OR BLAKE3-chained tamper-evidence per
  deployment.

## Out of scope (BRD-SEC-005 production obligations)

- **Externally anchored integrity** — no signed digest
  pushed to an external anchor.
- **Tamper PREVENTION** — an attacker with write access to
  the whole file can rebuild the chain end-to-end.
- **WORM storage** — plain file on local disk.
- **Access-controlled retention** — operator must enforce
  FS permissions.
- **Keyed MAC chain** — extension to BACA-style keyed
  hashing deferred.

## Compliance

1. Fresh chain: `last_hash() == GENESIS_HASH` (32 zeros).
2. Append two records: `records[1].prev_hash ==
   records[0].this_hash`.
3. Body-byte tamper → either `Tamper`, `Serde`, or
   `Corruption` at the tampered record index.
4. Trailer-hash tamper → `Tamper`.
5. Reopen: `last_hash` loaded from disk; next append
   chains from it.
6. Oversize body (>128 KiB) → `RecordTooLarge`.
7. 6 unit tests.

# Bradian — penetration review scope-of-work

Draft scope document for an eventual independent penetration /
security review. The review itself is procurement — this
document is the artifact an owner can hand to an external
auditor.

## Review targets (components in scope)

| Target | File / module | Primary attack surface |
|---|---|---|
| BACE envelope + BACA MAC | `crates/bradian-relational/src/bace/` | Key confusion, downgrade, MAC bypass, truncation |
| Backup descriptor MAC (BRBKv002) | `crates/bradian-relational/src/backup.rs` | Descriptor tampering, cross-version confusion |
| RowStore WAL (BRWL) | `crates/bradian-relational/src/row_store.rs` | Opcode confusion, truncation recovery, frame injection |
| BRPG checkpoint pages | `crates/bradian-relational/src/page.rs` | Page flag confusion, bit-0/bit-1 swap, oversized payload |
| `.brcs` segment reader | `crates/bradian-storage/src/segment/reader.rs` | Trailer-byte confusion, dict-pool parse, column-dict region |
| SQL parser | `crates/bradian-relational/src/sql.rs` | Injection via prepared-statement binding, authz bypass |
| HTTP engine | `crates/bradian-serving/src/` | `BRADIAN_API_TOKEN` loopback boundary, CORS, endpoint ACLs |
| Local-access invariants | `crates/bradian-relational/src/local_access.rs` | Symlink races, parent-dir trust, UID/GID checks |

## Threat-model excerpts to validate

Drawn from `docs/security/BRADIAN_THREAT_MODEL.md`:

1. **TM-001** — attacker with FS write access swaps a BACA
   frame for its legacy BACE equivalent (downgrade). Strict
   mode (ADR 0091/0092) rejects; non-strict tolerates. Verify
   both.
2. **TM-002** — attacker with the `BRADIAN_API_TOKEN` over a
   non-loopback network path. CLAUDE.md §6 says this must be
   rejected until TLS lands.
3. **TM-003** — attacker with RowStore key-read capability
   but no MAC key. Verify BACA pages remain unreadable (wrong
   key → `Corruption`).
4. **TM-004** — torn WAL frame mid-power-loss. Verify
   recovery at last complete frame + no silent commit
   acceptance of the torn record (see
   `fault_recovery_extension.rs`).
5. **TM-005** — operator deletes `rows.lock` mid-session.
   Known foot-gun per ADR 0092 §2 honest boundary. Verify the
   data files themselves remain intact.
6. **TM-006** — SQL injection via prepared-statement param
   that contains `'` + `;`. Verify ADR 0100 escape.
7. **TM-007** — authz bypass by calling `execute()` (bypasses
   `execute_as()`'s capability check). Verify the
   documentation makes this boundary explicit and operator
   wrappers never fall back.

## What the review MUST NOT be scoped to

(These are out-of-scope per the current CLAUDE.md §2 honest
status. The auditor should NOT conclude "fail" on these;
they are documented as not-shipped.)

- Full enterprise identity / TLS mTLS (INV-003 open).
- Cross-platform recovery qualification (G1-PORTABILITY).
- Serializable isolation (ADR 0037 is bounded / explicit).
- Distributed timestamps / consensus.
- Tamper-proof audit log (ADR 0104 is append-only with
  CRC32, NOT hash-chained).

## Deliverables requested from the auditor

1. Written findings keyed to the component table above.
2. For each finding: severity (CVSS), repro, suggested fix
   path (with ADR reference if applicable).
3. Verification of the shipped invariants (MACs verify,
   CRC32s detect, downgrade rejects under strict mode).
4. Explicit list of items the auditor did NOT examine
   (coverage boundary document).

## Procurement notes

- Prefer auditors with prior experience in on-disk database
  internals (page codecs, WAL, MVCC) over generic AppSec
  shops.
- Scope should include **source-level review** of the four
  BACE-adjacent modules above, not just black-box testing.
- Expected engagement: 2–4 weeks of senior auditor time.

This document is the scope-of-work the owner will hand to the
procured auditor. Until that procurement happens, no
production security claim can be made beyond what the
in-repo tests demonstrate.

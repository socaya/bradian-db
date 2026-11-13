# Bradian current-source threat model

This Gate 0 threat model describes current code and open trust decisions. It is not
an independent penetration test, formal proof or production acceptance. Its scope
is the local native kernel/CLI and inherited loopback analytical service; cluster,
federation and end-to-end enterprise security are not implemented contracts.

## Assets and trust boundaries

Authoritative rows, retained history, metadata, catalog decisions, backups, analytical
exports, credentials and future policy contexts are sensitive assets. Current local
library callers execute with the process owner's filesystem authority. The new SQL
CLI adds a Unix private-root check, but does not add per-user SQL authentication.
The analytical HTTP service uses a loopback bootstrap administrator token. The
browser is a separate, untrusted credential/storage boundary; a token must not be
embedded in frontend files. No current local helper establishes remote node trust.

## Threats, observed controls and remaining blockers

| Threat | Current control/evidence | Remaining work |
|---|---|---|
| Unauthenticated analytical request | `dap-serving/tests/http_roundtrip.rs` exercises bootstrap route protection; health is intentionally public | Real sessions, expiry/revocation, scoped principals and transport qualification |
| Local cross-user row access | SQL CLI creates mode-0700 roots and rejects public mode bits/final symlinks; subprocess tests verify refusal | Owner/ACL/ancestor-race analysis and equivalent enforcement in every library/service path |
| Library caller bypasses CLI checks | Explicitly trusted local embedding; no claim that the CLI secures all calls | Authoritative access context and negative path-equivalence tests |
| Stolen plaintext WAL/backup | CRC/checkpoint verification detects accidental corruption | Integrated encryption, protected keys, archive authentication and key-recovery drills |
| Malicious frame/catalog mutation | Corruption, sequence, epoch and exact-catalog-horizon tests fail closed | CRC is not a signature; hostile tampering and authenticated recovery remain open |
| Partial commit or interrupted DDL | Atomic batch, process-exit, torn-frame, fencing and roll-forward tests | Failed I/O, disk-full, physical-device and cross-platform qualification |
| Query/metadata resource exhaustion | Parser/query-window/script limits and historical/single-table scan/snapshot/projection row/byte/work/cancellation controls, retained-history/staging, backing/recovery maps, build/metadata and SQL CLI output bounds exist | Directory/I/O/cache/decoded metadata workspace, catalog/aggregate transaction state, alternate engine paths and other output remain materialized; global quotas and complete cancellation/admission required |
| Secret/data leakage through diagnostics | CLI reports statement ordinal without echoing SQL; wide numeric JSON is lossless | Output is still a data export; redaction, role-aware diagnostics and audit policy are incomplete |
| Browser credential compromise | Bootstrap policy forbids bundling the administrator token | Real browser session, CSRF/CORS/content-security review and end-to-end user authorization |
| Cross-store stale or unauthorized analytics | Relational/analytical engines remain separate; no strong HTAP claim | Commit/policy-coordinate propagation with revision-equivalence and disclosure tests |
| Hostile filesystem or privileged OS account | Final symlink refusal and private mode bits are a limited guard | No defense against privileged host users, hostile ancestors, TOCTOU substitution or ACL misconfiguration is claimed |
| Split brain, rogue node, replay or stale gateway | No secure-cluster/strong-distributed release claim | Enrollment, mTLS, consensus, fencing, transaction outcomes and adversarial partition prototypes |

## Design blockers and acceptance policy

The canonical roadmap keeps kernel/local-access/runtime/failure work in Gate 1.
SQL permission equivalence and safe lifecycle work precede later cluster operations.
Global timestamp and identity wire formats must not be frozen before the prototypes
listed in the source inventory are evaluated. Encryption primitives and token helpers
must not be used as evidence that authorization or key management is integrated.

Automatic CI remains manual-only by owner instruction. Actual executed checks are
linked from the current verification report; test files alone are not acceptance.
No native service may be exposed remotely under the bootstrap restriction. No
production security, full gate closure or external audit is claimed by this document.

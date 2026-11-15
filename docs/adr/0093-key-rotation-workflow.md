# ADR 0093 — BACA key rotation: previous-key fallback (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

## 1. Context

Operators need to rotate the 32-byte BACA key without a full
rewrite outage. Under ADR 0075/0080/0086/0089/0090, every BACA
frame MAC's are bound to one key; a key swap makes every
existing frame unreadable until re-written. The operator
window is: deploy new key → re-write artifacts → retire old
key. During that window, readers must verify under either
key.

## 2. Scope

**In scope:**

- `BaceWiring.previous: Option<BaceAuthentication>` (default
  `None`). Semantics: "an older key operators may be
  rotating away from."
- Reader fallback: when a MAC verification fails under the
  primary `authentication` key, retry under `previous` if
  set. Success = accept. Failure under both = reject.
- Strict-mode interaction: when `strict_authenticated =
  true`, `previous` is **ignored**. Strict operators enforce
  the current key only.
- Backup descriptor decode (`decode_with_strict_auth`): same
  fallback logic.
- Writer path is **unchanged**: writers always MAC under
  `authentication` (the primary key). `previous` is a
  reader-only concept.
- Tests for: rotation-window round-trip, reject when both
  keys miss, strict ignores `previous`.

**Out of scope** (future ADRs):

- Multi-key chains (more than one previous key). One
  `previous` covers the common one-step rotation.
- Automated rotation daemon.
- Key material storage format (keyfile loader is a separate
  ADR).

## 3. Design

### 3.1 Flag placement

```rust
pub struct BaceWiring {
    // existing fields ...
    pub authentication: Option<BaceAuthentication>,
    pub strict_authenticated: bool,
    /// ADR 0093 — previous-key fallback for rotation windows.
    /// When a MAC check under `authentication` fails and this
    /// is `Some`, the reader retries under `previous`. Ignored
    /// under `strict_authenticated`.
    pub previous_authentication: Option<BaceAuthentication>,
}
```

### 3.2 Reader fallback helper

A new private helper `verify_with_fallback` wraps the
`read_authenticated_envelope` call:
- Try primary; if `Ok`, return.
- If `Err(MacMismatch)` and `previous.is_some() && !strict`,
  try previous; on `Ok` return.
- Any other error type or dual-mismatch → return the primary
  error.

Applies to:
- `.brcs` BACA page decode (trailer 2).
- BRWL BACA opcode 5 decode.
- BRPG bit-1 page decode.
- Backup descriptor `decode_v2`.

### 3.3 Compliance

1. Pre-ADR-0093: `previous = None` preserves all behavior.
2. Write under key A, read with primary=A, previous=None →
   ok.
3. Write under A, read with primary=B, previous=A → ok via
   fallback.
4. Write under A, read with primary=B, previous=C → fail
   (MAC mismatch under both).
5. Strict + primary=B + previous=A + A-signed frame →
   rejected (strict ignores previous).
6. Clippy clean.
7. 1131 pre-ADR-0093 tests pass.

## 4. Alternatives

**A. Vec of previous keys (multi-step rotation).** Deferred;
one step covers the common case.

**B. Writer signs with both keys.** Doubles MAC bytes on
disk; rejected — rotation is bounded, writer stays single-key.

**C. Separate "rotation open" entry point.** Rejected — the
field placement on `BaceWiring` keeps dispatch internal.

## 5. Open design questions

None substantive.

## 6. Acceptance evidence

1. ADRs 0060–0092 accepted.
2. §3.3 items 1–7 covered.
3. New integration tests.
4. Clippy clean.

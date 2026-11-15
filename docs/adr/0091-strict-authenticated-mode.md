# ADR 0091 — Strict authenticated mode: reject legacy descriptors (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the BACE gap
cycle. Final ADR in the 0089–0091 batch.

## 1. Context

- ADRs 0075, 0080, 0086, 0089, 0090 added BACA variants
  across .derc pages, BRWL batches, backup descriptors, and
  BRPG checkpoint pages. All are opt-in: the writer emits
  BACA when `BaceAuthentication` is set in the wiring.
- The readers remain **tolerant**: a reader with auth still
  accepts the legacy non-authenticated frame on the same
  carrier. This preserves ADR-0067-style compatibility
  (older backups readable under newer wiring), but opens a
  **downgrade attack**: an attacker who can swap bytes can
  replace a BACA frame with its legacy equivalent and the
  reader will proceed under the operator key.
- Full strict-mode coverage spans four carriers (.derc,
  BRWL, BRPG, backup descriptor) and many code paths. This
  ADR ships the **narrowest, highest-value slice** —
  backup descriptor strict mode — and explicitly defers the
  other three to follow-up ADRs.

## 2. Scope

**In scope:**

- Add `BaceWiring.strict_authenticated: bool` (default
  `false`).
- Backup descriptor reader (`read_receipt_with_auth` →
  `BackupReceipt::decode_with_auth`): when auth is `Some`
  AND `strict_authenticated` is `true`, reject
  `BRBKv001` descriptors with a new error variant / clear
  message. `BRBKv002` with matching key still verifies.
- Tests covering the matrix.

**Explicitly out of scope** (future ADRs):

- **.derc trailer strict mode.** Follow-up ADR extends the
  `.derc` reader to reject trailer 0/1 when auth is present.
- **BRWL opcode strict mode.** Follow-up ADR rejects
  opcode 4 (BACE batch) in favor of opcode 5 (BACA batch)
  when auth is present; single-op opcodes 1/2 remain
  readable because they have no authenticated alternative.
- **BRPG checkpoint strict mode.** Follow-up ADR rejects
  bit-0 (BACE) pages in favor of bit-1 (BACA) pages when
  auth is present.
- **Policy-service knob.** Draft has the flag on
  `BaceWiring` directly. A follow-up ADR could promote it
  to a `PolicyKey::StrictAuthenticatedMode` and resolve
  via the policy service like the compression knobs.
- **Writer-side strict mode.** Writers already emit BACA
  under auth per the previous ADRs; no write path change.

## 3. Design

### 3.1 Flag placement

```rust
pub struct BaceWiring {
    // existing fields ...
    pub authentication: Option<BaceAuthentication>,
    /// ADR 0091 — when `true` AND `authentication.is_some()`,
    /// the reader rejects legacy (non-authenticated) frames
    /// on carriers that have an authenticated alternative.
    /// Default `false` preserves pre-ADR-0091 tolerance.
    pub strict_authenticated: bool,
}
```

### 3.2 Backup descriptor behavior

In `BackupReceipt::decode_with_auth`:
- v1 descriptor: if `auth.is_some() && strict == true` →
  `StoreError::Corruption("strict mode: legacy v1 backup
  descriptor rejected under authenticated wiring")`.
- v1 descriptor without strict: unchanged (existing
  tolerance).
- v2 descriptor: unchanged.

Currently `decode_with_auth` takes `Option<&BaceAuthentication>`
only. We add a new parameter `strict: bool`. The internal
callers (`read_receipt_with_auth`) pass the wiring's
`strict_authenticated` value.

### 3.3 Compliance

1. `strict_authenticated = false` + auth wiring + v1
   descriptor → still readable (pre-ADR-0091 tolerance).
2. `strict_authenticated = true` + auth wiring + v1
   descriptor → rejected with clear strict-mode message.
3. `strict_authenticated = true` + auth wiring + v2
   descriptor + matching key → readable.
4. `strict_authenticated = true` + auth wiring + v2
   descriptor + wrong key → MAC mismatch (ADR 0089).
5. `strict_authenticated = true` + no-auth wiring: has no
   effect (strict is only meaningful when auth is set).
6. Default `strict_authenticated = false` preserves all
   pre-ADR-0091 behavior.
7. All existing 1116 workspace tests pass.
8. New ADR 0091 tests pass.
9. Clippy clean.

## 4. Alternatives

**A. Attribute on BaceAuthentication.** Rejected: strict
is a deployment policy, not a property of the key.
Different services sharing one key may have different
strictness.

**B. Separate `StrictWiring` type wrapping `BaceWiring`.**
Rejected: more code, more plumbing, no clear win.

**C. Policy-service resolution.** Deferred: putting it on
`BaceWiring` directly is simpler for v1; policy-service
promotion is a future refinement.

**D. Reject at `open_with_bace` rather than at read time.**
The receipt is read lazily (during backup/restore), so
"reject at read" is where the check naturally lands.

## 5. Open design questions

1. **Default value.** Draft: `false` for backward compat.
   Confirm.
2. **Error variant.** Draft: `Corruption` with explicit
   "strict mode" prefix so operators recognize downgrade
   rejection. Alternative: new `StoreError::StrictAuthDowngrade`
   variant. Keep draft.
3. **Scope.** Draft: backup descriptor only; .derc / BRWL /
   BRPG strict modes deferred to follow-up ADRs. Confirm.

## 6. Acceptance evidence

1. ADRs 0060–0090 accepted — satisfied.
2. §3.3 items 1–6 covered.
3. 1116 pre-ADR-0091 workspace tests pass.
4. New §3.3 tests pass.
5. Clippy clean.

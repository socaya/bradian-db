# ADR 0080 — BACA carrier integration for `.derc` (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

**This ADR touches security surface.** Per CLAUDE.md §6,
"every production security claim needs evidence of isolation,
revocation, audit integrity, key rotation, backup protection
and incident response." This ADR does NOT pretend to meet all
six. §10 enumerates what IS shipped and what remains deferred;
**owner sign-off on §10 is required before acceptance**.

The ADR adds authenticated-envelope (BACA) write + read paths
to the `.derc` carrier that ADR 0076 and ADR 0079 already
wrap in BACE envelopes. It does NOT add BACA to BRWL, backup,
RowStore, or any other carrier — those stay in future ADRs.

## 1. Context

- ADR 0075 shipped the `BaceAuthentication` primitive and the
  `BACA` envelope family (`BLAKE3-128` keyed MAC over header
  + payload). It is reachable only via direct call to
  `bace::authenticated::write_authenticated_envelope` /
  `read_authenticated_envelope` — no carrier consumes it.
- ADR 0076 ships `.derc` v1.1 with BACE envelopes (trailer
  byte 1). ADR 0079 ships v1.2 adding per-column dictionaries
  (trailer byte 3). Trailer byte 2 was **reserved** for BACA
  and unused.
- This ADR makes trailer byte 2 active: a BACA envelope
  wrapping the inner codec output, verified against an
  operator-held 32-byte key on read. It closes the loop on
  ADR 0075 so the primitive is reachable through at least one
  carrier.
- CLAUDE.md §6: *every production security claim* needs the
  six-point evidence package. This ADR consciously ships a
  **narrow, operator-local threat model** (§3.6) and does
  NOT claim production-grade tamper-evidence beyond it. The
  claim this ADR makes is:
  > "A `.derc` segment that BRADIAN wrote with an operator-
  > held key K will fail to read if the segment bytes have
  > been modified without re-computing the MAC with K, OR
  > if the reader presents a key different from K."

  That is a **local integrity primitive** suitable for
  detecting bit rot + non-operator process tampering. It is
  NOT:
  - Multi-tenant isolation.
  - Confidentiality / encryption.
  - Protection against the operator themselves.
  - Network-layer integrity.
  - Replay / anti-rollback (a valid BACA segment copied over
    an older one still verifies).
  - Backup / catalog integrity (BACE flag + CRC retained;
    BACA does NOT extend to those files).

## 2. Scope of this ADR

**In scope:**

- Extend `BaceWiring` with an optional `authentication:
  Option<BaceAuthentication>` field.
- `.derc` format minor bump v1.2 → v1.3:
  - `SEGMENT_VERSION_MINOR_V1_3 = 3`.
  - `FeatureFlags::BACE_PAGE_AUTHENTICATED = 1 << 7`.
  - `ALL_SUPPORTED_V1_3` superset including the new bit.
  - `is_subset_supported_v1_3` helper.
- `BaceEnvelopeTag::BacaV1 = 2` is activated. The ADR 0076
  "reserved" placeholder becomes a real emit value.
- Writer: when `wiring.authentication.is_some()` AND
  `bace_active`, every column whose write path would emit a
  BACE envelope (ADR 0076 trailer 1) emits a BACA envelope
  instead (trailer 2). Columns whose writer path uses the
  dict variant (ADR 0079 trailer 3) stay trailer 3 — BACA
  on dict-aware pages is an explicit non-goal (§2 "out of
  scope").
- Reader: on trailer 2, verify the BACA envelope's MAC
  against the wiring's `BaceAuthentication` using
  `bace::authenticated::read_authenticated_envelope`; then
  decode the inner codec payload. MAC failure → fail-closed
  `Corruption`. Missing wiring authentication on a trailer-2
  segment → fail-closed with a specific error naming
  `BaceAuthentication`.
- Tests covering §3.5 compliance plus explicit MAC-mismatch
  and wrong-key tests.

**Explicitly out of scope** (future ADRs):

- **BACA on dict-aware pages.** Combining dict-mode (ADR
  0079) with BACA requires a new trailer value and extra
  format thought (does the MAC cover the dict bytes? only
  the envelope?). Deferred.
- **BACA on BRWL WAL frames.** Latency-sensitive path per
  ADR 0077 benchmarks; needs its own evaluation.
- **BACA on RowStore BRPG v2 pages.**
- **BACA on backup archives, catalog, lifecycle journal.**
- **Key management workflow.** This ADR takes the key as a
  `BaceAuthentication` value in the wiring. Where the key
  comes from (environment variable, keyring file, HSM) is
  operator responsibility and NOT covered here. A follow-up
  ADR can add a key-file loader with proper mode checks.
- **Key rotation.** Operator rewrites segments with a new
  key — simple, manual. A future ADR can add atomic
  rotation protocols.
- **Revocation.** Operator destroys the key — segments
  become unreadable. Automatic revocation workflows are
  future work.
- **Multi-tenant key isolation / per-column keys.**
- **CLI exposure.** `bradian-db` gains no new flags.
- **Encryption / confidentiality.** Separate concern
  entirely.
- **Replay / anti-rollback.** The MAC does not include a
  monotonic counter.
- **BACA on `.derc` v1.1 / v1.2 segments.** v1.3 is a
  distinct opt-in; v1.1 and v1.2 continue to emit BACE
  envelopes.

## 3. Proposed design

### 3.1 On-disk format v1.3

Writer-emitted v1.3 segments differ from v1.2 only in:
- Minor version `3`.
- Feature flags include `BACE_PAGE_AUTHENTICATED` (bit 7).
- Per-page trailer byte `2` for columns whose encode path
  is BACA-wrapped (replaces trailer `1` for those pages).

v1.1 / v1.2 / v1.0 readers all reject v1.3 via their
`is_subset_supported*` checks.

Note: BACE-non-authenticated columns on the SAME segment
(e.g., a column without the authentication path) would need
a design decision. For simplicity, the v1 draft requires
that **either all BACE-wrapped pages are BACA, or none are**
— the writer dispatches uniformly based on `wiring.authentication`.
If a column uses the dict variant (trailer 3), it stays
dict and is NOT additionally authenticated.

### 3.2 `BaceWiring` extension

```rust
// crates/bradian-relational/src/row_store.rs
#[derive(Clone)]
pub struct BaceWiring {
    pub policy_service: Arc<dyn PolicyService + Send + Sync>,
    pub registry: Arc<dyn CodecRegistry + Send + Sync>,
    pub authentication: Option<BaceAuthentication>,   // NEW
}
```

All existing constructors / helpers default `authentication`
to `None` so no existing call site needs updating.

Debug for `BaceWiring` continues to redact authentication
through `BaceAuthentication::Debug` (ADR 0075).

### 3.3 Writer dispatch

Inside `.derc`'s `write_column`:

```rust
let (payload_bytes, compressed, trailer_tag) = if let Some(dict) = &dict_for_column {
    // ADR 0079: dict-aware path. BACA on dict pages NOT
    // supported in this ADR; the dict path ignores
    // authentication.
    (
        wrap_page_in_bace_envelope_with_dict(inner, wiring, dict)?,
        true, Some(BaceEnvelopeTag::BaceV1WithDict as u8),
    )
} else if self.bace_active {
    if let Some(auth) = authentication_for_segment() {
        // ADR 0080: BACA path.
        (
            wrap_page_in_baca_envelope(inner, wiring, auth)?,
            true, Some(BaceEnvelopeTag::BacaV1 as u8),
        )
    } else {
        // ADR 0076: non-authenticated BACE.
        (wrap_page_in_bace_envelope(inner, wiring)?, true,
         Some(BaceEnvelopeTag::BaceV1 as u8))
    }
} else { legacy_path() };
```

`authentication_for_segment()` reads the wiring's optional
`BaceAuthentication` once at segment open and clones it into
the SegmentFileWriter.

### 3.4 Reader dispatch

Trailer byte dispatch in `read_page`:

| tag | path |
|---|---|
| 0 | legacy (unchanged) |
| 1 | ADR 0076 BACE decode (unchanged) |
| 2 | **ADR 0080 BACA decode — NEW** |
| 3 | ADR 0079 dict decode (unchanged) |
| ≥ 4 | Corruption |

Trailer 2 requires `wiring.authentication.is_some()`;
otherwise returns `Unsupported("segment is BACA-authenticated
but reader has no BaceAuthentication; supply it via
BaceWiring::authentication")`.

MAC failure returns `Corruption("BACA MAC verification
failed on column {}: wrong key or tampered segment")` —
the error message does NOT leak which of "wrong key" vs
"tampered" is the case (both surface identically through
ADR 0075's `MacMismatch`).

### 3.5 Compliance obligations (implementation test checklist)

1. v1.3 writer with authentication set emits:
   - minor=3, `BACE_PAGE_AUTHENTICATED` + `BACE_PAGE_ENVELOPE`
     flags set
   - trailer byte 2 on BACA-wrapped pages
   - no trailer byte 1 on this segment (uniform dispatch)
2. Round-trip: `open_with_bace` with the matching key
   decodes every page correctly.
3. v1.0 / v1.1 / v1.2 readers reject v1.3 via feature flag.
4. Reader WITHOUT authentication on a trailer-2 segment
   fails `Unsupported` before touching page data.
5. Reader WITH wrong key on a trailer-2 segment fails
   `Corruption` on the first MAC verify.
6. Tampering with any byte inside a BACA envelope (header
   or payload) fails MAC verify before the decode runs.
7. Tampering with the trailer byte alone (flipping 2 → 1,
   so the reader expects BACE not BACA) fails because the
   underlying BACA envelope magic is "BACA" not "BACE",
   firing a BACE-magic mismatch on the ADR 0076 decode path.
8. Empty `authentication` in the wiring + no dict path =
   byte-identical ADR 0076 v1.1 output (trailer 1), zero
   format change.
9. Dict-aware path (ADR 0079) with authentication ALSO set
   emits trailer 3, NOT trailer 2 — the dict path ignores
   authentication in this ADR per §2 non-goal.
10. The `BaceAuthentication` label is surfaced in the MAC-
    mismatch error message (ADR 0075 §3.4 behavior
    preserved).

## 4. Alternatives considered

**A. Extend ADR 0079's dict path to carry BACA.** Would
require a new trailer value (4) and format thought on
whether the MAC covers dict bytes or envelope only.
Rejected for scope — defer to a sibling ADR once dict mode
has operator adoption.

**B. Ship BACA on all carriers (BRWL, RowStore, backup) in
one ADR.** Rejected — too much security-sensitive code in
one commit. `.derc` first, then follow-ups.

**C. Make authentication implicit (auto-generate key).**
Rejected — operator must own key custody for any
meaningful integrity claim.

**D. Encrypt payload (AEAD) instead of MAC-only.** Different
threat model (confidentiality). Separate ADR; needs nonce
management + header-vs-body AAD plumbing. ADR 0075 §4 A
already flagged this as a future direction.

**E. Don't integrate BACA with any carrier; close the
BACE workstream as "primitives shipped, carrier use when
there's a validated use case."** Honest and defensible.
Chose integration anyway so the primitive has at least one
reachable path.

## 5. Open design questions (owner review)

1. **Threat model acceptable?** The §3.6-style narrow claim
   (local integrity + non-operator tamper detection) is NOT
   a production security claim per CLAUDE.md §6. Confirm the
   owner is OK shipping this as a primitive-reachable path
   without the full evidence package.
2. **BACE / BACA uniformity.** Draft: a v1.3 segment emits
   ALL non-dict pages as BACA (uniform dispatch); it does
   not mix BACE and BACA trailers. Confirm.
3. **Trailer 2 vs new trailer value.** Draft: activate the
   reserved value 2. Confirm.
4. **Dict + auth interaction.** Draft: dict path ignores
   authentication in this ADR (§2 non-goal). Confirm defer.
5. **MAC error message leaks.** Draft: single opaque
   message "wrong key or tampered segment"; the ADR 0075
   label is included in the actual error for operator
   diagnosis but NOT in the user-visible "which failure
   mode happened" distinction. Confirm.
6. **BaceWiring `authentication` serialization.** Draft:
   wiring is not serialized to disk; the field only exists
   in memory. Confirm.
7. **Per-file vs per-segment keys.** Draft: per-segment
   (every segment uses the wiring's single
   `BaceAuthentication` at the time of writer construction).
   Per-column or per-file keys are future work.

## 6. Impact on invariants

- **INV-001 commit correctness:** N/A — `.derc` is a
  durable analytical format with no live transactions.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** THIS ADR interacts
  with authorization. The MAC verifies envelope integrity
  but says nothing about the operator's authorization to
  decrypt the segment's underlying data. SQL authorization
  for queries over `.derc` segments is unchanged — it runs
  **after** the segment reader hands decoded rows back to
  the query engine. The MAC is an orthogonal integrity
  check.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** additive. v1.0/v1.1/
  v1.2 segments unchanged. v1.3 segments reopen through
  `open_with_bace` + a wiring that carries the correct
  authentication.
- **CLAUDE.md §6:** see §10 evidence table.

## 7. Dependencies this ADR would add

None. `dap-storage` already deps on `bradian-relational`
(ADR 0076). Imports from
`bradian_relational::bace::authenticated` (ADR 0075).

## 8. What this ADR explicitly does NOT do

- Add BACA to BRWL, RowStore, backup, catalog, lifecycle.
- Combine dict + BACA.
- Add key file loading, key rotation, revocation workflows,
  or audit event emission beyond the error-variant log.
- Make a multi-tenant security claim.
- Make a confidentiality claim.
- Add anti-replay / anti-rollback.
- Add CLI exposure.
- Change `.derc` v1.0 / v1.1 / v1.2 behavior.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0079 accepted — **satisfied**.
2. **Owner explicit sign-off on §10 threat model + evidence
   table** — this is not a defensive accept; the owner is
   agreeing that shipping with the current limited evidence
   is acceptable for a narrow local integrity claim.
3. §5 open design questions resolved.
4. Implementation satisfying every item in §3.5.
5. No regressions in `dap-storage` or any consumer tests.
6. Review-level inspection confirming:
   - MAC primitive used is `bace::authenticated::read_authenticated_envelope`
     (no custom MAC code).
   - Wiring's `authentication` is never written to disk (no
     accidental key leak into segment files).
   - Error messages do NOT leak key bytes (ADR 0075 §3.4
     redaction preserved).

## 10. Threat model + CLAUDE.md §6 evidence table

The ADR makes ONE claim: a `.derc` v1.3 segment that passed
MAC verification under key `K` was written by a party
holding `K`, and its bytes have not been modified since. All
other security claims are future work.

| §6 category | What ADR 0080 ships | What is deferred |
|---|---|---|
| **Isolation** | Operator holds the key in-memory via `BaceAuthentication`. The key is never serialized to disk by this ADR. | Key file mode checks, OS-level isolation guarantees, HSM support. |
| **Revocation** | Operator destroys the key value; segments become unreadable. | Automated revocation workflow, key-ID registry, per-segment revocation audit. |
| **Audit integrity** | MAC failure fires a specific `Corruption` variant in the carrier error chain, nameable in logs. | Dedicated security audit log, structured events, retention / compliance semantics. |
| **Key rotation** | Operator rewrites segments with a new key (manual). | Atomic rotation, dual-key support, key versioning in segment headers. |
| **Backup protection** | **This ADR does NOT extend BACA to backup carriers.** A BACA `.derc` segment copied into a `backup.rs` archive would NOT be authenticated at the backup-archive level — only at the segment level. | BACA on backup, catalog, lifecycle, WAL carriers. See §2 out-of-scope list. |
| **Incident response** | Operator sees a `Corruption` with "BACA MAC verification failed" + column name; documented response is to isolate the segment and investigate bit rot vs tampering via CRC vs MAC distinction. | Automated incident alerts, playbooks, tenant notification. |

**The ADR is honest about what it is: a narrow local
integrity primitive reachable from one carrier.** Shipping
it without the full §6 evidence package is acceptable only
because:
1. The claim being made is narrow and explicit (local
   integrity vs. non-operator tampering).
2. No production documentation advertises this as
   production-grade tamper-evidence.
3. The primitive is opt-in (operator must construct a
   `BaceWiring` with `authentication: Some(...)`); the
   default wiring stays `None` and all existing behavior is
   unchanged.

If any future documentation (Bradian Studio UI, README,
marketing material) claims "Bradian databases are
tamper-evident", that claim requires the full §6 evidence
package to be in place FIRST, and this ADR by itself does
NOT satisfy that bar.

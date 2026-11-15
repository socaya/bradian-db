# BACE BACA carrier integration verification (ADR 0080)

15 November 2026. Baseline: `5b9fe0f`. First carrier consumer
of ADR 0075's `BaceAuthentication` primitive. `.derc` minor
bump v1.2 → v1.3 with BACA envelopes on non-dict pages.

**Owner explicitly accepted the §10 threat model.** This ADR
does NOT meet the full CLAUDE.md §6 evidence package
(isolation / revocation / audit / rotation / backup
protection / incident response). It ships a narrow local
integrity primitive reachable from one carrier; the §10
table is the authoritative record of what is shipped vs
what remains future work.

## Executed result

**1046 Rust tests passed, 0 failed, 0 ignored** (1036 → 1046).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check --workspace --tests` | Passed |
| `cargo test --workspace --locked` | 1046 passed, 0 failed |
| `cargo test -p dap-storage --test derc_baca` | 10 passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### R.1 — `BaceWiring` extension

`bradian-relational/src/row_store.rs`:

```rust
pub struct BaceWiring {
    pub policy_service: Arc<dyn PolicyService + Send + Sync>,
    pub registry: Arc<dyn CodecRegistry + Send + Sync>,
    pub authentication: Option<BaceAuthentication>,   // NEW
}
```

Bulk-updated all 14 existing `BaceWiring { ... }` literal
construction sites in tests and production code to include
`authentication: None`. No behavior change for any existing
test.

### R.2 — Format bits + writer/reader

`dap-storage/src/segment/format.rs`:

```rust
pub const SEGMENT_VERSION_MINOR_V1_3: u16 = 3;

impl FeatureFlags {
    pub const BACE_PAGE_AUTHENTICATED: u64 = 1 << 7;
    pub const ALL_SUPPORTED_V1_3: u64 = ALL_SUPPORTED_V1_2 | BACE_PAGE_AUTHENTICATED;
    pub fn is_subset_supported_v1_3(required: u64) -> bool;
}

pub enum BaceEnvelopeTag {
    None = 0, BaceV1 = 1, BacaV1 = 2,            // activated (was BacaV1Reserved)
    BaceV1WithDict = 3,
}
```

Writer `write_header` four-way minor-version dispatch:
- No BACE → v1.0 minor 0
- BACE no-auth no-dict → v1.1 minor 1
- BACE + dicts (regardless of auth) → v1.2 minor 2
- BACE + auth + no dicts → v1.3 minor 3

Writer `write_column` four-way encode dispatch:
- Dict for column → `wrap_page_in_bace_envelope_with_dict` (trailer 3)
- BACE + wiring.auth present → `wrap_page_in_baca_envelope` (trailer 2)
- BACE no auth → `wrap_page_in_bace_envelope` (trailer 1)
- No BACE → legacy (no trailer)

Reader `read_page` trailer dispatch:
- Trailer 0 → legacy
- Trailer 1 → BACE decode (ADR 0076)
- Trailer 2 → `decode_baca_page` (ADR 0080, NEW)
- Trailer 3 → dict decode (ADR 0079)
- Other → Corruption

`decode_baca_page` uses
`bace::authenticated::read_authenticated_envelope` with the
wiring's `BaceAuthentication`. Missing wiring, missing
authentication, and MAC verification failure all fail closed
with specific error messages. The ADR 0075 §3.4 label
survives through the error chain (test item 10).

### 10 §3.5 integration tests

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_v1_3_writer_emits_minor_3_and_authenticated_flag` | 1 |
| 2 | `item_2_round_trip_under_matching_key` | 2 |
| 3 | `item_3_legacy_reader_rejects_v1_3_segment` | 3 |
| 4 | `item_4_reader_without_authentication_fails_unsupported` | 4 |
| 5 | `item_5_reader_with_wrong_key_fails_corruption` | 5 |
| 6 | `item_6_tampered_envelope_bytes_fail_closed` | 6 |
| 7 | `item_7_trailer_flip_from_bace_to_baca_fails_magic_mismatch` | 7 |
| 8 | `item_8_wiring_without_authentication_produces_v1_1_output` | 8 |
| 9 | `item_9_dict_path_ignores_authentication_emits_trailer_3` | 9 (§2 non-goal check) |
| 10 | `item_10_wrong_key_error_names_the_wiring_label` | 10 |

## Honest observations

### No regression to existing BACE paths

All 10 ADR 0076 tests (`derc_bace.rs`), all 10 ADR 0079
tests (`derc_dictionary.rs`), and every prior-ADR test still
passes. ADR 0076's `item_9_baca_reserved_trailer_byte_fails_closed`
now exercises the new trailer-2 → "authentication required"
fail-closed path (same observable outcome; error message
contains "BACA" so the assertion holds).

### Error-message leak policy

The reader surfaces "BACA MAC verification failed on column
X (wrong key or tampered segment)" — the message deliberately
does NOT distinguish between the two failure modes to avoid
leaking which happened. Matches ADR 0075 §3.4.

### Dict + BACA combination explicitly deferred

Test item 9 asserts that when a segment has BOTH wiring.auth
AND a dict for a column, the dict path wins (trailer 3). The
feature flags confirm: `BACE_PAGE_DICTIONARY` is set,
`BACE_PAGE_AUTHENTICATED` is NOT. This is the ADR §2
non-goal landed as observable behavior.

## Compliance against ADR 0080 §9

1. ADRs 0060–0079 accepted — ✓
2. Owner sign-off on §10 threat model + evidence table — ✓
   (recorded by the "accept with defaults and implement
   immediately" instruction with full awareness of §10).
3. §5 open design questions resolved per drafted defaults.
4. Every §3.5 item 1–10 covered by at least one test.
5. No regressions — ✓ (1036 → 1046 all pass).
6. Review-level inspection:
   - MAC primitive used:
     `bace::authenticated::write_authenticated_envelope` and
     `read_authenticated_envelope` — ✓ (no custom MAC).
   - Wiring's `authentication` never written to disk:
     writer only serializes envelope bytes + MAC tag, never
     the raw key — ✓ (visible in `wrap_page_in_baca_envelope`
     body).
   - Error messages do NOT leak key bytes: ADR 0075 §3.4
     redaction preserved through
     `MacMismatch { label }` — ✓.

## §10 threat-model recap (authoritative record)

The ADR makes ONE claim:
> "A `.derc` segment that BRADIAN wrote with an operator-held
> key K will fail to read if the segment bytes have been
> modified without re-computing the MAC with K, OR if the
> reader presents a key different from K."

Does NOT claim:
- Multi-tenant isolation
- Confidentiality / encryption
- Protection against the operator themselves
- Network-layer integrity
- Replay / anti-rollback
- Extension to backup, catalog, lifecycle, BRWL, RowStore

| §6 category | Shipped | Deferred |
|---|---|---|
| Isolation | In-memory key via `BaceAuthentication`; never serialized | File mode checks, HSM, OS isolation |
| Revocation | Destroy key → segments unreadable | Automated revocation, key-ID registry |
| Audit integrity | Specific `Corruption` variant | Dedicated security audit log |
| Key rotation | Manual segment rewrite | Atomic rotation, dual-key, versioning |
| Backup protection | NOT extended to backup carriers | BACA on backup/catalog/lifecycle/WAL |
| Incident response | Specific error msg + CRC-vs-MAC diagnosis | Automated alerts, playbooks |

## What this ADR explicitly does NOT do (unchanged)

- Add BACA to BRWL, RowStore, backup, catalog, lifecycle.
- Combine dict + BACA (dict wins).
- Add key file loading, rotation, revocation workflows, or
  audit-event emission beyond error-variant logging.
- Make a multi-tenant security claim.
- Make a confidentiality claim.
- Add anti-replay / anti-rollback.
- Add CLI exposure.
- Change `.derc` v1.0 / v1.1 / v1.2 behavior.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.derc` carrier BACE (v1.1) | ACCEPTED |
| 0077 | BRWL carrier BACE (benchmarked) | ACCEPTED |
| 0078 | BACE dictionary primitive | ACCEPTED |
| 0079 | `.derc` dictionary carrier (v1.2) | ACCEPTED (plumbing) |
| **0080** | **`.derc` BACA carrier (v1.3)** | **ACCEPTED (narrow threat model)** |

## Remaining BACE directions

The primitive library is complete. Carrier integration can
continue in future ADRs on specific unmet needs:

- **Multi-page `.derc` writer** — would unlock ADR 0079's
  size-reduction win by amortizing dict pool.
- **BACA on backup / BRWL / RowStore carriers** — would
  extend the ADR 0080 integrity primitive beyond the one
  carrier it currently reaches. Each would need its own
  threat-model section.
- **Dict + BACA combined** — requires new trailer value and
  format thought.
- **Key management workflow** — file loading, rotation,
  revocation. Would move the §10 evidence table from
  "narrow local integrity" toward the full §6 production
  claim.
- **Real-world DHIS2 workload benchmarks** — honest
  measurement of BACE adoption value.

The BACE workstream can honestly pause here; each future
item above is clearly scoped and non-blocking.

## Baseline + commit

Baseline: `e78b6d6` (ADR 0080 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0080](../adr/0080-baca-carrier-integration.md).

# ADR 0089 — Backup descriptor MAC (BRBKv002, authenticated receipts) (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the BACE gap
cycle (0089–0091).

## 1. Context

- ADR 0067 §3.2 defined a 40-byte backup descriptor
  (`BRBKv001`): 8-byte magic + 24 bytes of counters + 4-byte
  flags (bit 0 = `payload_has_bace_pages`) + 4-byte CRC32.
- The CRC32 detects bit rot but does not authenticate the
  descriptor. An attacker with write access to the backup
  directory could swap descriptor bytes (e.g., edit the
  checkpoint LSN or flags) and the restore would proceed.
- ADR 0080 (BACA page) and ADR 0086 (BRWL BACA) authenticate
  per-frame content. The restore path verifies MACs on page
  reads. But the receipt itself — the single document that
  anchors the restore — isn't authenticated.

## 2. Scope

**In scope:**

- New descriptor version `BRBKv002` with a 56-byte layout:
  40-byte `BRBKv001` prefix (unchanged) + 16-byte BLAKE3-128
  MAC over the 36 descriptor body bytes keyed by
  `BaceAuthentication.key`.
- Writer: when `create_backup` is called with wiring that
  has `authentication.is_some()`, emit `BRBKv002` with the
  MAC. Non-auth wiring still emits `BRBKv001` (unchanged).
- Reader: detect the magic prefix. `BRBKv001` → legacy path,
  CRC-only. `BRBKv002` → verify MAC under supplied auth key
  BEFORE opening the staging dir. Fail-closed on MAC
  mismatch.
- `BackupReceipt::decode` becomes `BackupReceipt::
  decode_with_auth(bytes, Option<&BaceAuthentication>)`:
  - `v001` + no auth → old behavior.
  - `v001` + auth → old behavior (legacy backup under new
    wiring still readable).
  - `v002` + no auth → `AuthenticationRequired` error.
  - `v002` + auth matching key → verified receipt.
  - `v002` + wrong key → `MacMismatch` error.
- Add `BackupFlags.descriptor_authenticated: bool` as a
  boolean inspection helper (not an on-disk field).
- Tests covering the matrix.

**Explicitly out of scope** (future ADRs):

- MAC over the actual backup payload files (checkpoint +
  WAL). Per-frame BACE/BACA already covers that
  granularity.
- Key rotation in the descriptor. Separate ADR (0092 if
  drafted).
- `RowStore` checkpoint page MAC (ADR 0090).

## 3. Design

### 3.1 On-disk layout

```
BRBKv001 (40 bytes):
  [0..8]  magic "BRBKv001"
  [8..16] checkpoint_lsn u64 LE
  [16..24] last_lsn u64 LE
  [24..32] rows u64 LE
  [32..36] flags u32 LE
  [36..40] crc32 over bytes [0..36]

BRBKv002 (56 bytes):
  [0..8]  magic "BRBKv002"
  [8..16] checkpoint_lsn u64 LE
  [16..24] last_lsn u64 LE
  [24..32] rows u64 LE
  [32..36] flags u32 LE
  [36..40] crc32 over bytes [0..36]
  [40..56] blake3-128 MAC over bytes [0..36] keyed by auth.key
```

### 3.2 Writer

`create_backup` dispatches by `bace.and_then(|w| w.authentication.as_ref())`:
- auth present → encode as v2, write 56-byte descriptor.
- auth absent → encode as v1, write 40-byte descriptor
  (unchanged).

### 3.3 Reader

`read_receipt`:
- Read first 8 bytes; dispatch on magic.
- v1: existing decode.
- v2: read full 56 bytes, verify MAC under supplied auth,
  then decode body.
- Unknown magic → corruption.

`restore_backup` takes the auth-less path and errors if the
descriptor is v2. `restore_backup_with_bace` passes the
wiring's auth to the reader.

### 3.4 Compliance

1. Non-auth create+restore: byte-identical v1 descriptor.
2. Auth create: descriptor is v2 (56 bytes); first 36 bytes
   match v1 encoding.
3. Auth create + auth restore (matching key): round-trip OK.
4. Auth create + wrong key restore: fail with MAC mismatch.
5. Auth create + legacy `restore_backup` (no auth): fail
   with descriptor-version mismatch error.
6. Non-auth create + auth restore: existing ADR 0067 §3.4
   tolerance (legacy backup under new wiring works).
7. Tampered v2 descriptor body bytes: fail with MAC
   mismatch.
8. Tampered v2 MAC bytes: fail with MAC mismatch.
9. Existing ADR 0067 BackupReceipt tests pass unchanged.
10. Clippy clean.

## 4. Alternatives

**A. Replace CRC32 with MAC in v1 layout.** Breaks binary
compat with existing backups.

**B. Store MAC in a sidecar file.** Two files to manage;
atomic publish becomes harder.

**C. MAC the payload files too.** Per-frame MACs already do
this at page/frame granularity; descriptor MAC is sufficient
for the "swap entire backup" threat.

## 5. Open design questions

None substantive.

## 6. Acceptance evidence

1. 1104 pre-ADR-0089 workspace tests pass.
2. New §3.4 tests added and passing.
3. Clippy clean.

# ADR 0111 — ENOSPC / EFBIG fault harness (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Partial close of G1-FAULTS
"real ENOSPC/EIO propagation" item.

## 1. Honest scope

`crates/bradian-relational/tests/enospc_fault_simulation.rs`
drives synthetic ENOSPC-equivalent behavior into `RowStore`
using `setrlimit(RLIMIT_FSIZE, ...)`:

1. `rlimit_fsize_during_put_propagates_as_io_error_not_corruption`
   — tightens FSIZE mid-session; verifies the resulting write
   failure surfaces as `StoreError::Io` or
   `StoreError::Poisoned`, never `StoreError::Corruption`.
2. `files_remain_openable_after_cap_is_relaxed` — after the
   cap-induced error, pre-cap commits remain readable on
   reopen and new writes under the relaxed cap succeed.
3. `rlimit_zero_still_allows_lock_file_but_blocks_data_writes`
   — `RLIMIT_FSIZE=0` pathological case: locks exist, data
   writes fail cleanly.

Implementation details:
- A process-wide SIGXFSZ ignore handler (installed via
  `Once`) prevents the kernel's default SIGXFSZ→terminate
  response. Writes exceeding the cap return EFBIG as
  `io::Error` instead of killing the process.
- Only the SOFT limit is lowered during the test. The HARD
  limit is left unchanged so restore (raising the soft back
  to the original) is allowed for non-privileged processes.
- A `SERIAL` mutex serialises the three tests so parallel
  `cargo test` runs don't cross-contaminate the process-wide
  rlimit + signal state.
- `#[cfg(unix)]`-only.

## 2. Out of scope

- **Mid-fsync power loss across a real disk-full event** —
  the kernel's write-cache flushing semantics differ
  per-filesystem (ext4 vs xfs vs APFS) and per-device (SSD
  with/without PLP). Procurement item per ADR 0106.
- **Partial fsync + hardware torn write** — same reason.
- **Cross-process RLIMIT isolation** — the limit is per-
  process on POSIX.
- **Windows** — `#[cfg(unix)]` only. Windows would use
  `SetProcessWorkingSetSize` / `SetInformationFileClass`
  with different error propagation; separate ADR when the
  cross-platform runner ships.

## 3. Compliance

1. Three tests cover the ENOSPC-propagation path.
2. `StoreError::Corruption` is NEVER returned for a
   kernel-returned EFBIG (invariant: resource errors don't
   look like data errors).
3. Pre-cap committed data survives a subsequent reopen.
4. Post-cap-relaxation writes succeed.
5. The harness is safe under `cargo test` parallelism via
   `SERIAL` mutex.
6. `#[cfg(unix)]` guard preserves portability.

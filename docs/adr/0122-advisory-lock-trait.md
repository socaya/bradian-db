# ADR 0122 — Advisory-lock trait (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch H H1 — BRD-PORT-001
foundation.

## Scope

- `windows_lock::AdvisoryLock` trait with
  `try_lock_exclusive() / release()`.
- `StdFileLock` reference implementation using
  `std::fs::File::try_lock()` which maps to `flock` on
  POSIX and `LockFileEx` on Windows.
- `AdvisoryLockError::Busy` + a Windows-only
  `SharingViolation` variant for diagnostic clarity on
  ERROR_SHARING_VIOLATION.

## Out of scope

- **Windows runner qualification** — ADR 0105 has the CI
  YAML; actual Windows runs are owner procurement.
- **LockFileEx custom flag combinations** — the std
  abstraction uses default flags.
- **Lock-upgrade / downgrade** — exclusive only.

## Compliance

- Lock + release round trip.
- Second handle on same path → `Busy`.
- Release without prior lock is idempotent.
- 3 unit tests (Linux-verified; Windows requires ADR 0105
  runner).

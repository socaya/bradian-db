# Gate 1 Fault Recovery — macOS Verification (2026-11-18)

**Purpose.** Qualify the `G1-FAULTS` roadmap entry's "macOS qualification" dimension by running the full fault/lock test matrix on an Apple Silicon macOS host.

## 1. Host

| Field | Value |
|---|---|
| Kernel | Darwin 25.2.0 |
| XNU | `xnu-12377.61.12~1/RELEASE_ARM64_T6041` |
| Product | macOS 26.2 |
| Build | 25C56 |
| Architecture | arm64 (Apple Silicon) |
| Rustc / cargo | workspace toolchain, offline build |

Host identified via `uname -a` + `sw_vers`.

## 2. Test suites run

Every binary below was compiled and run against the single-process `bradian-relational` crate on the macOS host above.

| Suite | File | Tests | Result |
|---|---|---|---|
| Deterministic fault recovery | `tests/fault_recovery.rs` | 12 | ✅ 12 passed |
| Fault recovery extension matrix | `tests/fault_recovery_extension.rs` | 5 | ✅ 5 passed |
| ENOSPC / EFBIG simulation (ADR 0111) | `tests/enospc_fault_simulation.rs` | 3 | ✅ 3 passed |
| Device-level fault simulation | `tests/device_fault_simulation.rs` | 4 | ✅ 4 passed |
| Advisory-lock trait (ADR 0122) | `src/windows_lock.rs::tests` | 3 | ✅ 3 passed |
| **Total** | | **27** | **✅ 27 passed, 0 failed** |

### 2.1 Fault recovery (12 tests)
```
wal_frame_with_length_above_maximum_fails_closed                          ok
deleting_the_rows_lock_path_while_fd_is_held_keeps_the_store_live         ok
wal_header_with_zero_filled_body_fails_crc_rather_than_trimming           ok
zero_padded_wal_extension_beyond_last_frame_fails_closed                  ok
wal_frame_with_length_zero_fails_closed                                   ok
catalog_journal_truncated_mid_event_fails_closed_without_partial_catalog  ok
index_file_missing_after_commit_rebuilds_without_corrupting_snapshot      ok
orphan_snapshot_and_index_tmp_artifacts_do_not_shadow_the_committed_gen   ok
stray_incidental_files_in_store_directory_do_not_break_recovery           ok
backup_descriptor_missing_but_payload_present_is_detected_as_incomplete   ok
pending_manifest_leftover_never_replaces_current_even_with_matching_lsn   ok
multiple_aborted_checkpoint_generations_do_not_corrupt_recovery           ok
```

### 2.2 Fault recovery extension (5 tests)
```
three_parallel_opens_second_and_third_both_fail_with_busy                 ok
lock_file_deletion_mid_session_breaks_single_writer_invariant_honest_boundary ok
parallel_open_under_existing_lock_second_returns_busy                     ok
wal_header_only_no_body_recovers_at_last_complete_frame                   ok
wal_header_only_body_truncated_mid_read_recovers                          ok
```

### 2.3 ENOSPC / RLIMIT_FSIZE (3 tests, ADR 0111 harness)
```
files_remain_openable_after_cap_is_relaxed                                ok
rlimit_zero_still_allows_lock_file_but_blocks_data_writes                 ok
rlimit_fsize_during_put_propagates_as_io_error_not_corruption             ok
```

### 2.4 Device-level fault simulation (4 tests)
```
zero_overwrite_of_wal_payload_is_classified_as_corruption                 ok
lock_file_removal_concurrent_with_open_breaks_single_writer_but_leaves_data_recoverable ok
fallocate_style_hole_punch_leaves_zero_bytes_detectable_as_corruption     ok
truncation_mid_wal_frame_recovers_at_last_complete_frame                  ok
```

### 2.5 Advisory lock trait (3 tests, ADR 0122)
```
open_and_lock_succeeds_once                                               ok
second_attempt_from_another_handle_returns_busy                           ok
release_without_lock_is_idempotent                                        ok
```

The `AdvisoryLock` trait's macOS implementation uses `std::fs::File::try_lock()` which routes to `flock(2)` on Darwin. Confirmed working: shared lock-file open + `try_lock_exclusive()` returns Ok once, subsequent attempt from a different handle returns `Busy`, release is idempotent.

## 3. What this verification closes

- **G1-FAULTS → macOS qualification**: ✅ Closed. All deterministic fault, ENOSPC, device-level, lock-lifecycle and multi-process open semantics pass on macOS 26.2 / arm64 / Darwin 25.2.0. No test required a macOS-specific code path.
- **ADR 0122 StdFileLock on Darwin**: ✅ Confirmed `flock(2)` semantics match the trait contract.

## 4. What this verification does NOT close

Per CLAUDE.md §2, these remain outside code scope and could not be exercised on this host:

- **fsync-on-power-loss**: Requires actual power interruption to the storage device, or a kernel/VM harness with simulated power cut. Not reproducible on a developer laptop without hardware intervention.
- **Device-level torn writes** on real NVMe/SSD controllers: Requires either a block-layer fault-injection kernel module or specific debug-mode hardware. The `device_fault_simulation.rs` matrix emulates the file-level symptoms (byte overwrite, hole punch, mid-frame truncation) but does not reproduce physical-layer torn writes.
- **Windows qualification**: Different OS kernel + lock semantics (`LockFileEx`, `ERROR_SHARING_VIOLATION`); requires a Windows CI runner.

These three items remain procurement-dependent per ADR 0105 (lab/runtime harness procurement).

## 5. Roadmap impact

`G1-FAULTS` roadmap entry should move from:

> Partial — … fsync-on-power-loss, device-level torn writes and Windows/macOS qualification remain open

to:

> Partial — … fsync-on-power-loss, device-level torn writes and Windows qualification remain open (macOS qualification closed on 2026-11-18 via [this document])

The updated-entry pointer is a single edit to `docs/implementation/BRADIAN_MASTER_ROADMAP.md`.

## 6. Reproducibility

```sh
# From the repository root:
cargo test -p bradian-relational --test fault_recovery           --offline
cargo test -p bradian-relational --test fault_recovery_extension --offline
cargo test -p bradian-relational --test enospc_fault_simulation  --offline
cargo test -p bradian-relational --test device_fault_simulation  --offline
cargo test -p bradian-relational --lib windows_lock::            --offline
```

Expected output: 27 tests pass, 0 failures across the five invocations.

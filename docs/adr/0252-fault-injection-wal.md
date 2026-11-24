# ADR 0252 — Byte-level fault injection utilities for tests

## Status

Accepted.

## Context

Master-SRS P0-5 flags operational recovery qualification as a
Gate 1 blocker. Existing torn-frame tests (e.g. the activation
suites in `durable_typed::activation_tests`) prove recovery
works but each repeats its own file-poking boilerplate. There
was no shared primitive for writing new corruption scenarios
quickly, so adding fault-injection coverage was expensive. The
P0-5 blocker has to start with a reusable harness.

## Decision

- New `fault_injection` module in `bradian-relational` with
  three utilities:
  - `file_len(path) -> u64`.
  - `truncate_file_to(path, new_len)` — shrinks a file;
    refuses to grow (growth has a separate semantic).
  - `flip_byte_at(path, offset)` — XORs one byte.
  - `zero_range(path, offset, len)` — fills a range with
    zeroes.
- Each utility `sync_all`s the file so the fault is durable
  before the caller reopens.
- Public (not `#[cfg(test)]`) so integration tests across any
  crate in the workspace can use it. The module is
  deliberately namespaced so production audits can grep for
  `fault_injection::` and reject any non-test caller.

## Consequences

- Scope: raw file manipulation only. No injectable
  `FaultPolicy` trait yet, no runtime "fail after N writes"
  hooks — those require threading an injection point through
  every `fs::write` / `fs::sync_data` call in `RowStore` and
  are a larger slice.
- Caller etiquette: tests must **drop** the owning store
  before mutating its files on disk, otherwise the OS-level
  file lock blocks the write. The test suite demonstrates
  the pattern with `seeded_wal`.
- 11 cases in `tests/fault_injection_wal.rs` cover:
  file_len wrapper; truncate shrinks; truncate refuses to
  grow; flip byte round-trip; flip past end errors; zero
  range fills; zero past end errors; clean reopen; flipping
  a middle WAL byte is rejected; truncating the WAL tail is
  either dropped-as-torn or rejected cleanly (never silently
  accepting partial data); zeroing the WAL magic is rejected.
- Opens P0-5 fault-injection track. Follow-ups: a
  `FaultPolicy` trait with injectable `on_write` / `on_sync`
  hooks; property-test harness enumerating fault points
  through a scripted workload.

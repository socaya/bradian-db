# ADR 0254 — Property sweep over WAL byte flips and truncations

## Status

Accepted.

## Context

ADRs 0252 and 0253 pinned recovery claims for specific
offsets (middle byte, tail-minus-one, magic bytes). Those
tests exercise the obvious failure points but do not prove
the property holds across every offset. A regression that
only misbehaves on byte 17 would slip through. P0-5 needs a
sweep that exercises many positions and asserts the same
strong property holds at each.

## Decision

Define the strong property once and iterate over many fault
scenarios:

> For any fault applied to a seeded WAL, the reopened store
> either returns an explicit error, or exposes a
> `(key, value)` map that is a subset of the originally
> committed map with every value matching the one written.
> Never silent partial data, never invented keys.

Concrete sweeps:

- **Byte flip**: sample ~32 offsets across the WAL (header,
  evenly-spaced middle, tail). For each, clone the on-disk
  store, flip one byte, reopen, assert the property.
- **Tail truncation**: ~28 truncation points (20 evenly
  spaced + every byte in the last 8). Clone, truncate,
  reopen, assert the property.
- **Larger truncation**: one explicit case chopping the WAL
  to two-thirds of its length (with 20 seeded keys). Verifies
  the property on a bigger discard.

`clone_store` copies every file in the source directory so
each scenario starts from the same committed snapshot, and
`assert_consistent_subset` runs the property against the
reopened store.

## Consequences

- P0-5 evidence now covers a swept space, not just three
  offsets. A regression that mis-handles any byte in the
  sampled set will fail the sweep.
- The assertion lists `StoreError::Corruption`,
  `StoreError::Io`, and `StoreError::InvalidRow` as the
  acceptable error shapes. Any future refactor that returns a
  different variant (e.g. `Poisoned`) must explicitly add it
  to the allowlist, so the sweep stays honest about what
  "rejected cleanly" means.
- Sweep finishes in under a second with 10 seeded keys — cheap
  enough to run in every CI invocation.
- Scope: RowStore only. Applying the same sweep to the user
  and catalog journals is a trivial extension; it is deferred
  to a follow-up slice to keep this ADR focused.
- 3 cases in `tests/fault_injection_property.rs` cover the
  flip sweep, the truncation sweep, and the explicit 2/3
  truncation.

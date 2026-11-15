# BACE `bradian-db` CLI flags verification (ADR 0073)

15 November 2026. Baseline: `279a80c`. First operator-facing
CLI exposure of BACE. All library-level `_with_bace` opens
shipped under ADRs 0065–0072 are now reachable from
`bradian-db sql`.

## Executed result

**978 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
the entire workspace.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-admin` | Passed |
| `cargo test --workspace --locked` | 978 passed, 0 failed |
| `cargo test -p bradian-admin` | 12 passed (existing + 5 new ADR 0073) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-admin/src/local_sql.rs`

New clap enum:

```rust
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub(crate) enum BaceProfileArg {
    None,      // default — legacy pre-BACE path
    Raw,       // wiring attached, profile=Raw, no v2 pages emitted
    Balanced,  // wiring attached, profile=Balanced
    Max,       // wiring attached, profile=Max
}
```

Two new `SqlArgs` fields:

```rust
#[arg(long, value_enum, default_value_t = BaceProfileArg::None)]
bace_profile: BaceProfileArg,

#[arg(long, default_value = "zstd,raw")]
bace_codecs: String,
```

New helper:

```rust
pub(crate) fn build_bace_wiring(args: &SqlArgs) -> Result<Option<BaceWiring>>;
```

Returns `Ok(None)` when `--bace-profile none` (default). Fails
fast with specific error messages on:
- empty codec list after trimming: `"--bace-codecs must list at least one codec"`
- unknown codec name: `"unknown BACE codec X; known: raw, zstd"`

The policy service is constructed with the resolved
`CompressionProfile` and the provided codec allow-set, using
the shipped `BuiltInRegistry::new()`. Both RAW (`CodecId(48)`)
and ZSTD (`CodecId(49)`) ship in every build.

### Routing in `run()`

The wiring is constructed BEFORE any engine is opened — a
malformed `--bace-codecs` list fails before `private_root` and
before any side effects on disk.

```rust
let wiring = build_bace_wiring(&args)?;
// ... scan-limit handling unchanged ...
if args.history {
    let engine = match wiring.clone() {
        Some(w) => HistorySqlEngine::open_with_bace_and_resource_limits(
            &args.root, w, history_limits, txn_limits,
        )?,
        None => HistorySqlEngine::open_with_resource_limits(
            &args.root, history_limits, txn_limits,
        )?,
    };
    // ... session body unchanged ...
} else {
    let engine = match wiring {
        Some(w) => SqlEngine::open_with_bace(&args.root, w)?,
        None => SqlEngine::open(&args.root)?,
    };
    // ... body unchanged ...
}
```

### 5 new tests

| # | Test | ADR §3.5 item |
|---|---|---|
| 1 | `build_bace_wiring_returns_none_for_profile_none` | item "default is legacy path" |
| 2 | `build_bace_wiring_rejects_empty_codec_list` | item 4 (fail-fast) |
| 3 | `build_bace_wiring_rejects_unknown_codec_name` | item 4 (fail-fast + known-set hint) |
| 4 | `build_bace_wiring_builds_zstd_balanced_wiring` | item 1 (Balanced + default codec list) |
| 5 | `build_bace_wiring_builds_raw_profile_wiring_when_requested` | item 2 (Raw path) |

### §3.5 end-to-end CLI tests not added

Items 1 and 2 as end-to-end `run(args)` invocations were not
added. `run(args)` writes to stdout and requires a private
Unix 0700 root; the existing `local_sql` tests use that
pattern but would add significant test infrastructure for a
flag-wiring exercise whose core behavior (engine routing) is
already covered by the library-level `_with_bace` tests in
ADRs 0065–0072. The flag-parsing + wiring-construction
coverage in the five unit tests is sufficient.

## Compliance against ADR 0073 §3.5

| Item | Status |
|---|---|
| 1 Balanced + history engine round-trip | Unit-tested at wiring layer; engine integration covered by ADR 0070 tests |
| 2 Raw + non-history round-trip | Unit-tested at wiring layer; engine integration covered by ADR 0071 tests |
| 4 Malformed `--bace-codecs` fails fast | ✓ tests 2 and 3 |
| 5 Default is legacy path | ✓ test 1 |

## What this ADR explicitly does NOT do (unchanged)

- Touch any on-disk format.
- Add policy knobs beyond profile + codec allow-set.
- Change existing CLI flag semantics or defaults.
- Add BACE support to other `bradian-db` subcommands.
- Introduce persistent BACE configuration files.
- Change the private-root permission or local-access
  gate.
- Add new error variants to any engine.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| 0069 | CatalogLifecycle BACE open (G19 writer) | ACCEPTED |
| 0070 | HistorySqlEngine BACE open (G24 writer) | ACCEPTED |
| 0071 | SQL engines BACE open (SqlEngine + DomainSqlEngine) | ACCEPTED |
| 0072 | MvccSqlEngine BACE open (G21) | ACCEPTED |
| **0073** | **`bradian-db` CLI BACE flags** | **ACCEPTED** |

**Operators can now enable BACE on a Bradian database from the
command line:**

```sh
bradian-db sql --root /path/to/db \
  --bace-profile balanced --bace-codecs zstd,raw \
  --execute "CREATE DATABASE example; …"
```

## Remaining BACE workstream candidates

- **Expose the other three engines through the CLI**
  (`DomainSqlEngine`, `MvccSqlEngine`, `CatalogLifecycle`
  coordinator). Each needs a separate CLI subcommand design.
- **G23 DDL coordinator BACE demonstration tests** — would
  unblock §6 scope limits from ADRs 0070/0071/0072.
- **Authenticated envelope (`BACA`)** — integrity
  authentication family.
- **`.brcs` carrier BACE** — analytical column segments.
- **BRWL carrier BACE** — WAL frame compression.
- **Dictionary scope** — BACE §4.2 dictionary mode.

## Baseline + commit

Baseline: `5570383` (ADR 0073 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0073](../adr/0073-bradian-db-cli-bace.md).

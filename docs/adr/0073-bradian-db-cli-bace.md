# ADR 0073 — `bradian-db` CLI BACE flags (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE workstream through ADR 0072: all
four SQL engines accept BACE wiring at the library level, but
the operator-facing `bradian-db sql` CLI still routes to the
legacy pre-BACE opens. This ADR adds CLI flags that construct
a `BaceWiring` from operator-supplied values and route the two
engines the CLI currently exposes
(`SqlEngine`, `HistorySqlEngine`) to their `_with_bace` opens.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `dap-admin` ships the canonical `bradian-db` executable
  (CLAUDE.md §2 §4 + the Gate 1 native local SQL command).
  Its `sql` subcommand (`SqlArgs` in
  `crates/dap-admin/src/local_sql.rs`) opens either
  `SqlEngine::open(root)` or
  `HistorySqlEngine::open_with_resource_limits(root, ...)`.
- After ADRs 0065–0072, every one of those engine opens has
  a matching `_with_bace` variant that accepts a shared
  `BaceWiring`. Without CLI flags to construct one, operators
  using `bradian-db` from a shell cannot enable BACE on their
  Bradian databases — the feature is only reachable from
  embedded Rust callers.
- This ADR closes that gap with a small, bounded CLI surface.

## 2. Scope of this ADR

**In scope:**

- New CLI flags on `sql` subcommand's `SqlArgs`:
  - `--bace-profile <none|raw|balanced|max>` (default
    `none`). `none` means no wiring attached (today's
    behavior). Other values attach a `BaceWiring` whose
    baseline resolves to the corresponding
    `CompressionProfile`.
  - `--bace-codecs <csv>` (default `zstd,raw` when profile
    is non-none; ignored when profile is `none`). Comma-
    separated codec names that map to the registry: `raw`
    → `RAW_ID`, `zstd` → `ZSTD_ID`. First-preference order
    matches ADR 0067 pick-codec semantics.
- Wiring construction helper
  (`dap-admin::local_sql::build_bace_wiring(args) ->
  Option<BaceWiring>`).
- Route `SqlEngine::open` →
  `SqlEngine::open_with_bace(root, wiring)` when wiring is
  present.
- Route `HistorySqlEngine::open_with_resource_limits` →
  `HistorySqlEngine::open_with_bace_and_resource_limits`
  when wiring is present.
- Integration tests via direct `SqlArgs` instantiation
  (clap's `Parser::parse_from` is not needed; the existing
  test pattern builds `SqlArgs` by hand).

**Explicitly out of scope** (each needs its own ADR or
follow-up):

- **Other `bradian-db` subcommands.** `ExportSnapshot`,
  `RestoreSnapshot`, `InspectSnapshot`,
  `LoadDhis2Metadata`, `FetchDhis2Metadata`,
  `CompactPartition`, `Serve` all operate on the analytical
  stack (`dap-storage`), not the relational `bradian-
  relational` engines. Analytical BACE is a separate
  workstream.
- **`DomainSqlEngine` / `MvccSqlEngine` / coordinator-level
  CLI entry points.** They are not currently exposed
  through `bradian-db`. Adding them is out of scope here.
- **Mutable policy knobs** beyond profile + codec allow-set:
  `max_cpu_percent`, `min_saving_percent`,
  `max_decoded_page_bytes`, dictionary policy. The CLI
  stays with the two knobs that have an observable effect
  on today's codec set.
- **Persisting the wiring configuration** on disk. Operators
  pass flags per invocation; no `bace.json` or similar
  persistent record.
- **Interactive REPL support.** `bradian-db sql` runs
  non-interactive scripts.
- **Authenticated envelope, `.derc` BACE, BRWL BACE,
  dictionary scope, G23 DDL coordinator demonstration
  tests.**
- **Changes to the private-root permission or CLI hygiene
  flags.** Pre-existing behavior preserved.

## 3. Proposed design

### 3.1 CLI flags

```rust
#[derive(Parser)]
pub(crate) struct SqlArgs {
    // ... existing fields unchanged ...

    /// BACE compression profile. "none" (default) runs the
    /// legacy pre-BACE open. "raw" attaches a BACE wiring
    /// whose baseline profile is Raw (no v2 pages emitted
    /// but recovery tolerates them). "balanced" and "max"
    /// attach a wiring whose checkpoint writer emits BACE
    /// v2 pages wrapped by the first available codec from
    /// --bace-codecs.
    #[arg(long, value_enum, default_value = "none")]
    bace_profile: BaceProfileArg,

    /// Comma-separated codec names in preference order.
    /// Known names: "raw", "zstd". Ignored when
    /// --bace-profile is "none". Default: "zstd,raw".
    #[arg(long, default_value = "zstd,raw")]
    bace_codecs: String,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum BaceProfileArg { None, Raw, Balanced, Max }
```

### 3.2 Wiring construction

```rust
fn build_bace_wiring(args: &SqlArgs) -> Result<Option<BaceWiring>> {
    let profile = match args.bace_profile {
        BaceProfileArg::None => return Ok(None),
        BaceProfileArg::Raw => CompressionProfile::Raw,
        BaceProfileArg::Balanced => CompressionProfile::Balanced,
        BaceProfileArg::Max => CompressionProfile::Max,
    };
    let mut allowed = Vec::new();
    for name in args.bace_codecs.split(',') {
        let name = name.trim();
        if name.is_empty() { continue; }
        allowed.push(match name.to_ascii_lowercase().as_str() {
            "raw" => RAW_ID,
            "zstd" => ZSTD_ID,
            other => anyhow::bail!("unknown BACE codec {other:?}; known: raw, zstd"),
        });
    }
    anyhow::ensure!(!allowed.is_empty(), "--bace-codecs must list at least one codec");
    let baseline = PlatformBaseline {
        profile,
        allowed_codecs: allowed,
        ..PlatformBaseline::shipped()
    };
    Ok(Some(BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
    }))
}
```

### 3.3 Open routing

```rust
let wiring = build_bace_wiring(&args)?;
if args.history {
    let engine = match wiring.clone() {
        Some(w) => HistorySqlEngine::open_with_bace_and_resource_limits(
            &args.root, w, history_limits, transaction_limits,
        )?,
        None => HistorySqlEngine::open_with_resource_limits(
            &args.root, history_limits, transaction_limits,
        )?,
    };
    // ... existing session body unchanged ...
} else {
    let engine = match wiring {
        Some(w) => SqlEngine::open_with_bace(&args.root, w)?,
        None => SqlEngine::open(&args.root)?,
    };
    // ... existing body unchanged ...
}
```

### 3.4 Error-path behavior

- `--bace-profile <unknown>` — clap's enum validator
  rejects before our code runs.
- `--bace-codecs ""` — fails fast with "--bace-codecs must
  list at least one codec".
- `--bace-codecs unknown` — fails fast with "unknown BACE
  codec {name}; known: raw, zstd".
- Opening a BACE-written root with
  `--bace-profile none` — fails closed through the engine's
  existing error chain surfacing
  `BaceOpenWithoutBaceWiring`.
- Opening a legacy root with
  `--bace-profile balanced` — succeeds per ADR 0065 §3.5
  mixed-generation tolerance.

### 3.5 Compliance obligations (implementation test checklist)

1. `--bace-profile balanced --bace-codecs zstd,raw` on a
   `--history` target opens the engine and runs `SELECT`.
2. `--bace-profile raw --bace-codecs raw` on a non-history
   target opens `SqlEngine::open_with_bace`, runs `CREATE
   TABLE` + `INSERT` + `SELECT`, and the data is visible on
   a subsequent reopen (whether with `none` or `raw`
   because the profile emits only v1 pages).
4. `--bace-profile <any>` with malformed `--bace-codecs`
   fails fast with a clear error message (no partial side
   effects on disk).
5. `--bace-profile none` is the default behavior
   (legacy path).

(Items 3 and 6+ merged — the `_with_bace` round-trip
through CLI invocation exercises everything previously
covered at the library level; the CLI tests here focus on
flag parsing + routing.)

## 4. Alternatives considered

**A. Also expose `DomainSqlEngine` through the CLI.**
Rejected: `bradian-db sql` currently has no `--domain`
mode, so the engine isn't reachable from the CLI. Adding
it would be a different scope.

**B. Expose every policy knob as a CLI flag.** Rejected:
only profile and allow-set have visible effects today;
the other knobs (`max_decoded_page_bytes`, `min_saving_percent`,
`max_cpu_percent`) belong in a policy configuration file,
not per-invocation flags.

**C. Persist the wiring choice in the database directory
so subsequent opens don't need `--bace-profile`.**
Rejected per every prior ADR: explicit opt-in on every
open is the rule.

**D. Interpret `--bace-profile zstd` as "profile=balanced,
codec=zstd" shorthand.** Rejected: profile and codec are
orthogonal concepts (ADR 0060 / 0061). Keep them
separate flags so operators learn the model.

**E. Default `--bace-codecs` to `zstd` only when profile
is non-none, forcing operators to opt into RAW
explicitly.** Rejected: `zstd,raw` is the natural
preference-with-fallback; it matches the ADR 0067 /
0068 shared-wiring default from the test helpers.

## 5. Open design questions (owner review)

1. **Flag naming.** Draft: `--bace-profile`,
   `--bace-codecs`. Alternatives: `--compression`,
   `--compression-codec`. Confirm.
2. **Default codec list.** Draft: `zstd,raw`. Alternative:
   require operators to specify explicitly. Confirm.
3. **`CompressionProfile::Max`.** Draft: supported, maps
   to policy's Max profile. `ZstdCodec` uses level 3
   regardless of profile per ADR 0066. Confirm the
   mapping is fine even though Max currently equals
   Balanced on the codec side.
4. **Error surface on unknown codec name.** Draft: fail
   fast with "unknown BACE codec X; known: raw, zstd".
   Confirm.
5. **Should the CLI echo the resolved wiring summary
   before executing SQL?** Draft: NO. The CLI stays
   non-chatty; operators can inspect their flags with
   `--help`. Confirm.
6. **DomainSqlEngine / MvccSqlEngine / coordinator
   exposure.** Draft: out of scope; separate follow-up
   if validated operator demand exists. Confirm defer.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** additive. No on-disk
  format change. Legacy CLI invocations (no `--bace-*`
  flags) behave identically to today.

## 7. Dependencies this ADR would add

- `dap-admin` already depends on `bradian-relational`. This
  ADR adds explicit imports from
  `bradian_relational::bace::*` and
  `bradian_relational::row_store::BaceWiring` inside
  `dap-admin/src/local_sql.rs`.
- No new workspace deps.

## 8. What this ADR explicitly does NOT do

- Touch any on-disk format.
- Add policy knobs beyond profile + codec allow-set.
- Change existing CLI flag semantics or defaults.
- Add BACE support to other `bradian-db` subcommands.
- Introduce persistent BACE configuration files.
- Change the private-root permission or local-access
  gate.
- Add new error variants to any engine.
- Exercise G23 DDL coordinator flows.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0072 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions.
3. Implementation satisfying items 1, 2, 4, 5 in §3.5
   (item 3 was merged with item 2).
4. No regressions in `dap-admin` tests and
   `bradian-relational` tests.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.

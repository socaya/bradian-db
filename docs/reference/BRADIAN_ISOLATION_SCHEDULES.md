# Bradian isolation schedule corpus

In-tree companion to [`BRADIAN_ISOLATION_LEVELS.md`](BRADIAN_ISOLATION_LEVELS.md).
This document enumerates concrete multi-transaction schedules
drawn from Berenson et al., *"A Critique of ANSI SQL
Isolation Levels"* (1995), and records what each Bradian
engine actually does when the schedule is executed.

Every row is backed by a test in
[`crates/bradian-relational/tests/isolation_schedules.rs`](../../crates/bradian-relational/tests/isolation_schedules.rs)
that uses a `std::sync::Barrier` to force the interleaving.

The format is Berenson's: `w1[x]` means transaction 1 writes
x; `r2[x]` means transaction 2 reads x; `c1` commits tx 1;
`a1` aborts tx 1.

## 1. Schedules covered

| Shorthand | Schedule | ANSI name | Anomaly | Snapshot mode | Read-validation mode |
|-----------|----------|-----------|---------|---------------|-----------------------|
| **P0** | `w1[x] w2[x] c1 c2` | Dirty write | Lost-update variant | **Prevented** — second committer conflicts | **Prevented** |
| **P1** | `w1[x] r2[x] c1 c2` | Dirty read | Read uncommitted | **Prevented** — snapshot never sees uncommitted | **Prevented** |
| **P2** | `r1[x] w2[x] c2 r1[x] c1` | Non-repeatable read | Fuzzy read | **Prevented** — same snapshot | **Prevented** |
| **P3** | `r1[P] w2[Q] c2 r1[P] c1` | Phantom | Insert of row matching the predicate | **Prevented** within a snapshot | **Prevented** across bounded observation window |
| **P4** | `r1[x] w2[x] c2 w1[x] c1` | Lost update | Both tx overwrite x | **Prevented** — conflict check aborts whichever commits second | **Prevented** |
| **A5A** | `r1[x] r1[y] r2[y] w2[y] r2[x] w2[x] c2 c1` | Read skew | Observable inconsistency on a two-key invariant | **Possible** in snapshot mode | **Prevented** within observation window |
| **A5B** | `r1[x] r2[y] w1[y] w2[x] c1 c2` | Write skew | Two writers, each reads what the other writes | **Possible** in snapshot mode | **Prevented** within observation window |

## 2. How to read the matrix

- *Prevented* means the engine aborts one of the two
  transactions with an explicit error (`SqlError::Table(Resource(..))`
  or a conflict); the anomaly does NOT appear in the
  committed history.
- *Possible* means neither transaction fails; the resulting
  committed state exhibits the anomaly. Callers that need
  this prevented must enable read-validation mode where
  supported.
- The matrix reflects **current behaviour**, not a
  normative claim of serializability. Bradian does not
  claim serializable isolation even in read-validation
  mode — see [`BRADIAN_ISOLATION_LEVELS.md`](BRADIAN_ISOLATION_LEVELS.md).

## 3. Scope and limits

- **`DomainSqlEngine` and `MvccSqlEngine` serialize sessions
  through a per-engine operation mutex.** Two in-process
  threads cannot interleave concurrent sessions — the
  second `with_session` call blocks until the first
  returns. The in-tree corpus therefore uses sequential
  setups that mirror the Berenson schedule shape but
  assert the structural invariant (no silent overwrite,
  no cross-session visibility of uncommitted state,
  stable within-session snapshot, etc.). This is a
  conservative fit: the mutex PREVENTS each anomaly by
  making concurrency structurally impossible in-process.
- Multi-process / distributed schedules are OPEN — Bradian
  does not yet claim distributed isolation.
- `SqlEngine` holds a per-statement operation mutex, so
  P1 / P2 / P3 / P4 are structurally impossible for
  autocommit statements too.
- The read-validation mode (ADR 0037) is
  HistorySqlEngine-only; schedules marked "read-validation
  mode" cite the corresponding `read_validation_sql::*`
  test as their original owner — this corpus does not
  re-run them, just cross-references.

## 4. External reference-suite integration

External SQL-92 / 99 / 2016 conformance suites
(SQLLogicTest, pgTAP, DBMS-independent isolation tests)
are not yet integrated — tracked as
[ADR 0277](../adr/0271-0274-0276-0277-p0-3-deferred.md).
This corpus closes the **in-tree** reference portion of
G1-REFERENCE; the external-suite tail remains infra.

# Historical transaction admission

Fixed Master Gate 1 / G1-MEMORY now bounds active historical SQL transactions and
explicitly admitted native durable/typed transactions sharing one history owner.

| Policy | Default | Scope |
|---|---|---|
| TransactionLimits.max_active | 128 | Pin references already held plus the proposed new transaction |
| --max-active-transactions | 128 | Existing history-mode CLI owner; requires --history |

Multiple handles at the same snapshot LSN consume separate slots. Admission counts
all current pin references, including administrative handles, and is serialized with
pin creation. No slot is created when the limit refuses a BEGIN. Zero is valid and
refuses all SQL/fallible BEGINs; it does not disable owner-open validation or recovery.

## Configure

```sh
bradian-db sql --root ./private-history --history --max-active-transactions 8 --execute "BEGIN; SELECT id FROM demo.public.items; COMMIT"
```

A CLI process has one scoped SQL session and retains the existing exclusive store
lock. The shared-count policy is most relevant to concurrent embedded sessions on
one HistorySqlEngine, rather than cross-process SQL sharing.

Native TransactionLimits is in runtime_limits. DurableMvccStore,
DurableTypedStore and HistorySqlEngine expose open_with_resource_limits accepting
history and transaction policy (the typed form also accepts its catalog tables).
Existing open/open_with_history_limits or raw open_with_limits use defaults.
transaction_limits reports the selected policy. It is immutable per owner and not
persisted in WAL/checkpoints/archives; select it again after reopening/restoring.

Raw/typed try_begin, try_begin_with_read_validation and try_begin_with_limits perform
admission. The last accepts staging policy and the explicit read-validation choice.
Every historical SQL BEGIN uses this path, with no unbounded fallback. Ordinary
read-only SQL transactions also consume one slot until completed.

## Completion and refusal

Refusal returns an active-history-transaction resource error before pin creation,
with no staged work, WAL append or LSN change. It leaves other sessions/markers
intact. Callers can explicitly try again after another transaction ends; there is
no automatic wait queue or retry loop.

Commit, read-only completion, full rollback, drop and consumed commit errors release
their existing pin and therefore capacity. Scoped SQL callbacks also release pending
transactions on return/error/unwind. Statement errors without markers retain the
existing full-abort behavior. A failed transaction with a savepoint still owns its
slot; ROLLBACK TO recovers that transaction without freeing the slot. Only ending
the outer transaction releases it. Existing retention pins and conflicts remain in
force, including across savepoint rollback.

## Compatibility and remaining scope

The existing infallible native begin, begin_with_read_validation and begin_with_limits
remain trusted administrative compatibility APIs. They do not enforce this cap and
can exceed it. Their pins are included when a later SQL/fallible BEGIN checks capacity.
Use the try_begin APIs when admission is required. Owner validation, migrations and
administrative recovery retain their existing compatibility behavior.

The limit counts admitted handles per history owner. It is not a process-wide pool,
aggregate byte reservation, cross-domain limit or authorization control. It does not
cover all native callers, other engines, catalog/decoded state, allocator slack or
long-lived nontransactional scan/output buffers. Global memory admission and broader
runtime/failure qualification remain open. No cancellation, OOM, device-failure or
cross-platform guarantee is added.

See [ADR 0050](../adr/0050-historical-transaction-admission.md) and
[executed verification](../implementation/BRADIAN_GATE1_TRANSACTION_ADMISSION_VERIFICATION_2026_11_13.md).

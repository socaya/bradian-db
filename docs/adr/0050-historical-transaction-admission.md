# ADR 0050 — Historical transaction admission (G1-MEMORY)

**Status:** Accepted for historical SQL BEGIN and fallible native durable/typed BEGIN.
Gate 1 remains active.

Per-transaction staging limits do not bound how many transaction snapshots a domain
can retain. TransactionLimits defaults to 128 active handles per durable history
owner. Historical SQL BEGIN, including read-validation mode, now uses fallible native
admission before creating a pin or staged state.

Count every current pin reference, including multiple handles at one LSN and existing
administrative pins. Check count, addition overflow and proposed capacity under the
same state mutex that creates the new pin. A resource refusal changes no pin, staged
state, WAL or LSN. It does not fence the owner or automatically retry/queue work.

Reuse the existing pin lifetime: successful commit, read-only completion, rollback,
drop, consumed commit errors and scoped callback exit/unwind release admitted
capacity. Savepoints and failed transactions retaining a marker continue to hold
one slot until the outer transaction ends. ROLLBACK TO does not create/free a slot.
Original retention and read/write conflict checks remain unchanged.

Native try_begin, try_begin_with_read_validation and try_begin_with_limits return
resource errors. Durable, typed and SQL open_with_resource_limits accept history and
transaction policies; existing open APIs choose defaults. CLI --max-active-transactions
requires --history. Zero policy admits no SQL/fallible BEGIN, while valid owner open
and administrative recovery can still proceed. Policies are not persisted.

The established infallible native begin/begin_with_read_validation/begin_with_limits
APIs retain their trusted administrative compatibility contract and do not enforce
admission. Their pins count against subsequent fallible/SQL admission, but they can
exceed the configured cap themselves. SQL has no such fallback. This is not a
universal native/process memory limit or a security boundary against trusted library
callers. Other engines, aggregate bytes across domains and a process-wide pool remain
open. No dependency, persistent format, SQL grammar or isolation change is introduced.

# ADR 0033 — Atomic history archive receipts and statistics

**Status:** Accepted. Existing backup formats remain unchanged.

G24's history manifest must describe the copied G22 state, not source state read
again after the archive is copied. Concurrent SQL writes could previously advance
the version count between archive capture and the separate statistics call,
causing an otherwise valid archive to fail verification.

Capture the G10 receipt and G22 statistics under the same existing MVCC mutation
mutex. The RowStore backup runs while that mutex remains held. Return an owned
statistics snapshot with its receipt and use those exact fields in the G24
manifest. Source writers may continue after capture while CRC/restore verification
runs. Schema mutation remains exclusive through the mutable engine contract.

This changes no catalog, row, WAL or archive representation and introduces no
new mutation lock ordering: MVCC state precedes RowStore, as in the original
backup path. It is a blocking local capture, not a nonblocking or distributed
snapshot. Archive verification also enforces the G30/G36 catalog horizon.

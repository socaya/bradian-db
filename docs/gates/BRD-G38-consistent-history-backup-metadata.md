# BRD-G38 — Consistent history archive receipts and statistics

**Design:** [ADR 0033](../adr/0033-atomic-history-backup-statistics.md)

G24 previously captured archive bytes and then independently read live version
statistics for the manifest. A concurrent writer could advance the source during
CRC work, making a valid captured archive fail verification against a later count.

`DurableMvccStore::backup_with_stats` now holds the existing mutation mutex through
archive capture and statistics construction. G24 uses that owned receipt/statistics
pair; it no longer samples live counts afterward. Writers resume while the archive
is checksummed and independently restored for validation. Catalog changes remain
excluded by mutable DDL ownership, and restore verification checks the exact
G30/G36 catalog horizon before publishing a destination.

Two integration tests establish that archive statistics remain valid after the
source changes, and that three backups created alongside continuous epoch-tagged
SQL updates restore with matching receipts, version counts and historical SQL
results. The complete workspace also exercises the existing corruption and
occupied-destination rejection suites.

No persistent format changed. Capture is blocking, single-process and local;
nonblocking, distributed, encrypted and remote/PITR archives remain separate work.

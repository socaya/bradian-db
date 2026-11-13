# ADR 0030 — Opt-in live schema epoch row encoding

**Status:** Accepted for immutable-schema historical SQL.

The G32 BRVE codec becomes usable in G23 SQL only after an explicit, one-time
`enable_epoch_row_writes` call. The decision is a logical G22 key
`\0bradian-row-encoding-v1` with value `BRADIAN-EPOCH-ROWS-V1`, committed in the
same WAL as row versions. It does not touch the catalog WAL or schema pins.
Older readers reject this unrecognized namespace; there is no implicit upgrade.

The registry is reconstructed from the retained, validated original schema pins.
Each BRVE row must use that table's exact registration LSN. Reopen validates every
retained payload, including overwritten/deleted rows, against the registry and
rejects BRVE versions predating or lacking the format decision. Mixed BRRW/BRVE
rows decode through all live and historical SQL paths, with physical PK checks.

Transactions use the encoding decision visible at their own pinned snapshot.
A transaction begun before the transition can commit BRRW afterward. This is
intentional: mixed rows are supported and each decoder verifies its own format.
Transactions begun after the decision emit BRVE. No existing rows are rewritten.
The extra 29-byte envelope counts against the existing physical inline limit.

The G24 archive naturally includes the encoding decision and both row formats;
restore must run full G23 validation. Checkpoints/retention preserve the decision
as an immutable singleton key, just as original schema pins remain singleton keys.
Cancellation, deletion or rewriting of the encoding decision is corruption.

This step does not activate historical schema revisions. Coordinating catalog and
history WALs and mapping rows across multiple active schemas remain separate work.

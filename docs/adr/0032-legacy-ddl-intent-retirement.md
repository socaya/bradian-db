# ADR 0032 — Verified retirement of inactive DDL intents

**Status:** Accepted for a one-way, explicit local upgrade step.

G28–G31 metadata records inactive proposals, cancellations, catalog horizon fences
and readiness certificates. None authorizes schema activation. A root may explicitly
retire that entire inactive vocabulary before using G36. Retirement does not apply
any proposed schema, change rows or advance the catalog WAL.

A singleton G22 key `\0bradian-schema-legacy-retirement-v1` stores version 1, the exact
catalog horizon, count and CRC32 of the canonical ordered legacy (key, LSN, value)
records. Verify the existing catalog fences before writing it. On reopen, verify the
complete legacy history first, then the retirement digest, chronology and original
catalog binding. Every legacy record must predate retirement. Rewrites, deletion,
partial retirement and newly appended old-protocol records are errors.

After retirement, old intent mutation APIs refuse further writes. Prepared-proposal
status is empty; archival fence/readiness accessors remain available for inspection.
G36's first catalog horizon must equal the retirement horizon. Thereafter its chain
tracks the catalog; archived G30 fences retain their original meaning instead of
being silently rebound. The retirement record and archived metadata are retained in
checkpoints/backups. This is a retirement of inactive work, not a schema migration,
cryptographic attestation or automatic application of previously proposed changes.

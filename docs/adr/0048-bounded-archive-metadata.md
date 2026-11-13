# ADR 0048 — Bounded archive metadata I/O (G1-MEMORY)

**Status:** Accepted for checkpoint manifests, native backup receipts and historical/
lifecycle archive JSON manifests. Gate 1 remains active.

Whole-file metadata reads could allocate based on an arbitrarily large file before
validating its format. Read BRCP checkpoint manifests and BRBKv001 backup receipts
into their exact 40-byte arrays, then probe one trailing byte. Short/trailing data
remains format corruption; missing checkpoint manifests remain optional, and actual
I/O errors retain their classification. Never trust a metadata length observation
instead of bounding actual reads.

MetadataLimits defaults to 1 MiB of serialized JSON manifest bytes for both historical
and lifecycle archives. Read at most the policy plus a one-byte overflow probe before
JSON parsing, component hashing or restore staging. Handle interrupted/short reads.
Reject oversized input as a resource error without modifying its source.

Apply the same policy while serializing manifests, before creating their manifest
file. Serialize borrowed reports rather than cloning their table lists. Stream the
canonical payload into CRC calculation without a second payload vector, then build
only a bounded envelope buffer. Preserve canonical field order and checksum bytes.
The serializer's counter uses checked arithmetic and refuses before extending beyond
policy. Resource errors remain distinct from malformed JSON and I/O errors.

Existing backup/restore entry points use defaults. Native historical and lifecycle
backup APIs accept explicit metadata policy. Historical restore composes explicit
history and metadata limits; lifecycle restore accepts metadata limits. No policy is
persisted. A backup that cannot serialize its manifest is never published, although
component checkpointing/staging can already have occurred under the prior contract.
A read refusal precedes destination staging. Retrying write refusal uses a fresh
destination or explicit incomplete-stage cleanup, as before.

This bounds serialized metadata input/output, not report/catalog objects, decoded
JSON structures, allocator slack, archive payloads, global memory or runtime work.
Directory enumeration and other metadata/catalog state remain separate. No file
format, dependency, isolation or device/platform guarantee changes.

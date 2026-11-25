# ADR 0335 — SAN-based cert principal mapping

## Status

Accepted. Sibling of ADR 0332's `extract_common_name` /
`principal_from_peer_chain` for operators who encode
principal in Subject-Alt-Name rather than in the Subject
DN's CommonName.

## Context

Modern PKIX guidance is to carry identity in SAN, not in
CN — Chrome and many other clients deprecated CN-based
identity years ago. Operators issuing client certs for
Bradian may follow the modern convention; a SAN-based
mapper is the sibling of ADR 0332 they need.

## Decision

Add to `bradian_security::cert_principal`:

```rust
pub fn extract_san_dns_names(cert_der: &[u8])
    -> CertPrincipalResult<Vec<String>>;

pub fn principal_from_peer_chain_san(
    cert_chain: &[impl AsRef<[u8]>],
    mapper:     impl FnOnce(&[String]) -> Option<Principal>,
) -> CertPrincipalResult<Principal>;

pub enum CertPrincipalError {
    ..., MissingSan, ...
}
```

Semantics:

- `extract_san_dns_names` walks the SubjectAlternativeName
  X.509 extension on the parsed cert and returns every
  `GeneralName::DNSName` entry as `String`. Returns
  `Ok(vec![])` when the extension is absent — this is
  different from parse failure (`ParseError`).
- `principal_from_peer_chain_san(chain, mapper)` passes
  the **full** SAN DNS list to the mapper so the mapper
  picks whichever entry it prefers (first-match,
  allowlist, substring-match, etc.). An empty SAN list
  is `MissingSan`; a non-`None` mapper result wins; a
  `None` result is `MapperRefused` with all candidate
  names joined by `,` so the operator sees what was
  presented.

## Scope

**In scope:**
- Real SAN DNS extraction via `x509-parser`'s
  `SubjectAlternativeName` + `GeneralName::DNSName`
  variants.
- `MissingSan` error for certs without SAN entries.
- 5 new tests (14 total in `cert_principal`): SAN
  extraction, mapper first-match on a specific suffix,
  mapper refusal surfaces all candidate names,
  empty-list behaviour, empty-chain refusal, `MissingSan`
  Display wiring.

**Out of scope** (dedicated follow-ups):
- SAN IP / URI / rfc822Name variants — most
  Bradian-style deployments identify principals by
  DNS name. Supporting IP SAN is a trivial extension.
- An operator helper that picks the first SAN matching
  a regex / allowlist (right now the mapper does it
  manually).
- A `WireServer::serve_one_mtls_san` entry that uses
  SAN internally — the generic `serve_one_mtls{_authz}`
  is already flexible enough; a caller just supplies a
  SAN-based mapper.

## Consequences

- Operators with modern PKIX conventions (identity in
  SAN) can map peer certs to principals without manually
  parsing X.509 themselves.
- `CertPrincipalError` grows one variant (`MissingSan`).
- The cert-principal module is stable across both
  mapping conventions.

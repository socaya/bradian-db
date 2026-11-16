# ADR 0107 — Penetration-review scope document (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a procurement artifact.**

## 1. Honest scope

Shipped in-repo: `docs/security/BRADIAN_PEN_REVIEW_SCOPE.md`
— a scope-of-work document the owner can hand to an external
auditor. It enumerates:
- 8 in-scope components with file / module pointers.
- 7 threat-model excerpts the review must validate.
- 5 out-of-scope items the auditor must NOT grade as "fail"
  (documented not-shipped per CLAUDE.md §2).
- 4 required deliverables from the auditor.
- Procurement notes.

## 2. Out of scope

The **actual review** requires:
- Owner procurement of an external auditor.
- Scheduling + NDA / contract.
- 2–4 weeks of senior auditor time.
- Signed deliverables.

None of these can happen in-repo.

## 3. Compliance

1. Scope document is complete at the component-level.
2. Threat-model excerpts match
   `docs/security/BRADIAN_THREAT_MODEL.md`.
3. Out-of-scope boundaries explicitly map to the CLAUDE.md
   §2 honest-status items so no future auditor confusion.
4. Document is ready to send once procurement happens.

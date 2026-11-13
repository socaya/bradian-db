# ADR 0035 — Immutable constant defaults in schema epochs

**Status:** Accepted for exclusive ADD COLUMN with typed literal defaults.

G40 adds an optional `column_defaults` map to a table definition. Empty maps are
omitted from serialization and deserialize as empty, preserving the exact canonical
bytes of existing G11/G23 schema pins. Defaults use the existing typed Value codec,
refer to known columns and must satisfy their declared type, length and nullability.
Existing defaults cannot change through this increment.

G40 permits trailing columns with immutable constant defaults, including NOT NULL
columns with a non-NULL default. Old rows are decoded against their original epoch
and projected using the value bound when each new column was introduced. No old
payload is rewritten. INSERT omissions and explicit DEFAULT use the current schema's
same value; explicit NULL remains distinct and is rejected for NOT NULL columns.
NULL defaults are valid only for nullable columns.

The G36 history intent/activation uses version 2 whenever the defaults map changes.
The G11 journal uses a distinct ProposeDefaultedRevision command, while G33/G39 APIs
retain their earlier contracts. Version-1 intents cannot introduce defaults, and
activation versions must match their intent. Thus old readers reject the new event
instead of ignoring an unknown Table field and incorrectly projecting NULL. History
key, WAL, page and BRVE row layouts remain unchanged.

The shared projection validator preserves existing column names/types/order,
constraints and previously bound defaults. It permits earlier nullable relaxation
and defaulted additions when projecting across several epochs. The default-changing
DDL itself may only append columns: SET/DROP DEFAULT on existing columns, expressions,
SET NOT NULL on existing data, renames, removals and type changes remain unsupported.

All new metadata is validated and capacity-checked before the first intent write.
The existing four-boundary roll-forward, failed-owner fence, backup verification and
retention rules apply. Nullable/required NULL projections are revalidated against the
target schema. The base DESCRIBE result stays compatible; WITH DEFAULTS explicitly
adds has_default and column_default fields for historical inspection.

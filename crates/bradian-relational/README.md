# Bradian Relational Catalog

A first, independent foundation for Bradian's general-purpose SQL catalog. Models databases, schemas, tables, columns and constraint definitions with validated creation and dependency-safe deletion. Separate from DHIS2 metadata.

```sh
cargo test -p bradian-relational
```

**Important:** This crate is intentionally **in-memory** and is not a transaction manager, SQL parser, persistent row store, or runtime PK/FK constraint engine. It does not claim ACID or SQL compliance. See `docs/adr/0006-generic-relational-catalog.md` and the master SRS v1.1.

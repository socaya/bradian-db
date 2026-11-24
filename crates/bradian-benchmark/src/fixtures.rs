//! Synthetic DHIS2 aggregate fact generator.
//!
//! Produces a reproducible set of rows across a chosen number of org units, periods and data
//! elements. Deterministic for a given seed.

use bradian_core::{Column, LogicalType, Schema, Value};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

pub struct FactsFixture {
    pub schema: Schema,
    pub rows: Vec<Vec<Value>>,
}

pub fn generate_schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("coc", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap()
}

pub fn generate_facts(
    seed: u64,
    org_units: usize,
    periods: usize,
    data_elements: usize,
    rows: usize,
) -> FactsFixture {
    let schema = generate_schema();
    let mut rng = ChaCha12Rng::seed_from_u64(seed);
    let ous: Vec<String> = (0..org_units).map(|i| format!("ou{:0>9}", i)).collect();
    let pers: Vec<String> = (0..periods)
        .map(|i| {
            let year = 2026 - (i / 12) as i32;
            let month = (i % 12 + 1) as u32;
            format!("{:04}{:02}", year, month)
        })
        .collect();
    let des: Vec<String> = (0..data_elements).map(|i| format!("de{:0>9}", i)).collect();

    let mut out = Vec::with_capacity(rows);
    for _ in 0..rows {
        let ou = ous[rng.gen_range(0..ous.len())].clone();
        let per = pers[rng.gen_range(0..pers.len())].clone();
        let de = des[rng.gen_range(0..des.len())].clone();
        let value = if rng.gen_bool(0.05) {
            Value::Null
        } else {
            Value::F64(rng.gen_range(0.0..1000.0))
        };
        out.push(vec![
            Value::Str(ou),
            Value::Str(per),
            Value::Str(de),
            Value::Str("cocDEFAULT0".into()),
            value,
        ]);
    }
    FactsFixture { schema, rows: out }
}

/// ADR 0306 — typed workload descriptor with named
/// presets. Bench runs refer to presets by name so
/// comparison results stay apples-to-apples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkloadSpec {
    pub seed:           u64,
    pub org_units:      usize,
    pub periods:        usize,
    pub data_elements:  usize,
    pub rows:           usize,
}

impl WorkloadSpec {
    pub fn small() -> Self {
        Self { seed: 1, org_units: 50, periods: 24, data_elements: 200, rows: 100_000 }
    }
    pub fn medium() -> Self {
        Self { seed: 2, org_units: 500, periods: 36, data_elements: 1_000, rows: 2_000_000 }
    }
    pub fn dhis2_like() -> Self {
        Self { seed: 7, org_units: 5_000, periods: 24, data_elements: 2_000, rows: 10_000_000 }
    }
}

/// ADR 0306 — convenience wrapper.
pub fn generate_from_spec(spec: &WorkloadSpec) -> FactsFixture {
    generate_facts(spec.seed, spec.org_units, spec.periods, spec.data_elements, spec.rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determinism_across_two_calls() {
        let a = generate_from_spec(&WorkloadSpec::small());
        let b = generate_from_spec(&WorkloadSpec::small());
        assert_eq!(a.rows.len(), b.rows.len());
        assert_eq!(a.rows, b.rows,
            "reproducibility: identical spec must yield identical rows");
    }

    #[test]
    fn presets_are_distinct_fingerprints() {
        let small     = WorkloadSpec::small();
        let medium    = WorkloadSpec::medium();
        let dhis2like = WorkloadSpec::dhis2_like();
        assert_eq!(small.rows,     100_000);
        assert_eq!(medium.rows,    2_000_000);
        assert_eq!(dhis2like.rows, 10_000_000);
        // Different seeds yield different row sets even at
        // identical dimensions. Only check row 0 — this is
        // a fingerprint spot-check, not a bulk compare.
        let a = generate_facts(small.seed, 1, 1, 1, 8);
        let b = generate_facts(small.seed + 1, 1, 1, 1, 8);
        assert_ne!(a.rows[0][4], b.rows[0][4],
            "seed advancement should change the random draw");
    }
}

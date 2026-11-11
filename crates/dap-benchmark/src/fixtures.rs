//! Synthetic DHIS2 aggregate fact generator.
//!
//! Produces a reproducible set of rows across a chosen number of org units, periods and data
//! elements. Deterministic for a given seed.

use dap_core::{Column, LogicalType, Schema, Value};
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

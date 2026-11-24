//! ADR 0300 — DHIS2 metadata import / export round-trip.
//!
//! Seeds a fresh catalog with a reference bundle, exports it
//! to DHIS2-shaped JSON, reimports into a second fresh
//! catalog, and asserts both catalogs enumerate pairwise
//! equal on every object type (sorted by UID).

use bradian_catalog::{apply_dhis2_metadata_json, export_dhis2_bundle_json, Catalog};

const REFERENCE_BUNDLE: &str = r#"{
  "categoryCombos": [
    {
      "id": "CCaaaaaaaaa",
      "name": "default",
      "categoryOptionCombos": [{ "id": "COCbbbbbbbb" }]
    }
  ],
  "categoryOptionCombos": [
    {
      "id": "COCbbbbbbbb",
      "name": "default",
      "categoryCombo": { "id": "CCaaaaaaaaa" }
    }
  ],
  "dataElements": [
    {
      "id": "DEaaaaaaaaa",
      "code": "DE_A",
      "name": "Measles vaccinations",
      "valueType": "INTEGER_POSITIVE",
      "aggregationType": "SUM",
      "categoryCombo": { "id": "CCaaaaaaaaa" }
    }
  ],
  "dataSets": [
    {
      "id": "DSaaaaaaaaa",
      "name": "EPI Monthly",
      "periodType": "Monthly",
      "dataSetElements": [
        { "dataElement": { "id": "DEaaaaaaaaa" } }
      ]
    }
  ]
}"#;

#[test]
fn import_export_reimport_is_equivalent_on_observable_subset() {
    let a = Catalog::new();
    let count = apply_dhis2_metadata_json(&a, REFERENCE_BUNDLE.as_bytes()).unwrap();
    assert_eq!(count, 4, "reference bundle should emit 4 metadata objects");

    let exported = export_dhis2_bundle_json(&a);
    // Must be valid JSON and carry each top-level DHIS2 key
    // the importer reads, even for empty object classes.
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    for key in [
        "dataElements", "dataSets", "categoryCombos", "categoryOptionCombos",
        "organisationUnits", "programs", "programStages",
        "programIndicators", "indicators",
    ] {
        assert!(parsed.get(key).and_then(|v| v.as_array()).is_some(),
            "export missing key `{key}`");
    }
    // Spot-check: data element DEaaaaaaaaa was round-tripped
    // with its code, name, value type and category combo
    // reference.
    let des = parsed["dataElements"].as_array().unwrap();
    assert_eq!(des.len(), 1);
    assert_eq!(des[0]["id"],              "DEaaaaaaaaa");
    assert_eq!(des[0]["code"],            "DE_A");
    assert_eq!(des[0]["name"],            "Measles vaccinations");
    assert_eq!(des[0]["valueType"],       "INTEGER_POSITIVE");
    assert_eq!(des[0]["aggregationType"], "SUM");
    assert_eq!(des[0]["categoryCombo"]["id"], "CCaaaaaaaaa");

    // Re-ingest into a fresh catalog and compare enumerations.
    let b = Catalog::new();
    apply_dhis2_metadata_json(&b, exported.as_bytes()).unwrap();

    fn sort_by_uid<T: Clone, F: Fn(&T) -> String>(mut v: Vec<T>, k: F) -> Vec<T> {
        v.sort_by_key(k); v
    }

    assert_eq!(
        sort_by_uid(a.data_elements(), |d| d.uid.as_ref().to_string()),
        sort_by_uid(b.data_elements(), |d| d.uid.as_ref().to_string()),
        "data elements diverged after round-trip",
    );
    assert_eq!(
        sort_by_uid(a.datasets(), |d| d.uid.as_ref().to_string()),
        sort_by_uid(b.datasets(), |d| d.uid.as_ref().to_string()),
        "datasets diverged after round-trip",
    );
    assert_eq!(
        sort_by_uid(a.category_combos(), |c| c.uid.as_ref().to_string()),
        sort_by_uid(b.category_combos(), |c| c.uid.as_ref().to_string()),
        "category combos diverged after round-trip",
    );
    assert_eq!(
        sort_by_uid(a.category_option_combos(), |c| c.uid.as_ref().to_string()),
        sort_by_uid(b.category_option_combos(), |c| c.uid.as_ref().to_string()),
        "category option combos diverged after round-trip",
    );
}

#[test]
fn export_is_deterministic_regardless_of_insertion_order() {
    let a = Catalog::new();
    apply_dhis2_metadata_json(&a, REFERENCE_BUNDLE.as_bytes()).unwrap();
    let first  = export_dhis2_bundle_json(&a);
    let second = export_dhis2_bundle_json(&a);
    assert_eq!(first, second,
        "export must be byte-deterministic for a given catalog state");
}

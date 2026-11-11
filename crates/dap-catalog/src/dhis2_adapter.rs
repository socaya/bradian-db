//! DHIS2 metadata adapter.
//!
//! Loads a DHIS2 `/api/metadata` JSON bundle into the in-memory [`Catalog`]. Only the
//! fields the analytical engine reads are pulled through; unknown top-level keys are
//! ignored.
//!
//! The adapter is transport-agnostic: callers hand it a `serde_json::Value` (typically
//! loaded from disk or fetched over HTTP by the admin CLI). A live HTTP client lands with
//! the admin app; keeping the adapter pure JSON means tests can exercise it with fixtures
//! committed to the repo.

use dap_core::{DapError, DapResult, PeriodType, Uid};
use serde::Deserialize;

use crate::metadata::{
    AggregationType, CategoryCombo, CategoryOptionCombo, DataElement, DataValueType, Dataset,
    Indicator, MetadataObject, OrgUnit, Program, ProgramIndicator, ProgramStage, ProgramType,
};
use crate::registry::Catalog;

/// Shape of the metadata document the DHIS2 API returns. Only the subset DAP reads is
/// modelled. DHIS2's extra fields parse as `serde(default)` and are discarded.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Dhis2MetadataBundle {
    #[serde(default)]
    pub data_elements: Vec<Dhis2DataElement>,
    #[serde(default)]
    pub data_sets: Vec<Dhis2Dataset>,
    #[serde(default)]
    pub category_combos: Vec<Dhis2CategoryCombo>,
    #[serde(default)]
    pub category_option_combos: Vec<Dhis2CategoryOptionCombo>,
    #[serde(default)]
    pub organisation_units: Vec<Dhis2OrgUnit>,
    #[serde(default)]
    pub programs: Vec<Dhis2Program>,
    #[serde(default)]
    pub program_stages: Vec<Dhis2ProgramStage>,
    #[serde(default)]
    pub program_indicators: Vec<Dhis2ProgramIndicator>,
    #[serde(default)]
    pub indicators: Vec<Dhis2Indicator>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2DataElement {
    pub id: Uid,
    #[serde(default)]
    pub code: Option<String>,
    pub name: String,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub aggregation_type: Option<String>,
    pub category_combo: Dhis2Ref,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2Dataset {
    pub id: Uid,
    pub name: String,
    pub period_type: String,
    #[serde(default)]
    pub data_set_elements: Vec<Dhis2DataSetElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2DataSetElement {
    pub data_element: Dhis2Ref,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2CategoryCombo {
    pub id: Uid,
    pub name: String,
    #[serde(default)]
    pub category_option_combos: Vec<Dhis2Ref>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2CategoryOptionCombo {
    pub id: Uid,
    pub name: String,
    pub category_combo: Dhis2Ref,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2OrgUnit {
    pub id: Uid,
    #[serde(default)]
    pub code: Option<String>,
    pub name: String,
    #[serde(default)]
    pub parent: Option<Dhis2Ref>,
    #[serde(default)]
    pub level: u16,
    /// DHIS2 ships the path as a slash-separated ancestor list (`/root/mid/leaf`).
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2Program {
    pub id: Uid,
    pub name: String,
    pub program_type: String,
    #[serde(default)]
    pub program_stages: Vec<Dhis2Ref>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2ProgramStage {
    pub id: Uid,
    pub name: String,
    pub program: Dhis2Ref,
    #[serde(default)]
    pub repeatable: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2ProgramIndicator {
    pub id: Uid,
    pub name: String,
    pub program: Dhis2Ref,
    pub expression: String,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub aggregation_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2Indicator {
    pub id: Uid,
    pub name: String,
    pub numerator: String,
    pub denominator: String,
    #[serde(default = "default_factor")]
    pub factor: i64,
}

fn default_factor() -> i64 {
    1
}

/// Reference to another DHIS2 object (`{ "id": "deAAAAAAAAA" }`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dhis2Ref {
    pub id: Uid,
}

/// Parse a DHIS2 metadata bundle from JSON bytes and apply it to the catalog.
pub fn apply_dhis2_metadata_json(catalog: &Catalog, json: &[u8]) -> DapResult<usize> {
    let bundle: Dhis2MetadataBundle = serde_json::from_slice(json)?;
    apply_dhis2_bundle(catalog, bundle)
}

pub fn apply_dhis2_bundle(catalog: &Catalog, bundle: Dhis2MetadataBundle) -> DapResult<usize> {
    let mut objects: Vec<MetadataObject> = Vec::new();

    for cc in bundle.category_combos {
        objects.push(MetadataObject::CategoryCombo(CategoryCombo {
            uid: cc.id,
            name: cc.name,
            option_combos: cc
                .category_option_combos
                .into_iter()
                .map(|r| r.id)
                .collect(),
        }));
    }

    for coc in bundle.category_option_combos {
        objects.push(MetadataObject::CategoryOptionCombo(CategoryOptionCombo {
            uid: coc.id,
            name: coc.name,
            category_combo: coc.category_combo.id,
        }));
    }

    for de in bundle.data_elements {
        objects.push(MetadataObject::DataElement(DataElement {
            uid: de.id,
            code: de.code,
            name: de.name,
            value_type: parse_value_type(de.value_type.as_deref())?,
            aggregation_type: parse_aggregation_type(de.aggregation_type.as_deref())?,
            category_combo: de.category_combo.id,
        }));
    }

    for ds in bundle.data_sets {
        objects.push(MetadataObject::Dataset(Dataset {
            uid: ds.id,
            name: ds.name,
            period_type: parse_period_type(&ds.period_type)?,
            data_elements: ds
                .data_set_elements
                .into_iter()
                .map(|dse| dse.data_element.id)
                .collect(),
        }));
    }

    for ou in bundle.organisation_units {
        let path = ou
            .path
            .as_deref()
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| Uid::new(s.to_string()))
            .collect::<DapResult<Vec<_>>>()
            .unwrap_or_else(|_| vec![ou.id.clone()]);
        objects.push(MetadataObject::OrgUnit(OrgUnit {
            uid: ou.id,
            code: ou.code,
            name: ou.name,
            parent: ou.parent.map(|r| r.id),
            level: ou.level,
            path,
        }));
    }

    for ps in bundle.program_stages {
        objects.push(MetadataObject::ProgramStage(ProgramStage {
            uid: ps.id,
            name: ps.name,
            program: ps.program.id,
            repeatable: ps.repeatable,
        }));
    }

    for p in bundle.programs {
        objects.push(MetadataObject::Program(Program {
            uid: p.id,
            name: p.name,
            program_type: parse_program_type(&p.program_type)?,
            stages: p.program_stages.into_iter().map(|r| r.id).collect(),
        }));
    }

    for pi in bundle.program_indicators {
        objects.push(MetadataObject::ProgramIndicator(ProgramIndicator {
            uid: pi.id,
            name: pi.name,
            program: pi.program.id,
            expression: pi.expression,
            filter: pi.filter,
            aggregation_type: parse_aggregation_type(pi.aggregation_type.as_deref())?,
        }));
    }

    for i in bundle.indicators {
        objects.push(MetadataObject::Indicator(Indicator {
            uid: i.id,
            name: i.name,
            numerator: i.numerator,
            denominator: i.denominator,
            factor: i.factor,
        }));
    }

    let count = objects.len();
    catalog.apply(objects)?;
    Ok(count)
}

fn parse_value_type(raw: Option<&str>) -> DapResult<DataValueType> {
    Ok(match raw.unwrap_or("NUMBER") {
        "NUMBER" => DataValueType::Number,
        "INTEGER" => DataValueType::Integer,
        "INTEGER_POSITIVE" | "POSITIVE_INTEGER" => DataValueType::PositiveInteger,
        "INTEGER_NEGATIVE" | "NEGATIVE_INTEGER" => DataValueType::NegativeInteger,
        "INTEGER_ZERO_OR_POSITIVE" | "ZERO_OR_POSITIVE_INTEGER" => {
            DataValueType::ZeroOrPositiveInteger
        }
        "PERCENTAGE" => DataValueType::Percentage,
        "BOOLEAN" | "TRUE_ONLY" => DataValueType::Boolean,
        "TEXT" => DataValueType::Text,
        "LONG_TEXT" => DataValueType::LongText,
        "DATE" => DataValueType::Date,
        "DATETIME" => DataValueType::DateTime,
        "TIME" => DataValueType::Time,
        other => {
            return Err(DapError::UnsupportedFeature(format!(
                "DHIS2 valueType '{other}' is not supported; add it to the adapter when needed"
            )))
        }
    })
}

fn parse_aggregation_type(raw: Option<&str>) -> DapResult<AggregationType> {
    Ok(match raw.unwrap_or("SUM") {
        "SUM" | "DEFAULT" => AggregationType::Sum,
        "AVERAGE" | "AVERAGE_SUM_ORG_UNIT_HIERARCHY" => AggregationType::Average,
        "AVERAGE_SUM_ORG_UNIT" => AggregationType::AverageSumOrgUnit,
        "COUNT" => AggregationType::Count,
        "MIN" => AggregationType::Min,
        "MAX" => AggregationType::Max,
        "STDDEV" => AggregationType::StdDev,
        "VARIANCE" => AggregationType::Variance,
        "LAST" => AggregationType::Last,
        "LAST_IN_PERIOD" => AggregationType::LastInPeriod,
        "NONE" => AggregationType::None,
        "CUSTOM" => AggregationType::Custom,
        other => {
            return Err(DapError::UnsupportedFeature(format!(
                "DHIS2 aggregationType '{other}' is not supported"
            )))
        }
    })
}

fn parse_period_type(raw: &str) -> DapResult<PeriodType> {
    Ok(match raw {
        "Daily" => PeriodType::Daily,
        "Weekly" => PeriodType::Weekly,
        "Monthly" => PeriodType::Monthly,
        "Quarterly" => PeriodType::Quarterly,
        "Yearly" => PeriodType::Yearly,
        other => {
            return Err(DapError::UnsupportedFeature(format!(
                "DHIS2 periodType '{other}' is not supported in v1 (Weekly variants, \
                 fiscal years and relative periods are deferred)"
            )))
        }
    })
}

fn parse_program_type(raw: &str) -> DapResult<ProgramType> {
    Ok(match raw {
        "WITH_REGISTRATION" => ProgramType::WithRegistration,
        "WITHOUT_REGISTRATION" => ProgramType::WithoutRegistration,
        other => {
            return Err(DapError::UnsupportedFeature(format!(
                "DHIS2 programType '{other}' is not supported"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_small_bundle() {
        let json = r##"{
            "dataElements": [{
                "id": "deANC000001",
                "name": "ANC 1st visits",
                "valueType": "INTEGER",
                "aggregationType": "SUM",
                "categoryCombo": {"id": "ccDEFAULTT1"}
            }],
            "dataSets": [{
                "id": "dsANC000000",
                "name": "ANC monthly",
                "periodType": "Monthly",
                "dataSetElements": [
                    {"dataElement": {"id": "deANC000001"}}
                ]
            }],
            "categoryCombos": [{
                "id": "ccDEFAULTT1",
                "name": "default",
                "categoryOptionCombos": [{"id": "cocDEFAULT0"}]
            }],
            "categoryOptionCombos": [{
                "id": "cocDEFAULT0",
                "name": "default",
                "categoryCombo": {"id": "ccDEFAULTT1"}
            }],
            "organisationUnits": [{
                "id": "ouROOT00000",
                "name": "root",
                "level": 1,
                "path": "/ouROOT00000"
            }, {
                "id": "ouAAAAAAAAA",
                "name": "A",
                "parent": {"id": "ouROOT00000"},
                "level": 2,
                "path": "/ouROOT00000/ouAAAAAAAAA"
            }],
            "programs": [{
                "id": "prANC000000",
                "name": "ANC",
                "programType": "WITH_REGISTRATION",
                "programStages": [{"id": "ps000000001"}]
            }],
            "programStages": [{
                "id": "ps000000001",
                "name": "Visit",
                "program": {"id": "prANC000000"},
                "repeatable": true
            }],
            "programIndicators": [{
                "id": "piANCVISITS",
                "name": "ANC visits",
                "program": {"id": "prANC000000"},
                "expression": "V{event_count}",
                "aggregationType": "SUM"
            }],
            "indicators": [{
                "id": "indANCCOVER",
                "name": "ANC coverage",
                "numerator": "#{deANC000001}",
                "denominator": "#{deANC000001}",
                "factor": 100
            }]
        }"##;
        let catalog = Catalog::new();
        let count = apply_dhis2_metadata_json(&catalog, json.as_bytes()).unwrap();
        assert!(count >= 8, "parsed {count} objects");

        let de = catalog.data_element(&Uid::new("deANC000001").unwrap()).unwrap();
        assert_eq!(de.aggregation_type, AggregationType::Sum);
        assert_eq!(de.value_type, DataValueType::Integer);

        let desc = catalog.org_unit_subtree(&Uid::new("ouROOT00000").unwrap());
        assert_eq!(desc.len(), 2);

        let pi = catalog
            .program_indicator(&Uid::new("piANCVISITS").unwrap())
            .unwrap();
        assert_eq!(pi.expression, "V{event_count}");

        let ind = catalog.indicator(&Uid::new("indANCCOVER").unwrap()).unwrap();
        assert_eq!(ind.factor, 100);
    }

    #[test]
    fn unknown_value_type_rejected() {
        let json = r##"{
            "dataElements": [{
                "id": "deBADTYPEX1",
                "name": "bad",
                "valueType": "CODED",
                "categoryCombo": {"id": "ccDEFAULTT1"}
            }]
        }"##;
        let catalog = Catalog::new();
        let err = apply_dhis2_metadata_json(&catalog, json.as_bytes()).expect_err("rejected");
        assert!(err.to_string().contains("valueType"));
    }
}

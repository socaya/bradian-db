//! Live DHIS2 HTTP client.
//!
//! Fetches `/api/metadata` from a running DHIS2 instance and applies the response to a
//! [`Catalog`]. The client:
//!
//! - Uses basic authentication (DHIS2's standard mode) or an opaque bearer token.
//! - Accepts the field subset DAP cares about via the DHIS2 `fields=` filter so we do not
//!   waste bandwidth on the ~hundred-plus fields DHIS2 can return.
//! - Returns the number of objects applied plus the new catalog revision.
//!
//! The client is gated behind the `http` feature so pure-offline builds (for example the
//! `DER Client` embedded edge binary) need not pull reqwest + rustls into the audit
//! surface.

use dap_core::{DapError, DapResult};

use crate::dhis2_adapter::apply_dhis2_metadata_json;
use crate::registry::Catalog;

#[derive(Clone, Debug)]
pub struct Dhis2HttpClient {
    base_url: String,
    auth: Dhis2Auth,
    client: reqwest::Client,
    timeout: std::time::Duration,
}

#[derive(Clone, Debug)]
pub enum Dhis2Auth {
    Basic { username: String, password: String },
    Bearer(String),
}

/// Default field selector hitting every DHIS2 object the DAP adapter reads. Keep in sync
/// with `dap-catalog::dhis2_adapter` — the adapter rejects on unknown value types, so
/// mismatches will surface as loud errors rather than silent data loss.
const DEFAULT_FIELDS: &str = concat!(
    "dataElements[id,code,name,valueType,aggregationType,categoryCombo[id]],",
    "dataSets[id,name,periodType,dataSetElements[dataElement[id]]],",
    "categoryCombos[id,name,categoryOptionCombos[id]],",
    "categoryOptionCombos[id,name,categoryCombo[id]],",
    "organisationUnits[id,code,name,parent[id],level,path],",
    "programs[id,name,programType,programStages[id]],",
    "programStages[id,name,program[id],repeatable],",
    "programIndicators[id,name,program[id],expression,filter,aggregationType],",
    "indicators[id,name,numerator,denominator,factor]"
);

impl Dhis2HttpClient {
    pub fn new(base_url: impl Into<String>, auth: Dhis2Auth) -> DapResult<Self> {
        let client = reqwest::Client::builder()
            .gzip(true)
            .user_agent(concat!("dap-admin/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DapError::other(format!("reqwest build: {e}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth,
            client,
            timeout: std::time::Duration::from_secs(60),
        })
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Fetch `/api/metadata` with the DAP default field selector and apply it to `catalog`.
    pub async fn load_metadata(&self, catalog: &Catalog) -> DapResult<Dhis2LoadReport> {
        self.load_metadata_with_fields(catalog, DEFAULT_FIELDS).await
    }

    pub async fn load_metadata_with_fields(
        &self,
        catalog: &Catalog,
        fields: &str,
    ) -> DapResult<Dhis2LoadReport> {
        let url = format!(
            "{}/api/metadata?fields={}&paging=false",
            self.base_url,
            url::form_urlencoded::byte_serialize(fields.as_bytes()).collect::<String>()
        );
        let mut req = self.client.get(&url).timeout(self.timeout);
        match &self.auth {
            Dhis2Auth::Basic { username, password } => {
                req = req.basic_auth(username, Some(password));
            }
            Dhis2Auth::Bearer(token) => {
                req = req.bearer_auth(token);
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DapError::other(format!("GET {url} failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(DapError::other(format!(
                "DHIS2 returned {} for {url}",
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| DapError::other(format!("read body: {e}")))?;
        let applied = apply_dhis2_metadata_json(catalog, &bytes)?;
        Ok(Dhis2LoadReport {
            applied_objects: applied,
            catalog_revision: catalog.revision().to_string(),
            body_bytes: bytes.len() as u64,
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Dhis2LoadReport {
    pub applied_objects: usize,
    pub catalog_revision: String,
    pub body_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_constructs_with_basic_auth() {
        let client = Dhis2HttpClient::new(
            "https://play.dhis2.org/dev",
            Dhis2Auth::Basic {
                username: "admin".into(),
                password: "district".into(),
            },
        )
        .unwrap();
        assert_eq!(client.base_url, "https://play.dhis2.org/dev");
    }

    #[test]
    fn default_fields_lists_every_known_object_kind() {
        for expected in [
            "dataElements",
            "dataSets",
            "categoryCombos",
            "categoryOptionCombos",
            "organisationUnits",
            "programs",
            "programStages",
            "programIndicators",
            "indicators",
        ] {
            assert!(
                DEFAULT_FIELDS.contains(expected),
                "field selector missing {expected}"
            );
        }
    }
}

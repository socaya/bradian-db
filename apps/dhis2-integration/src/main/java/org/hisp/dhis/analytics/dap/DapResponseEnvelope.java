// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;
import java.util.Map;

/**
 * Parsed analytics response from DAP. Mirrors
 * {@code dap_serving::dhis2_compat::Dhis2AnalyticsResponse}. DHIS2's analytics service
 * lifts {@link #rows} and {@link #headers} into its own {@code Grid}; the extra
 * {@link #provenance} block is the DAP-specific freshness stamp DHIS2 uses to surface
 * stale / partial / exact badges.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public final class DapResponseEnvelope
        implements org.hisp.dhis.analytics.AnalyticsManager.DapResult {

    public List<Header> headers;
    public List<List<Object>> rows;
    @JsonProperty("metaData")
    public MetaData metaData;
    public int width;
    public int height;
    @JsonProperty("rowContext")
    public Map<String, Object> rowContext;
    public Provenance provenance;

    @JsonIgnoreProperties(ignoreUnknown = true)
    public static final class Header {
        public String name;
        public String column;
        public String valueType;
        public String type;
        public boolean hidden;
        public boolean meta;
    }

    @JsonIgnoreProperties(ignoreUnknown = true)
    public static final class MetaData {
        public Map<String, Object> items;
        public Map<String, Object> dimensions;
    }

    @JsonIgnoreProperties(ignoreUnknown = true)
    public static final class Provenance {
        @JsonProperty("data_revision")
        public long dataRevision;
        @JsonProperty("metadata_revision")
        public String metadataRevision;
        @JsonProperty("watermark_ms")
        public long watermarkMs;
        @JsonProperty("is_exact")
        public boolean isExact;
        public String engine;
    }
}

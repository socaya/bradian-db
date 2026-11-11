// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;
import java.util.Map;

/**
 * Parsed response from {@code /api/analytics.json}. The DHIS2 shape is preserved
 * verbatim; {@link #provenance} is the DAP-specific extension that lets DHIS2 render
 * staleness / partial / exact badges.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public final class DapAnalyticsResponse {

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

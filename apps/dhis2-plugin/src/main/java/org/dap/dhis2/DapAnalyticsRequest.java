// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * Request envelope posted to {@code /api/analytics.json}. Mirrors the Rust
 * {@code Dhis2AnalyticsRequest} struct in {@code dap-serving::dhis2_compat}.
 */
public final class DapAnalyticsRequest {

    public final String dataset;
    public final String program;
    public final String indicator;
    public final String stage;
    public final String partition;
    public final List<String> dx;
    public final List<String> pe;
    public final List<String> ou;
    public final List<String> aggregation;
    public final List<Filter> filter;
    public final Integer limit;

    private DapAnalyticsRequest(Builder b) {
        this.dataset = b.dataset;
        this.program = b.program;
        this.indicator = b.indicator;
        this.stage = b.stage;
        this.partition = Objects.requireNonNullElse(b.partition, "p1");
        this.dx = Collections.unmodifiableList(b.dx);
        this.pe = Collections.unmodifiableList(b.pe);
        this.ou = Collections.unmodifiableList(b.ou);
        this.aggregation = Collections.unmodifiableList(b.aggregation);
        this.filter = Collections.unmodifiableList(b.filter);
        this.limit = b.limit;
    }

    public static Builder newBuilder() {
        return new Builder();
    }

    public static final class Filter {
        public final String column;
        public final String op;
        public final Object value;

        public Filter(String column, String op, Object value) {
            this.column = Objects.requireNonNull(column, "column");
            this.op = Objects.requireNonNull(op, "op");
            this.value = value;
        }
    }

    public static final class Builder {
        private String dataset;
        private String program;
        private String indicator;
        private String stage;
        private String partition;
        private final List<String> dx = new ArrayList<>();
        private final List<String> pe = new ArrayList<>();
        private final List<String> ou = new ArrayList<>();
        private final List<String> aggregation = new ArrayList<>();
        private final List<Filter> filter = new ArrayList<>();
        private Integer limit;

        public Builder dataset(String v)   { this.dataset = v; return this; }
        public Builder program(String v)   { this.program = v; return this; }
        public Builder indicator(String v) { this.indicator = v; return this; }
        public Builder stage(String v)     { this.stage = v; return this; }
        public Builder partition(String v) { this.partition = v; return this; }
        public Builder dx(List<String> v)  { this.dx.addAll(v); return this; }
        public Builder pe(List<String> v)  { this.pe.addAll(v); return this; }
        public Builder ou(List<String> v)  { this.ou.addAll(v); return this; }
        public Builder aggregation(List<String> v) { this.aggregation.addAll(v); return this; }
        public Builder filter(Filter f)    { this.filter.add(f); return this; }
        public Builder limit(Integer v)    { this.limit = v; return this; }

        public DapAnalyticsRequest build() {
            if (dataset == null && program == null) {
                throw new IllegalStateException("dataset or program must be set");
            }
            return new DapAnalyticsRequest(this);
        }
    }
}

// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import java.util.List;

/**
 * Thin façade DHIS2 interacts with. Mirrors the subset of
 * {@code org.hisp.dhis.analytics.AnalyticsDatabase} the plug-in needs; the real DHIS2
 * interface carries more methods, but a shaded wrapper in DHIS2's own module forwards
 * each call to the matching method here.
 */
public interface AnalyticsDatabase {

    /**
     * Aggregate analytics: data values, data elements, periods, org units.
     */
    AnalyticsResult executeAggregate(DapAnalyticsRequest request) throws AnalyticsException;

    /**
     * Event analytics: event grain program indicator.
     */
    AnalyticsResult executeEvent(DapAnalyticsRequest request) throws AnalyticsException;

    /**
     * Enrollment analytics: enrollment grain program indicator (tracker).
     */
    AnalyticsResult executeEnrollment(DapAnalyticsRequest request) throws AnalyticsException;

    /**
     * Result grid returned to DHIS2. Column names match the request's group-by + measure
     * columns. {@link #provenance()} carries DAP's freshness stamp.
     */
    final class AnalyticsResult {
        private final List<String> columns;
        private final List<List<Object>> rows;
        private final DapAnalyticsResponse.Provenance provenance;

        public AnalyticsResult(List<String> columns, List<List<Object>> rows,
                                DapAnalyticsResponse.Provenance provenance) {
            this.columns = columns;
            this.rows = rows;
            this.provenance = provenance;
        }

        public List<String> columns() { return columns; }
        public List<List<Object>> rows() { return rows; }
        public DapAnalyticsResponse.Provenance provenance() { return provenance; }
    }

    final class AnalyticsException extends Exception {
        public AnalyticsException(String message) { super(message); }
        public AnalyticsException(String message, Throwable cause) { super(message, cause); }
    }
}

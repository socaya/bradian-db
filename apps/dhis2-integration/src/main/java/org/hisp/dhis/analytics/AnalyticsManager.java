// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics;

/**
 * Minimal copy of DHIS2's {@code org.hisp.dhis.analytics.AnalyticsManager} interface.
 * The real DHIS2 API publishes many more methods — the fields we implement cover the
 * two paths DHIS2 v43 exercises for Doris:
 *
 * <ol>
 *   <li>{@link #executeAggregate(DapRequest)} — called by DHIS2 for aggregate
 *       analytics requests.</li>
 *   <li>{@link #executeEvent(DapRequest)} and {@link #executeEnrollment(DapRequest)} —
 *       called for the event and tracker analytics paths.</li>
 * </ol>
 *
 * <p>Our implementation of this interface for DAP lives alongside — see
 * {@link org.hisp.dhis.analytics.dap.DapAnalyticsManager}. When the upstream PR lands,
 * DHIS2's authoritative interface replaces this stand-in.
 */
public interface AnalyticsManager {

    DapResult executeAggregate(DapRequest request) throws AnalyticsException;

    DapResult executeEvent(DapRequest request) throws AnalyticsException;

    DapResult executeEnrollment(DapRequest request) throws AnalyticsException;

    /** Local shim so this skeleton compiles. DHIS2's real interface uses its own types. */
    interface DapRequest {}

    /** Local shim so this skeleton compiles. DHIS2's real interface uses its own types. */
    interface DapResult {}

    class AnalyticsException extends Exception {
        public AnalyticsException(String message) { super(message); }
        public AnalyticsException(String message, Throwable cause) { super(message, cause); }
    }
}

// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics;

/**
 * Minimal copy of DHIS2's {@code org.hisp.dhis.analytics.AnalyticsTableManager}. DHIS2
 * calls this interface during the analytics-table rebuild (nightly in most
 * deployments). For PostgreSQL the DHIS2 implementation runs SQL DDL + {@code INSERT
 * INTO ... SELECT}; for Doris it issues bulk loads over the MySQL wire protocol; for
 * DAP we batch canonical changes and POST them to {@code /api/analytics/v1/ingest}.
 *
 * <p>Our DAP-specific implementation is {@link
 * org.hisp.dhis.analytics.dap.DapAnalyticsTableManager}.
 */
public interface AnalyticsTableManager {

    /** Prepare the analytics backend for a fresh rebuild (partition reset + manifest). */
    void prepareRebuild() throws AnalyticsManager.AnalyticsException;

    /** Push one batch of canonical changes to the analytics backend. */
    void ingestBatch(DapIngest batch) throws AnalyticsManager.AnalyticsException;

    /** Flip the analytics backend to the newly-ingested data atomically. */
    void finaliseRebuild() throws AnalyticsManager.AnalyticsException;

    /** Shim for the batch shape DHIS2's rebuild path hands this interface. */
    interface DapIngest {}
}

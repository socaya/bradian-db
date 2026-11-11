// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import org.hisp.dhis.analytics.AnalyticsManager;

import java.io.IOException;

/**
 * Executes analytics queries against the DAP engine. DHIS2 calls this on the critical
 * path for every analytics request; one instance per DHIS2 server. The real DHIS2 class
 * wires this against {@link org.hisp.dhis.analytics.AnalyticsManager}; here we keep a
 * thin local interface so the sources compile outside the DHIS2 build.
 */
public final class DapAnalyticsManager implements AnalyticsManager {

    private final DapHttpClient client;

    public DapAnalyticsManager(DapConfiguration config) {
        this(new DapHttpClient(config));
    }

    public DapAnalyticsManager(DapHttpClient client) {
        this.client = client;
    }

    public DapResponseEnvelope aggregate(DapRequestEnvelope req) throws AnalyticsException {
        return call(() -> client.postAnalytics("/api/analytics.json", req));
    }

    public DapResponseEnvelope event(DapRequestEnvelope req) throws AnalyticsException {
        return call(() -> client.postAnalytics("/api/analytics/events/query.json", req));
    }

    public DapResponseEnvelope enrollment(DapRequestEnvelope req) throws AnalyticsException {
        return call(() -> client.postAnalytics("/api/analytics/enrollments/query.json", req));
    }

    @Override public DapResult executeAggregate(DapRequest request) throws AnalyticsException {
        return (DapResult) aggregate((DapRequestEnvelope) request);
    }
    @Override public DapResult executeEvent(DapRequest request) throws AnalyticsException {
        return (DapResult) event((DapRequestEnvelope) request);
    }
    @Override public DapResult executeEnrollment(DapRequest request) throws AnalyticsException {
        return (DapResult) enrollment((DapRequestEnvelope) request);
    }

    private interface Call {
        DapResponseEnvelope run() throws IOException, InterruptedException;
    }

    private DapResponseEnvelope call(Call call) throws AnalyticsException {
        try {
            return call.run();
        } catch (IOException e) {
            throw new AnalyticsException("DAP request failed: " + e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new AnalyticsException("DAP request interrupted", e);
        }
    }
}

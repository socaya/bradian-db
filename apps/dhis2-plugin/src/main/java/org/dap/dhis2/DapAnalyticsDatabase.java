// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Properties;
import java.util.stream.Collectors;

/**
 * The service-provider implementation DHIS2 picks up via
 * {@link java.util.ServiceLoader}. Builds a {@link DapAnalyticsClient} at startup and
 * forwards every analytics call to the DAP HTTP engine.
 */
public final class DapAnalyticsDatabase
        implements AnalyticsDatabase, AnalyticsDatabaseProvider {

    private final DapAnalyticsClient client;

    /** No-arg constructor required by {@link java.util.ServiceLoader}. */
    public DapAnalyticsDatabase() {
        this.client = null;
    }

    private DapAnalyticsDatabase(DapAnalyticsClient client) {
        this.client = client;
    }

    @Override
    public String name() {
        return "dap";
    }

    @Override
    public AnalyticsDatabase create(Properties dhisConf) {
        return new DapAnalyticsDatabase(new DapAnalyticsClient(
                Dhis2PluginConfig.fromProperties(dhisConf)));
    }

    @Override
    public AnalyticsResult executeAggregate(DapAnalyticsRequest request)
            throws AnalyticsException {
        return liftResponse(call(() -> requireClient().aggregateAnalytics(request)));
    }

    @Override
    public AnalyticsResult executeEvent(DapAnalyticsRequest request)
            throws AnalyticsException {
        return liftResponse(call(() -> requireClient().eventAnalytics(request)));
    }

    @Override
    public AnalyticsResult executeEnrollment(DapAnalyticsRequest request)
            throws AnalyticsException {
        return liftResponse(call(() -> requireClient().enrollmentAnalytics(request)));
    }

    private DapAnalyticsClient requireClient() throws AnalyticsException {
        if (client == null) {
            throw new AnalyticsException(
                    "DapAnalyticsDatabase was obtained via the service-loader default "
                    + "constructor — call create(dhisConf) before executing requests.");
        }
        return client;
    }

    private interface Call {
        DapAnalyticsResponse run()
                throws IOException, InterruptedException, AnalyticsException;
    }

    private DapAnalyticsResponse call(Call call) throws AnalyticsException {
        try {
            return call.run();
        } catch (IOException e) {
            throw new AnalyticsException("DAP request failed: " + e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new AnalyticsException("DAP request interrupted", e);
        } catch (AnalyticsException e) {
            throw e;
        }
    }

    private AnalyticsResult liftResponse(DapAnalyticsResponse r) {
        List<String> columns = r.headers == null
                ? List.of()
                : r.headers.stream().map(h -> h.name).collect(Collectors.toList());
        List<List<Object>> rows = r.rows == null ? new ArrayList<>() : r.rows;
        return new AnalyticsResult(columns, rows, r.provenance);
    }
}

// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import org.hisp.dhis.analytics.AnalyticsDatabase;

import java.util.Properties;

/**
 * Single entry point DHIS2's {@code AnalyticsManagerFactory} + {@code
 * AnalyticsTableManagerFactory} dispatch to when {@code analytics.database = dap}.
 *
 * <p>Mirrors the shape of DHIS2 v43's internal {@code DorisAnalyticsBackend}: given a
 * {@link Properties} bag resolved from {@code dhis.conf}, returns configured query and
 * table-manager instances.
 */
public final class DapAnalyticsBackend {

    public static final AnalyticsDatabase KEY = AnalyticsDatabase.DAP;

    private final DapConfiguration config;
    private final DapHttpClient client;

    public DapAnalyticsBackend(Properties dhisConf) {
        this.config = DapConfiguration.fromProperties(dhisConf);
        this.client = new DapHttpClient(config);
    }

    public DapConfiguration config() { return config; }

    public DapAnalyticsManager analyticsManager() {
        return new DapAnalyticsManager(client);
    }

    public DapAnalyticsTableManager tableManager(String dataset, String partition) {
        return new DapAnalyticsTableManager(client, dataset, partition);
    }
}

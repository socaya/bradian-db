// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Properties;
import java.util.ServiceLoader;

import static org.junit.jupiter.api.Assertions.*;

class DapAnalyticsDatabaseTest {

    @Test
    void serviceLoaderPicksUpTheProvider() {
        ServiceLoader<AnalyticsDatabaseProvider> loader =
                ServiceLoader.load(AnalyticsDatabaseProvider.class);
        List<AnalyticsDatabaseProvider> providers = loader.stream()
                .map(ServiceLoader.Provider::get)
                .toList();
        assertTrue(providers.stream().anyMatch(p -> "dap".equals(p.name())),
                "DAP provider must be registered under META-INF/services");
    }

    @Test
    void uncreatedInstanceRefusesToExecute() {
        DapAnalyticsDatabase provider = new DapAnalyticsDatabase();
        DapAnalyticsRequest req = DapAnalyticsRequest.newBuilder()
                .dataset("dsAAAAAAAAA")
                .partition("p1")
                .build();
        AnalyticsDatabase.AnalyticsException e = assertThrows(
                AnalyticsDatabase.AnalyticsException.class,
                () -> provider.executeAggregate(req));
        assertTrue(e.getMessage().contains("create(dhisConf)"));
    }

    @Test
    void createBuildsAConfiguredClient() {
        DapAnalyticsDatabase provider = new DapAnalyticsDatabase();
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "basic");
        p.setProperty("analytics.database.auth.username", "u");
        p.setProperty("analytics.database.auth.password", "s");
        AnalyticsDatabase db = provider.create(p);
        assertNotNull(db);
        // We cannot execute without a live engine; the create() pathway succeeding is
        // the invariant we care about here.
    }
}

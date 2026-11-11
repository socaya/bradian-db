// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockserver.integration.ClientAndServer;

import java.util.Map;
import java.util.Properties;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockserver.integration.ClientAndServer.startClientAndServer;
import static org.mockserver.model.HttpRequest.request;
import static org.mockserver.model.HttpResponse.response;

class DapAnalyticsTableManagerTest {

    private ClientAndServer server;
    private DapConfiguration config;
    private AtomicInteger seen;

    @BeforeEach
    void start() {
        seen = new AtomicInteger();
        server = startClientAndServer(0);
        server.when(request().withMethod("POST").withPath("/api/analytics/v1/ingest"))
              .respond(request -> {
                  seen.incrementAndGet();
                  return response().withStatusCode(204);
              });
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url",
                "http://localhost:" + server.getLocalPort());
        p.setProperty("analytics.database.auth", "none");
        p.setProperty("analytics.database.retry.max_attempts", "1");
        config = DapConfiguration.fromProperties(p);
    }

    @AfterEach
    void stop() { if (server != null) server.stop(); }

    @Test
    void ingestsBatchesAndTracksCount() throws Exception {
        DapHttpClient client = new DapHttpClient(config);
        DapAnalyticsTableManager tm = new DapAnalyticsTableManager(client, "dsAAAAAAAAA", "p1");
        tm.prepareRebuild();
        for (int i = 0; i < 3; i++) {
            tm.ingestBatch(new DapIngestBatch.Builder("dsAAAAAAAAA", "p1")
                    .upsert("c-" + i, "deAAAAAAAAA", 1 + i, "2026-01-01T00:00:00Z",
                            Map.of(
                                    "ou", "ouAAAAAAAAA",
                                    "period", "202601",
                                    "de", "deAAAAAAAAA",
                                    "value", 42.0 + i))
                    .build());
        }
        tm.finaliseRebuild();
        assertEquals(3, tm.batchesSent());
        assertEquals(3, seen.get());
    }

    @Test
    void finaliseRebuildDoesNotHitNetwork() throws Exception {
        DapHttpClient client = new DapHttpClient(config);
        DapAnalyticsTableManager tm = new DapAnalyticsTableManager(client, "d", "p1");
        tm.prepareRebuild();
        tm.finaliseRebuild();
        assertEquals(0, seen.get());
    }
}

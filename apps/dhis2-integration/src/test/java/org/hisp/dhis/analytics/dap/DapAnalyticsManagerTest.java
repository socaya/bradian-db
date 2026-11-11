// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockserver.integration.ClientAndServer;

import java.net.http.HttpClient;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockserver.integration.ClientAndServer.startClientAndServer;
import static org.mockserver.model.HttpRequest.request;
import static org.mockserver.model.HttpResponse.response;

class DapAnalyticsManagerTest {

    private ClientAndServer server;
    private DapConfiguration config;
    private final ObjectMapper mapper = new ObjectMapper();

    @BeforeEach
    void start() {
        server = startClientAndServer(0);
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url",
                "http://localhost:" + server.getLocalPort());
        p.setProperty("analytics.database.auth", "basic");
        p.setProperty("analytics.database.auth.username", "u");
        p.setProperty("analytics.database.auth.password", "s");
        p.setProperty("analytics.database.retry.max_attempts", "1");
        config = DapConfiguration.fromProperties(p);
    }

    @AfterEach
    void stop() { if (server != null) server.stop(); }

    @Test
    void aggregateHappyPath() throws Exception {
        Map<String, Object> body = Map.of(
                "headers", List.of(
                        Map.of("name", "ou", "column", "ou", "valueType", "TEXT",
                               "type", "java.lang.String", "hidden", false, "meta", false),
                        Map.of("name", "sum_value", "column", "sum_value", "valueType", "NUMBER",
                               "type", "java.lang.String", "hidden", false, "meta", false)
                ),
                "rows", List.of(List.of("ouAAAAAAAAA", 42.0)),
                "metaData", Map.of("items", Map.of(), "dimensions", Map.of()),
                "width", 2,
                "height", 1,
                "rowContext", Map.of(),
                "provenance", Map.of(
                        "data_revision", 3,
                        "metadata_revision", "abc",
                        "watermark_ms", 1700000000000L,
                        "is_exact", true,
                        "engine", "dap"
                )
        );
        server.when(request().withMethod("POST").withPath("/api/analytics.json"))
              .respond(response()
                      .withStatusCode(200)
                      .withContentType(org.mockserver.model.MediaType.APPLICATION_JSON)
                      .withBody(mapper.writeValueAsBytes(body)));

        DapHttpClient client = new DapHttpClient(config,
                HttpClient.newBuilder().connectTimeout(Duration.ofMillis(500)).build());
        DapAnalyticsManager mgr = new DapAnalyticsManager(client);
        DapResponseEnvelope resp = mgr.aggregate(DapRequestEnvelope.newBuilder()
                .dataset("dsAAAAAAAAA").partition("p1").build());
        assertEquals(2, resp.headers.size());
        assertEquals(1, resp.height);
        assertTrue(resp.provenance.isExact);
    }

    @Test
    void strictProvenanceRejectsInexact() throws Exception {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url",
                "http://localhost:" + server.getLocalPort());
        p.setProperty("analytics.database.auth", "none");
        p.setProperty("analytics.database.provenance", "strict");
        p.setProperty("analytics.database.retry.max_attempts", "1");
        DapConfiguration strict = DapConfiguration.fromProperties(p);
        Map<String, Object> body = Map.of(
                "headers", List.of(),
                "rows", List.of(),
                "metaData", Map.of("items", Map.of(), "dimensions", Map.of()),
                "width", 0,
                "height", 0,
                "rowContext", Map.of(),
                "provenance", Map.of(
                        "data_revision", 1,
                        "metadata_revision", "x",
                        "watermark_ms", 0,
                        "is_exact", false,
                        "engine", "dap"
                )
        );
        server.when(request().withMethod("POST").withPath("/api/analytics.json"))
              .respond(response()
                      .withStatusCode(200)
                      .withContentType(org.mockserver.model.MediaType.APPLICATION_JSON)
                      .withBody(mapper.writeValueAsBytes(body)));

        DapAnalyticsManager mgr = new DapAnalyticsManager(strict);
        assertThrows(java.io.IOException.class,
                () -> new DapHttpClient(strict).postAnalytics(
                        "/api/analytics.json",
                        DapRequestEnvelope.newBuilder().dataset("d").build()));
        assertNotNull(mgr);
    }
}

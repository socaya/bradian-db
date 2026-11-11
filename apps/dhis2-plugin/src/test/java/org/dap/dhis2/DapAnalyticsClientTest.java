// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockserver.integration.ClientAndServer;

import java.net.URI;
import java.net.http.HttpClient;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockserver.integration.ClientAndServer.startClientAndServer;
import static org.mockserver.model.HttpRequest.request;
import static org.mockserver.model.HttpResponse.response;

class DapAnalyticsClientTest {

    private ClientAndServer server;
    private Dhis2PluginConfig config;
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
        p.setProperty("analytics.database.read_timeout_ms", "5000");
        config = Dhis2PluginConfig.fromProperties(p);
    }

    @AfterEach
    void stop() {
        if (server != null) server.stop();
    }

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

        DapAnalyticsClient client = new DapAnalyticsClient(config,
                HttpClient.newBuilder().connectTimeout(Duration.ofMillis(500)).build());
        DapAnalyticsRequest req = DapAnalyticsRequest.newBuilder()
                .dataset("dsAAAAAAAAA")
                .partition("p1")
                .pe(List.of("202601"))
                .build();
        DapAnalyticsResponse resp = client.aggregateAnalytics(req);
        assertEquals(2, resp.headers.size());
        assertEquals(1, resp.height);
        assertEquals("dap", resp.provenance.engine);
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
        Dhis2PluginConfig strict = Dhis2PluginConfig.fromProperties(p);

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
        server.reset();
        server.when(request().withMethod("POST").withPath("/api/analytics.json"))
              .respond(response()
                      .withStatusCode(200)
                      .withContentType(org.mockserver.model.MediaType.APPLICATION_JSON)
                      .withBody(mapper.writeValueAsBytes(body)));

        DapAnalyticsClient client = new DapAnalyticsClient(strict,
                HttpClient.newBuilder().connectTimeout(Duration.ofMillis(500)).build());
        DapAnalyticsRequest req = DapAnalyticsRequest.newBuilder().dataset("d").build();
        assertThrows(java.io.IOException.class, () -> client.aggregateAnalytics(req));
    }

    @Test
    void propagatesServerFailure() {
        server.reset();
        server.when(request().withMethod("POST").withPath("/api/analytics.json"))
              .respond(response().withStatusCode(502).withBody("upstream down"));

        DapAnalyticsClient client = new DapAnalyticsClient(config);
        DapAnalyticsRequest req = DapAnalyticsRequest.newBuilder().dataset("d").build();
        java.io.IOException e =
                assertThrows(java.io.IOException.class, () -> client.aggregateAnalytics(req));
        assertTrue(e.getMessage().contains("502"));
    }

    @Test
    void baseUrlRelativeResolution() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "http://host.example/dap/");
        p.setProperty("analytics.database.auth", "none");
        Dhis2PluginConfig cfg = Dhis2PluginConfig.fromProperties(p);
        URI resolved = cfg.baseUrl().resolve("/api/analytics.json");
        assertEquals("http://host.example/api/analytics.json", resolved.toString());
    }
}

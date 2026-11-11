// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.util.Base64;

/**
 * Thin HTTP client the plug-in uses to talk to a DAP engine. Synchronous — DHIS2's
 * analytics-service runs on a worker thread pool. One client instance per DHIS2 server;
 * HttpClient is internally thread-safe.
 */
public final class DapAnalyticsClient {

    private final Dhis2PluginConfig config;
    private final HttpClient http;
    private final ObjectMapper mapper;

    public DapAnalyticsClient(Dhis2PluginConfig config) {
        this(config, HttpClient.newBuilder()
                .connectTimeout(config.connectTimeout())
                .build());
    }

    DapAnalyticsClient(Dhis2PluginConfig config, HttpClient http) {
        this.config = config;
        this.http = http;
        this.mapper = new ObjectMapper()
                .setSerializationInclusion(JsonInclude.Include.NON_NULL)
                .disable(SerializationFeature.WRITE_DATES_AS_TIMESTAMPS);
    }

    public DapAnalyticsResponse aggregateAnalytics(DapAnalyticsRequest request)
            throws IOException, InterruptedException {
        return post("/api/analytics.json", request);
    }

    public DapAnalyticsResponse eventAnalytics(DapAnalyticsRequest request)
            throws IOException, InterruptedException {
        return post("/api/analytics/events/query.json", request);
    }

    public DapAnalyticsResponse enrollmentAnalytics(DapAnalyticsRequest request)
            throws IOException, InterruptedException {
        return post("/api/analytics/enrollments/query.json", request);
    }

    private DapAnalyticsResponse post(String path, DapAnalyticsRequest request)
            throws IOException, InterruptedException {
        byte[] body = mapper.writeValueAsBytes(request);
        URI uri = config.baseUrl().resolve(path);
        HttpRequest.Builder b = HttpRequest.newBuilder(uri)
                .timeout(config.readTimeout())
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .POST(HttpRequest.BodyPublishers.ofByteArray(body));
        applyAuth(b);

        IOException last = null;
        for (int attempt = 0; attempt < Math.max(1, config.retryMaxAttempts()); attempt++) {
            try {
                HttpResponse<byte[]> resp = http.send(b.build(),
                        HttpResponse.BodyHandlers.ofByteArray());
                int status = resp.statusCode();
                if (status >= 200 && status < 300) {
                    DapAnalyticsResponse parsed = mapper.readValue(resp.body(),
                            DapAnalyticsResponse.class);
                    enforceProvenance(parsed);
                    return parsed;
                }
                String msg = "DAP responded " + status + ": " + new String(resp.body(),
                        StandardCharsets.UTF_8);
                if (status >= 500) {
                    last = new IOException(msg);
                    // Retry 5xx; fall through.
                } else {
                    throw new IOException(msg);
                }
            } catch (IOException ioe) {
                last = ioe;
            }
        }
        throw last == null ? new IOException("DAP unreachable") : last;
    }

    private void applyAuth(HttpRequest.Builder b) {
        switch (config.authMode()) {
            case BASIC -> {
                String creds = config.username() + ":" + config.password();
                String enc = Base64.getEncoder().encodeToString(
                        creds.getBytes(StandardCharsets.UTF_8));
                b.header("authorization", "Basic " + enc);
            }
            case BEARER -> b.header("authorization", "Bearer " + config.bearerToken());
            case NONE -> { /* no header */ }
        }
    }

    private void enforceProvenance(DapAnalyticsResponse response) throws IOException {
        if (response.provenance == null) return;
        boolean exact = response.provenance.isExact;
        switch (config.provenance()) {
            case STRICT -> {
                if (!exact) {
                    throw new IOException("DAP response is not exact; strict provenance mode");
                }
            }
            case WARN, LENIENT -> { /* DHIS2 handler decides */ }
        }
    }
}

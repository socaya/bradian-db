// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

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
 * Low-level HTTP client wrapping {@link java.net.http.HttpClient}. Owned by {@link
 * DapAnalyticsManager} and {@link DapAnalyticsTableManager}; one instance per DHIS2
 * server.
 */
public final class DapHttpClient {

    private final DapConfiguration config;
    private final HttpClient http;
    private final ObjectMapper mapper;

    public DapHttpClient(DapConfiguration config) {
        this(config, HttpClient.newBuilder()
                .connectTimeout(config.connectTimeout())
                .build());
    }

    DapHttpClient(DapConfiguration config, HttpClient http) {
        this.config = config;
        this.http = http;
        this.mapper = new ObjectMapper()
                .setSerializationInclusion(JsonInclude.Include.NON_NULL)
                .disable(SerializationFeature.WRITE_DATES_AS_TIMESTAMPS);
    }

    public DapConfiguration config() { return config; }

    public DapResponseEnvelope postAnalytics(String path, DapRequestEnvelope req)
            throws IOException, InterruptedException {
        return postJson(path, req, DapResponseEnvelope.class);
    }

    public void postIngest(DapIngestBatch batch) throws IOException, InterruptedException {
        postJson("/api/analytics/v1/ingest", batch, Void.class);
    }

    private <T> T postJson(String path, Object body, Class<T> type)
            throws IOException, InterruptedException {
        byte[] bytes = mapper.writeValueAsBytes(body);
        URI uri = config.baseUrl().resolve(path);
        HttpRequest.Builder b = HttpRequest.newBuilder(uri)
                .timeout(config.readTimeout())
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .POST(HttpRequest.BodyPublishers.ofByteArray(bytes));
        applyAuth(b);

        IOException last = null;
        for (int attempt = 0; attempt < Math.max(1, config.retryMaxAttempts()); attempt++) {
            try {
                HttpResponse<byte[]> resp = http.send(b.build(),
                        HttpResponse.BodyHandlers.ofByteArray());
                int status = resp.statusCode();
                if (status >= 200 && status < 300) {
                    if (type == Void.class || resp.body().length == 0) {
                        return null;
                    }
                    T parsed = mapper.readValue(resp.body(), type);
                    enforceProvenance(parsed);
                    return parsed;
                }
                String msg = "DAP responded " + status + ": " + new String(resp.body(),
                        StandardCharsets.UTF_8);
                if (status >= 500) {
                    last = new IOException(msg);
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

    private <T> void enforceProvenance(T parsed) throws IOException {
        if (!(parsed instanceof DapResponseEnvelope resp)) return;
        if (resp.provenance == null) return;
        if (config.provenance() == DapConfiguration.ProvenanceMode.STRICT
                && !resp.provenance.isExact) {
            throw new IOException("DAP response is not exact; strict provenance mode");
        }
    }
}

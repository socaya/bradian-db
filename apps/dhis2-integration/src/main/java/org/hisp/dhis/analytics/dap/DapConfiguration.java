// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import java.net.URI;
import java.time.Duration;
import java.util.Objects;
import java.util.Properties;

/**
 * Typed read of the {@code analytics.database.*} properties DHIS2's
 * {@code DhisConfigurationProvider} resolves from {@code dhis.conf}.
 *
 * <p>Field-for-field mirror of the {@code DorisConfiguration} class DHIS2 ships in v43;
 * every operator-facing property has a direct parallel so moving a deployment from
 * Doris to DAP is a one-line change (and back).
 */
public final class DapConfiguration {

    public enum AuthMode { BASIC, BEARER, NONE }
    public enum ProvenanceMode { STRICT, WARN, LENIENT }

    private final URI baseUrl;
    private final AuthMode authMode;
    private final String username;
    private final String password;
    private final String bearerToken;
    private final Duration connectTimeout;
    private final Duration readTimeout;
    private final int retryMaxAttempts;
    private final int ingestBatchRows;
    private final int ingestMaxConcurrency;
    private final String partitionPrefix;
    private final ProvenanceMode provenance;

    public DapConfiguration(URI baseUrl, AuthMode authMode, String username, String password,
                            String bearerToken, Duration connectTimeout, Duration readTimeout,
                            int retryMaxAttempts, int ingestBatchRows, int ingestMaxConcurrency,
                            String partitionPrefix, ProvenanceMode provenance) {
        this.baseUrl = Objects.requireNonNull(baseUrl, "baseUrl");
        this.authMode = Objects.requireNonNull(authMode, "authMode");
        this.username = username;
        this.password = password;
        this.bearerToken = bearerToken;
        this.connectTimeout = Objects.requireNonNull(connectTimeout, "connectTimeout");
        this.readTimeout = Objects.requireNonNull(readTimeout, "readTimeout");
        this.retryMaxAttempts = retryMaxAttempts;
        this.ingestBatchRows = ingestBatchRows;
        this.ingestMaxConcurrency = ingestMaxConcurrency;
        this.partitionPrefix = partitionPrefix == null ? "p" : partitionPrefix;
        this.provenance = Objects.requireNonNull(provenance, "provenance");
    }

    public URI baseUrl() { return baseUrl; }
    public AuthMode authMode() { return authMode; }
    public String username() { return username; }
    public String password() { return password; }
    public String bearerToken() { return bearerToken; }
    public Duration connectTimeout() { return connectTimeout; }
    public Duration readTimeout() { return readTimeout; }
    public int retryMaxAttempts() { return retryMaxAttempts; }
    public int ingestBatchRows() { return ingestBatchRows; }
    public int ingestMaxConcurrency() { return ingestMaxConcurrency; }
    public String partitionPrefix() { return partitionPrefix; }
    public ProvenanceMode provenance() { return provenance; }

    public static DapConfiguration fromProperties(Properties p) {
        String baseUrl = requireProp(p, "analytics.database.base_url");
        AuthMode auth;
        try {
            auth = AuthMode.valueOf(p.getProperty("analytics.database.auth", "basic")
                    .toUpperCase());
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException(
                    "analytics.database.auth must be one of basic / bearer / none", e);
        }
        String username = p.getProperty("analytics.database.auth.username");
        String password = p.getProperty("analytics.database.auth.password");
        String bearer   = p.getProperty("analytics.database.auth.bearer_token");
        if (auth == AuthMode.BASIC && (username == null || password == null)) {
            throw new IllegalArgumentException(
                    "analytics.database.auth = basic requires auth.username and auth.password");
        }
        if (auth == AuthMode.BEARER && (bearer == null || bearer.isEmpty())) {
            throw new IllegalArgumentException(
                    "analytics.database.auth = bearer requires auth.bearer_token");
        }
        Duration connectTimeout = Duration.ofMillis(parseLong(p,
                "analytics.database.connect_timeout_ms", 5_000));
        Duration readTimeout = Duration.ofMillis(parseLong(p,
                "analytics.database.read_timeout_ms", 60_000));
        int retry = (int) parseLong(p, "analytics.database.retry.max_attempts", 3);
        int ingestBatch = (int) parseLong(p, "analytics.database.ingest.batch_rows", 10_000);
        int ingestConcurrency = (int) parseLong(p,
                "analytics.database.ingest.max_concurrency", 4);
        String prefix = p.getProperty("analytics.database.partition_prefix", "p");
        ProvenanceMode provenance;
        try {
            provenance = ProvenanceMode.valueOf(
                    p.getProperty("analytics.database.provenance", "warn").toUpperCase());
        } catch (IllegalArgumentException e) {
            throw new IllegalArgumentException(
                    "analytics.database.provenance must be strict / warn / lenient", e);
        }
        return new DapConfiguration(
                URI.create(baseUrl), auth, username, password, bearer,
                connectTimeout, readTimeout, retry, ingestBatch, ingestConcurrency,
                prefix, provenance);
    }

    private static String requireProp(Properties p, String key) {
        String v = p.getProperty(key);
        if (v == null || v.isEmpty()) {
            throw new IllegalArgumentException(key + " is required");
        }
        return v;
    }

    private static long parseLong(Properties p, String key, long def) {
        String v = p.getProperty(key);
        if (v == null) return def;
        try {
            return Long.parseLong(v.trim());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(key + " must be an integer", e);
        }
    }
}

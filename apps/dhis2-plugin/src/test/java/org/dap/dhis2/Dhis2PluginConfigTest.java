// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.*;

class Dhis2PluginConfigTest {

    @Test
    void basicAuthRequiresUsernameAndPassword() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "basic");
        p.setProperty("analytics.database.auth.username", "u");
        p.setProperty("analytics.database.auth.password", "s");
        Dhis2PluginConfig c = Dhis2PluginConfig.fromProperties(p);
        assertEquals("u", c.username());
        assertEquals(Dhis2PluginConfig.AuthMode.BASIC, c.authMode());
        assertEquals(Duration.ofMillis(5000), c.connectTimeout());
        assertEquals(Duration.ofMillis(60000), c.readTimeout());
        assertEquals(3, c.retryMaxAttempts());
        assertEquals("p", c.partitionPrefix());
        assertEquals(Dhis2PluginConfig.ProvenanceMode.WARN, c.provenance());
    }

    @Test
    void bearerRequiresToken() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "bearer");
        IllegalArgumentException e = assertThrows(
                IllegalArgumentException.class,
                () -> Dhis2PluginConfig.fromProperties(p));
        assertTrue(e.getMessage().contains("bearer_token"));
    }

    @Test
    void customTimeoutsAndProvenance() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "none");
        p.setProperty("analytics.database.connect_timeout_ms", "250");
        p.setProperty("analytics.database.read_timeout_ms", "30000");
        p.setProperty("analytics.database.retry.max_attempts", "5");
        p.setProperty("analytics.database.partition_prefix", "edge");
        p.setProperty("analytics.database.provenance", "strict");
        Dhis2PluginConfig c = Dhis2PluginConfig.fromProperties(p);
        assertEquals(Duration.ofMillis(250), c.connectTimeout());
        assertEquals(Duration.ofMillis(30000), c.readTimeout());
        assertEquals(5, c.retryMaxAttempts());
        assertEquals("edge", c.partitionPrefix());
        assertEquals(Dhis2PluginConfig.ProvenanceMode.STRICT, c.provenance());
    }

    @Test
    void missingBaseUrlRejects() {
        Properties p = new Properties();
        assertThrows(IllegalArgumentException.class,
                () -> Dhis2PluginConfig.fromProperties(p));
    }

    @Test
    void badAuthModeRejects() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "oauth2");
        assertThrows(IllegalArgumentException.class,
                () -> Dhis2PluginConfig.fromProperties(p));
    }
}

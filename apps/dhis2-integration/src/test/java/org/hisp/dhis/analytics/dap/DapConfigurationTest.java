// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.Properties;

import static org.junit.jupiter.api.Assertions.*;

class DapConfigurationTest {

    @Test
    void basicAuthRequiresUsernameAndPassword() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "basic");
        p.setProperty("analytics.database.auth.username", "u");
        p.setProperty("analytics.database.auth.password", "s");
        DapConfiguration c = DapConfiguration.fromProperties(p);
        assertEquals(DapConfiguration.AuthMode.BASIC, c.authMode());
        assertEquals(Duration.ofMillis(5000), c.connectTimeout());
        assertEquals(Duration.ofMillis(60000), c.readTimeout());
        assertEquals(10_000, c.ingestBatchRows());
        assertEquals(4, c.ingestMaxConcurrency());
    }

    @Test
    void ingestKnobsParse() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "none");
        p.setProperty("analytics.database.ingest.batch_rows", "2500");
        p.setProperty("analytics.database.ingest.max_concurrency", "8");
        DapConfiguration c = DapConfiguration.fromProperties(p);
        assertEquals(2500, c.ingestBatchRows());
        assertEquals(8, c.ingestMaxConcurrency());
    }

    @Test
    void bearerRequiresToken() {
        Properties p = new Properties();
        p.setProperty("analytics.database.base_url", "https://dap.local:8443");
        p.setProperty("analytics.database.auth", "bearer");
        assertThrows(IllegalArgumentException.class, () -> DapConfiguration.fromProperties(p));
    }

    @Test
    void missingBaseUrlRejects() {
        assertThrows(IllegalArgumentException.class,
                () -> DapConfiguration.fromProperties(new Properties()));
    }
}

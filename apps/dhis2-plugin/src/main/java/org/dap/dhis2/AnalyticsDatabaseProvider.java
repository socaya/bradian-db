// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import java.util.Properties;

/**
 * Service-provider interface the DHIS2 build (2.41+) wires to its own
 * {@code org.hisp.dhis.analytics.AnalyticsDatabase} contract via a shaded dependency.
 *
 * <p>DHIS2 2.41 is the minimum compatible version — it introduced the pluggable
 * analytics-engine SPI we bind to. Earlier DHIS2 versions do not scan
 * {@code META-INF/services} for analytics providers and therefore cannot load this
 * plug-in.
 *
 * <p>Keeping this interface local lets us ship a DHIS2-version-agnostic plug-in. The
 * DHIS2 adapter module only has to:
 *
 * <ol>
 *   <li>Load the {@link java.util.ServiceLoader} entries for this interface.</li>
 *   <li>Match {@code analytics.database = dap} to our {@code name()}.</li>
 *   <li>Forward DHIS2's internal query shape into {@link DapAnalyticsRequest}.</li>
 *   <li>Return the result grid DHIS2 expects, derived from {@link DapAnalyticsResponse}.</li>
 * </ol>
 */
public interface AnalyticsDatabaseProvider {

    /**
     * Identifier matched by DHIS2's {@code analytics.database} property. For this
     * plug-in: {@code "dap"}.
     */
    String name();

    /**
     * Factory method called once by DHIS2 at server startup. Implementations should
     * fail-fast on misconfiguration.
     */
    AnalyticsDatabase create(Properties dhisConf);
}

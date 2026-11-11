// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics;

/**
 * Local stand-in for DHIS2's canonical {@code org.hisp.dhis.analytics.AnalyticsDatabase}
 * enum. The real enum already ships {@code POSTGRESQL}, {@code DORIS} and
 * {@code CLICKHOUSE} in DHIS2 2.41+. Our contribution is the {@code DAP} constant.
 *
 * <p>When the upstream PR lands, DHIS2's authoritative enum gains the {@code DAP}
 * entry and this file deletes itself; here we keep a minimal copy so the rest of the
 * sources in this directory compile without a full DHIS2 source checkout.
 */
public enum AnalyticsDatabase {
    POSTGRESQL("postgresql"),
    DORIS("doris"),
    CLICKHOUSE("clickhouse"),
    DAP("dap");

    private final String key;

    AnalyticsDatabase(String key) {
        this.key = key;
    }

    public String getKey() {
        return key;
    }

    public static AnalyticsDatabase fromKey(String key) {
        for (AnalyticsDatabase db : values()) {
            if (db.key.equalsIgnoreCase(key)) return db;
        }
        throw new IllegalArgumentException("unknown analytics.database: " + key);
    }
}

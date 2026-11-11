// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Canonical ingestion batch posted to {@code /api/analytics/v1/ingest} during
 * analytics-table rebuild. Each {@link Change} entry carries the same shape as
 * {@code dap_ingest::Change} on the Rust side, so the DAP ingestion engine's version
 * fences, quarantine log and WAL path all stay engaged.
 */
public final class DapIngestBatch
        implements org.hisp.dhis.analytics.AnalyticsTableManager.DapIngest {

    public final String dataset;
    public final String partition;
    public final List<Change> changes;

    public DapIngestBatch(String dataset, String partition, List<Change> changes) {
        this.dataset = Objects.requireNonNull(dataset, "dataset");
        this.partition = Objects.requireNonNull(partition, "partition");
        this.changes = Objects.requireNonNull(changes, "changes");
    }

    public static final class Change {
        public final String changeId;
        public final String siteId;
        public final String objectType;
        public final String objectUid;
        public final long sourceVersion;
        public final String operation; // "UPSERT" / "DELETE"
        public final String occurredAt;
        public final Map<String, Object> payload;

        public Change(String changeId, String siteId, String objectType, String objectUid,
                      long sourceVersion, String operation, String occurredAt,
                      Map<String, Object> payload) {
            this.changeId = Objects.requireNonNull(changeId);
            this.siteId = siteId;
            this.objectType = Objects.requireNonNull(objectType);
            this.objectUid = Objects.requireNonNull(objectUid);
            this.sourceVersion = sourceVersion;
            this.operation = Objects.requireNonNull(operation);
            this.occurredAt = Objects.requireNonNull(occurredAt);
            this.payload = Objects.requireNonNull(payload);
        }
    }

    public static final class Builder {
        private final String dataset;
        private final String partition;
        private final List<Change> changes = new ArrayList<>();

        public Builder(String dataset, String partition) {
            this.dataset = dataset;
            this.partition = partition;
        }

        public Builder upsert(String changeId, String objectUid, long version, String occurredAt,
                              Map<String, Object> payload) {
            changes.add(new Change(changeId, null, "AGGREGATE_DATA_VALUE", objectUid, version,
                    "UPSERT", occurredAt, payload));
            return this;
        }

        public Builder delete(String changeId, String objectUid, long version, String occurredAt,
                              Map<String, Object> payload) {
            changes.add(new Change(changeId, null, "AGGREGATE_DATA_VALUE", objectUid, version,
                    "DELETE", occurredAt, payload));
            return this;
        }

        public int size() { return changes.size(); }

        public DapIngestBatch build() {
            return new DapIngestBatch(dataset, partition, changes);
        }
    }
}

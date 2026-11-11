// SPDX-License-Identifier: Apache-2.0
package org.hisp.dhis.analytics.dap;

import org.hisp.dhis.analytics.AnalyticsManager.AnalyticsException;
import org.hisp.dhis.analytics.AnalyticsTableManager;

import java.io.IOException;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Builds the DAP analytics tables on the DHIS2 rebuild schedule. DHIS2 calls:
 *
 * <ol>
 *   <li>{@link #prepareRebuild()} once at the start of a rebuild.</li>
 *   <li>{@link #ingestBatch(DapIngest)} repeatedly — DHIS2's rebuild framework streams
 *       rows out of the operational PostgreSQL in configurable batches
 *       ({@code analytics.database.ingest.batch_rows}).</li>
 *   <li>{@link #finaliseRebuild()} once the stream completes — the engine flushes the
 *       in-memory delta and compacts.</li>
 * </ol>
 *
 * <p>The network path mirrors the Doris v43 model: DHIS2 owns the stream, the engine
 * owns durability. DAP's ingest HTTP endpoint applies the usual version fences and
 * quarantine rules.
 */
public final class DapAnalyticsTableManager implements AnalyticsTableManager {

    private final DapHttpClient client;
    private final String dataset;
    private final String partition;
    private final AtomicLong batchCount = new AtomicLong();

    public DapAnalyticsTableManager(DapHttpClient client, String dataset, String partition) {
        this.client = Objects.requireNonNull(client);
        this.dataset = Objects.requireNonNull(dataset);
        this.partition = Objects.requireNonNull(partition);
    }

    @Override
    public void prepareRebuild() throws AnalyticsException {
        // DAP does not require explicit preparation — the engine routes the first batch
        // to a freshly-created partition if needed. Keep the hook so the DHIS2 upstream
        // sequence remains intact (and so future partition-reset semantics can land
        // here without changing the interface).
        batchCount.set(0);
    }

    @Override
    public void ingestBatch(DapIngest batch) throws AnalyticsException {
        DapIngestBatch bundle;
        if (batch instanceof DapIngestBatch concrete) {
            bundle = concrete;
        } else {
            throw new AnalyticsException(
                    "DapAnalyticsTableManager.ingestBatch expects DapIngestBatch instances");
        }
        try {
            client.postIngest(bundle);
            batchCount.incrementAndGet();
        } catch (IOException e) {
            throw new AnalyticsException("DAP ingest failed: " + e.getMessage(), e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new AnalyticsException("DAP ingest interrupted", e);
        }
    }

    @Override
    public void finaliseRebuild() throws AnalyticsException {
        // The DAP engine flushes the in-memory delta + runs compaction on its own
        // schedule; there is no required finalise call. We expose the hook so DHIS2's
        // rebuild framework can log completion with the batch count.
    }

    public long batchesSent() { return batchCount.get(); }

    public String dataset()   { return dataset; }
    public String partition() { return partition; }
}

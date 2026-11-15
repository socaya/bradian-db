import { BrowserRouter, Route, Routes } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

import { Shell } from '@/components/Shell';
import { OverviewPage } from '@/pages/Overview';
import { DataSourcesPage } from '@/pages/DataSources';
import { StorageCatalogPage } from '@/pages/StorageCatalog';
import { QueryWorkbenchPage } from '@/pages/QueryWorkbench';
import { QueryHistoryPage } from '@/pages/QueryHistory';
import { MaterializedViewsPage } from '@/pages/MaterializedViews';
import { DataProductsPage } from '@/pages/DataProducts';
import { DistributedNodesPage } from '@/pages/DistributedNodes';
import { ReconciliationPage } from '@/pages/Reconciliation';
import { Dhis2ParityPage } from '@/pages/Dhis2Parity';
import { SecurityPage } from '@/pages/Security';
import { BackupsPage } from '@/pages/Backups';
import { BenchmarksPage } from '@/pages/Benchmarks';
import { SettingsPage } from '@/pages/Settings';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
      staleTime: 10_000,
    },
  },
});

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route element={<Shell />}>
            <Route path="/" element={<OverviewPage />} />
            <Route path="/sources" element={<DataSourcesPage />} />
            <Route path="/storage" element={<StorageCatalogPage />} />
            <Route path="/workbench" element={<QueryWorkbenchPage />} />
            <Route path="/history" element={<QueryHistoryPage />} />
            <Route path="/views" element={<MaterializedViewsPage />} />
            <Route path="/products" element={<DataProductsPage />} />
            <Route path="/nodes" element={<DistributedNodesPage />} />
            <Route path="/reconciliation" element={<ReconciliationPage />} />
            <Route path="/dhis2-parity" element={<Dhis2ParityPage />} />
            <Route path="/security" element={<SecurityPage />} />
            <Route path="/backups" element={<BackupsPage />} />
            <Route path="/benchmarks" element={<BenchmarksPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}

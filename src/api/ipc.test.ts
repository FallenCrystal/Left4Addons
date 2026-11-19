import { describe, expect, it, vi, beforeEach } from 'vitest';
import {
  getDashboardSnapshot,
  isTauriRuntime,
  resolveReconcileIssue,
  scanWorkspace,
  syncWorkshop,
} from './ipc';

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));

describe('v2 IPC client wrapper', () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it('detects non-tauri browser preview safely', () => {
    expect(isTauriRuntime()).toBe(false);
  });

  it('forwards core v2 commands with exact snake/camel payload mapping', async () => {
    invokeMock.mockResolvedValue({});

    await getDashboardSnapshot();
    expect(invokeMock).toHaveBeenCalledWith('get_dashboard_snapshot');

    await scanWorkspace();
    expect(invokeMock).toHaveBeenCalledWith('scan_workspace');

    await syncWorkshop();
    expect(invokeMock).toHaveBeenCalledWith('sync_workshop');

    await resolveReconcileIssue('overwritten_888', 'adopt_workshop_update');
    expect(invokeMock).toHaveBeenCalledWith('resolve_reconcile_issue', {
      issueId: 'overwritten_888',
      action: 'adopt_workshop_update',
    });
  });
});

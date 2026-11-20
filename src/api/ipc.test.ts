import { describe, expect, it, vi, beforeEach } from 'vitest';
import {
  applyProfile,
  applyRenames,
  deleteAddons,
  getDashboardSnapshot,
  IpcError,
  isTauriRuntime,
  moveAddons,
  resolveReconcileIssue,
  scanWorkspace,
  syncWorkshop,
  toggleAddons,
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

  it('preserves results from a partially completed batch', async () => {
    const report = {
      items: [
        { addonId: 'first', status: 'applied' },
        { addonId: 'second', status: 'failed', error: { code: 'conflict', message: '目标文件已存在' } },
        { addonId: 'third', status: 'not_attempted' },
      ],
    };
    invokeMock.mockResolvedValue(report);
    const addonIds = ['first', 'second', 'third'];

    expect(await moveAddons(addonIds, 'loading')).toEqual(report);
    expect(invokeMock).toHaveBeenLastCalledWith('move_addons', { addonIds, targetDirType: 'loading' });

    await toggleAddons(addonIds, false);
    expect(invokeMock).toHaveBeenLastCalledWith('toggle_addons', { addonIds, enabled: false, allowWorkshopDir: false });

    await applyRenames([['first', 'New name.vpk']]);
    expect(invokeMock).toHaveBeenLastCalledWith('apply_renames', { renames: [['first', 'New name.vpk']] });

    await deleteAddons(addonIds, true, false);
    expect(invokeMock).toHaveBeenLastCalledWith('delete_addons', { addonIds, deleteDiskFile: true, purgeKnownRecord: false });

    await applyProfile('profile');
    expect(invokeMock).toHaveBeenLastCalledWith('apply_profile', { profileId: 'profile' });
  });

  it.each([
    [{ code: 'conflict', message: '对账问题已变化' }, 'conflict', '对账问题已变化'],
    ['invalid invoke arguments', 'ipc', 'invalid invoke arguments'],
  ])('exposes an IPC error code and message', async (rejection, code, message) => {
    invokeMock.mockRejectedValue(rejection);
    const result = scanWorkspace();
    await expect(result).rejects.toBeInstanceOf(IpcError);
    await expect(result).rejects.toMatchObject({ code, message });
  });
});

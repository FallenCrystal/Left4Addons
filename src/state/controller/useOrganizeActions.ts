import type { Dispatch, RefObject, SetStateAction } from 'react';
import { useCallback } from 'react';
import type { LoadoutProfile, UserGroup } from '../../api/ipc';
import { summarizeBatchReport } from '../../utils/format';
import type { BatchDialogPayload, DesktopClient, NotifyFn, TranslateFn } from './types';

export interface UseOrganizeActionsParams {
  client: DesktopClient;
  tRef: RefObject<TranslateFn>;
  notify: NotifyFn;
  refresh: () => Promise<boolean>;
  setBusy: Dispatch<SetStateAction<boolean>>;
  setBatchReport: Dispatch<SetStateAction<BatchDialogPayload | null>>;
}

export function useOrganizeActions({
  client,
  tRef,
  notify,
  refresh,
  setBusy,
  setBatchReport,
}: UseOrganizeActionsParams) {
  const rebuildBundles = useCallback(async () => {
    setBusy(true);
    try {
      const bundles = await client.rebuildBundles();
      const refreshed = await refresh();
      if (refreshed) {
        notify(tRef.current('toast.bundleRebuilt', { count: bundles.length }));
      }
    } catch (err) {
      notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
    } finally {
      setBusy(false);
    }
  }, [client, notify, refresh, setBusy, tRef]);

  const createManualBundle = useCallback(
    async (name: string, memberAddonIds: string[]) => {
      setBusy(true);
      try {
        await client.createManualBundle(name, memberAddonIds);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const deleteBundle = useCallback(
    async (bundleId: string) => {
      setBusy(true);
      try {
        await client.deleteBundle(bundleId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const setBundleLocked = useCallback(
    async (bundleId: string, locked: boolean) => {
      setBusy(true);
      try {
        await client.setBundleLocked(bundleId, locked);
        await refresh();
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy],
  );

  const trackCollection = useCallback(
    async (collectionId: string, autoSync: boolean) => {
      setBusy(true);
      try {
        const record = await client.trackCollection(collectionId, autoSync);
        const refreshed = await refresh();
        if (refreshed) {
          notify(tRef.current('toast.collectionTracked', { title: record.title }));
        }
        return record;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const untrackCollection = useCallback(
    async (collectionId: string) => {
      setBusy(true);
      try {
        await client.untrackCollection(collectionId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const saveUserGroup = useCallback(
    async (group: UserGroup) => {
      setBusy(true);
      try {
        await client.saveUserGroup(group);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const deleteUserGroup = useCallback(
    async (groupId: string) => {
      setBusy(true);
      try {
        await client.deleteUserGroup(groupId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const saveProfile = useCallback(
    async (profile: LoadoutProfile) => {
      setBusy(true);
      try {
        await client.saveProfile(profile);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
        return true;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return false;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const deleteProfile = useCallback(
    async (profileId: string) => {
      setBusy(true);
      try {
        await client.deleteProfile(profileId);
        const refreshed = await refresh();
        if (refreshed) notify(tRef.current('common.done'));
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBusy, tRef],
  );

  const applyProfile = useCallback(
    async (profileId: string) => {
      setBusy(true);
      try {
        const report = await client.applyProfile(profileId);
        const refreshed = await refresh();
        const counts = summarizeBatchReport(report.batch);
        if (counts.hasIssue) {
          setBatchReport({
            batch: report.batch,
            profileSummary: {
              enabledCount: report.enabledCount,
              disabledCount: report.disabledCount,
              skippedWorkshopOnly: report.skippedWorkshopOnly,
              missingAddons: report.missingAddons,
            },
          });
        } else if (refreshed) {
          const parts = [
            tRef.current('organize.profileApplied', {
              enabled: report.enabledCount,
              disabled: report.disabledCount,
            }),
          ];
          if (report.skippedWorkshopOnly.length > 0) {
            parts.push(
              tRef.current('organize.profileSkipped', {
                count: report.skippedWorkshopOnly.length,
              }),
            );
          }
          if (report.missingAddons.length > 0) {
            parts.push(
              tRef.current('organize.profileMissing', { count: report.missingAddons.length }),
            );
          }
          notify(parts.join(' · '));
        }
        return report;
      } catch (err) {
        notify(err instanceof Error ? err.message : String(err), { tone: 'error' });
        return null;
      } finally {
        setBusy(false);
      }
    },
    [client, notify, refresh, setBatchReport, setBusy, tRef],
  );

  return {
    rebuildBundles,
    createManualBundle,
    deleteBundle,
    setBundleLocked,
    trackCollection,
    untrackCollection,
    saveUserGroup,
    deleteUserGroup,
    saveProfile,
    deleteProfile,
    applyProfile,
  };
}

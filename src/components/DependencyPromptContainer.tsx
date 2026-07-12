import React, { useEffect, useState, useRef } from 'react';
import { Addon, BackgroundTask, Settings } from '../types/addon';
import { DependencyDownloadItem, DependencyPromptModal } from './DependencyPromptModal';

type DiscoveredDependency = NonNullable<
  NonNullable<BackgroundTask['dependencyCheck']>['discoveredDependencies']
>[number];

interface DependencyPromptContainerProps {
  addons: Record<string, Addon>;
  knownUninstalledAddons: Record<string, Addon>;
  backgroundTasks: BackgroundTask[];
  settings: Settings;
  onDownload: (items: DependencyDownloadItem[]) => void;
  onGoToSettings: () => void;
}

export const DependencyPromptContainer: React.FC<DependencyPromptContainerProps> = ({
  addons,
  knownUninstalledAddons,
  backgroundTasks,
  settings,
  onDownload,
  onGoToSettings,
}) => {
  const behavior = settings.dependencyMissingBehavior || 'ask';
  
  const [modalOpen, setModalOpen] = useState(false);
  const [missingDependencies, setMissingDependencies] = useState<Addon[]>([]);
  const [isScanning, setIsScanning] = useState(false);

  const toDownloadItems = (items: Addon[]): DependencyDownloadItem[] => (
    items.map((addon) => ({
      workshopId: addon.workshopId || addon.id,
      title: addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName,
      imagePath: addon.imagePath || addon.workshopDetails?.previewUrl,
    }))
  );
  
  // Track previous task counts to detect completion
  const prevDownloadCountRef = useRef(0);
  const prevScanCountRef = useRef(0);
  const pendingDependencyEvaluationRef = useRef(false);
  const pendingDependencyCandidatesRef = useRef<Map<string, DiscoveredDependency>>(new Map());
  
  // To avoid repeatedly prompting for the same missing dependencies if user clicks "Cancel" or "Ignore",
  // we can maintain a Set of ignored workshop IDs per session.
  const ignoredDependencyIdsRef = useRef<Set<string>>(new Set());

  // Determine current active task counts
  const activeDownloadTasks = backgroundTasks.filter(t => t.kind === 'download' && (t.status === 'queued' || t.status === 'running'));
  const activeScanTasks = backgroundTasks.filter(t => t.kind === 'dependency-check' && (t.status === 'queued' || t.status === 'running'));
  
  const currentDownloadCount = activeDownloadTasks.length;
  const currentScanCount = activeScanTasks.length;
  const currentlyScanning = currentScanCount > 0;

  // Compute missing dependencies globally
  const calculateMissingDependencies = (taskDependencies: DiscoveredDependency[] = []) => {
    const missingIds = new Set<string>();
    
    // Check requiredItems of all addons we have (installed, disabled, downloading, or in knownUninstalled)
    const ownedAddons = new Set<Addon>();
    
    // 1. Addons that are installed
    Object.values(addons).forEach(addon => {
      if (addon.dirType !== 'none') {
        ownedAddons.add(addon);
      }
    });
    
    // Helper to find addon by workshopId
    const findAddon = (id: string) => {
      const normalizedId = String(id || '').trim();
      if (!normalizedId) return undefined;
      
      const direct = addons[normalizedId] || knownUninstalledAddons[normalizedId];
      if (direct) return direct;

      return Object.values(addons).find(a => 
        a.workshopId === normalizedId || a.id === normalizedId
      ) || Object.values(knownUninstalledAddons).find(a => 
        a.workshopId === normalizedId || a.id === normalizedId
      );
    };

    // 2. Addons that are in download queue
    activeDownloadTasks.forEach(task => {
      task.targetIds.forEach(id => {
        const addon = findAddon(id);
        if (addon) {
          ownedAddons.add(addon);
        }
      });
    });

    // Gather their required items
    const requiredItems = new Map<string, DiscoveredDependency>();
    
    ownedAddons.forEach(addon => {
      if (addon.workshopDetails?.requiredItems) {
        addon.workshopDetails.requiredItems.forEach(req => {
          requiredItems.set(req.workshopId, req);
        });
      }
    });

    taskDependencies.forEach((dependency) => {
      const workshopId = dependency.workshopId?.trim();
      if (workshopId) {
        requiredItems.set(workshopId, { ...dependency, workshopId });
      }
    });

    // Filter out dependencies that are already installed or in download queue
    requiredItems.forEach((_, reqId) => {
      const isInstalled = Object.values(addons).some(a => (a.workshopId === reqId || a.id === reqId) && a.dirType !== 'none');
      const isDownloading = activeDownloadTasks.some(t => t.targetIds.includes(reqId));
      const isIgnored = ignoredDependencyIdsRef.current.has(reqId);
      
      if (!isInstalled && !isDownloading && !isIgnored) {
        missingIds.add(reqId);
      }
    });

    // Map ids back to full Addon objects for the UI
    return Array.from(missingIds).map(id => {
      const existing = findAddon(id);
      if (existing) return existing;
      
      // Fallback if not found in db yet
      const reqInfo = requiredItems.get(id);
      return {
        id,
        vpkName: id,
        dirType: 'none',
        isEnabled: false,
        fileSize: 0,
        filesCount: 0,
        workshopId: id,
        imagePath: reqInfo?.previewUrl,
        addonInfo: {
          addontitle: reqInfo?.title || id
        },
        steamDetails: {
          title: reqInfo?.title || id,
          creator_name: reqInfo?.creatorName,
        },
        workshopDetails: {
          title: reqInfo?.title || id,
          previewUrl: reqInfo?.previewUrl,
          creatorName: reqInfo?.creatorName,
          authorName: reqInfo?.creatorName,
        },
      } as Addon;
    });
  };

  useEffect(() => {
    // Check for transition from >0 to 0
    const downloadsFinished = prevDownloadCountRef.current > 0 && currentDownloadCount === 0;
    const scansFinished = prevScanCountRef.current > 0 && currentScanCount === 0;
    
    if (downloadsFinished || scansFinished) {
      pendingDependencyEvaluationRef.current = true;
      if (scansFinished) {
        backgroundTasks
          .filter((task) => task.kind === 'dependency-check' && task.status === 'completed')
          .flatMap((task) => task.dependencyCheck?.discoveredDependencies || [])
          .forEach((dependency) => {
            const workshopId = dependency.workshopId?.trim();
            if (workshopId) {
              pendingDependencyCandidatesRef.current.set(workshopId, {
                workshopId,
                title: dependency.title,
                previewUrl: dependency.previewUrl,
                creatorName: dependency.creatorName,
              });
            }
          });
      }
    }

    if (pendingDependencyEvaluationRef.current) {
      if (behavior === 'ignore') {
        pendingDependencyEvaluationRef.current = false;
      } else {
        const missing = calculateMissingDependencies([...pendingDependencyCandidatesRef.current.values()]);
        
        if (missing.length > 0) {
          pendingDependencyEvaluationRef.current = false;
          if (behavior === 'always') {
            // Automatically download them
            onDownload(toDownloadItems(missing));
          } else if (behavior === 'ask') {
            // Check if we are already showing the modal. 
            // We can update the missing dependencies list or open the modal.
            setMissingDependencies(missing);
            setIsScanning(currentlyScanning);
            setModalOpen(true);
          }
        } else if (!currentlyScanning && currentDownloadCount === 0) {
           // The dependency task can complete before its database payload reaches
           // this component. Keep evaluating when addon state changes.
           setModalOpen(false);
        }
      }
    } else if (modalOpen) {
       // Update modal state dynamically if it's open (e.g. scanning state changes)
       setIsScanning(currentlyScanning);
       // We can also dynamically update the missing list if new ones are found or some started downloading
       const missing = calculateMissingDependencies([...pendingDependencyCandidatesRef.current.values()]);
       if (missing.length === 0) {
         setModalOpen(false);
       } else {
         setMissingDependencies(missing);
       }
    }

    prevDownloadCountRef.current = currentDownloadCount;
    prevScanCountRef.current = currentScanCount;
    
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentDownloadCount, currentScanCount, behavior, addons, knownUninstalledAddons, backgroundTasks]);

  const handleDownload = (items: DependencyDownloadItem[]) => {
    onDownload(items);
    setModalOpen(false);
  };

  const handleCancel = () => {
    // If not scanning and user cancels, we add these to ignored list so they aren't repeatedly prompted
    if (!currentlyScanning) {
      missingDependencies.forEach(m => ignoredDependencyIdsRef.current.add(m.workshopId || m.id));
    }
    setModalOpen(false);
  };
  
  const handleGoToSettings = () => {
    setModalOpen(false);
    onGoToSettings();
  };

  return (
    <DependencyPromptModal
      open={modalOpen}
      missingDependencies={missingDependencies}
      isScanning={isScanning}
      onDownload={handleDownload}
      onCancel={handleCancel}
      onGoToSettings={handleGoToSettings}
    />
  );
};

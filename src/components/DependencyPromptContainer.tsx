import React, { useEffect, useState, useRef } from 'react';
import { Addon, BackgroundTask, Settings } from '../types/addon';
import { DependencyPromptModal } from './DependencyPromptModal';

interface DependencyPromptContainerProps {
  addons: Record<string, Addon>;
  knownUninstalledAddons: Record<string, Addon>;
  backgroundTasks: BackgroundTask[];
  settings: Settings;
  onDownload: (items: string[]) => void;
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
  
  // Track previous task counts to detect completion
  const prevDownloadCountRef = useRef(0);
  const prevScanCountRef = useRef(0);
  
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
  const calculateMissingDependencies = () => {
    const missingIds = new Set<string>();
    
    // Check requiredItems of all addons we have (installed, disabled, downloading, or in knownUninstalled)
    // Actually, maybe we only care about installed addons (dirType !== 'none') or addons currently being downloaded.
    // Let's gather all addons that user "owns" or is trying to own.
    const ownedIds = new Set<string>();
    
    // 1. Addons that are installed
    Object.values(addons).forEach(addon => {
      if (addon.dirType !== 'none') {
        ownedIds.add(addon.workshopId || addon.id);
      }
    });
    
    // 2. Addons that are in download queue
    activeDownloadTasks.forEach(task => {
      task.targetIds.forEach(id => ownedIds.add(id));
    });

    // Gather their required items
    const requiredItems = new Map<string, { workshopId: string, title?: string }>();
    
    ownedIds.forEach(id => {
      const addon = addons[id] || knownUninstalledAddons[id];
      if (addon && addon.workshopDetails?.requiredItems) {
        addon.workshopDetails.requiredItems.forEach(req => {
          requiredItems.set(req.workshopId, req);
        });
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
      const existing = addons[id] || knownUninstalledAddons[id];
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
        addonInfo: {
          addontitle: reqInfo?.title || id
        }
      } as Addon;
    });
  };

  useEffect(() => {
    // Check for transition from >0 to 0
    const downloadsFinished = prevDownloadCountRef.current > 0 && currentDownloadCount === 0;
    const scansFinished = prevScanCountRef.current > 0 && currentScanCount === 0;
    
    if (downloadsFinished || scansFinished) {
      if (behavior === 'ignore') {
        // Do nothing
      } else {
        const missing = calculateMissingDependencies();
        
        if (missing.length > 0) {
          if (behavior === 'always') {
            // Automatically download them
            onDownload(missing.map(m => m.workshopId || m.id));
          } else if (behavior === 'ask') {
            // Check if we are already showing the modal. 
            // We can update the missing dependencies list or open the modal.
            setMissingDependencies(missing);
            setIsScanning(currentlyScanning);
            setModalOpen(true);
          }
        } else if (!currentlyScanning && currentDownloadCount === 0) {
           // Close modal if no missing deps and everything finished
           setModalOpen(false);
        }
      }
    } else if (modalOpen) {
       // Update modal state dynamically if it's open (e.g. scanning state changes)
       setIsScanning(currentlyScanning);
       // We can also dynamically update the missing list if new ones are found or some started downloading
       const missing = calculateMissingDependencies();
       if (missing.length === 0) {
         setModalOpen(false);
       } else {
         setMissingDependencies(missing);
       }
    }

    prevDownloadCountRef.current = currentDownloadCount;
    prevScanCountRef.current = currentScanCount;
    
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentDownloadCount, currentScanCount, behavior]);

  const handleDownload = (ids: string[]) => {
    onDownload(ids);
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

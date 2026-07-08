type WorkshopLike = {
  id?: string;
  vpkName?: string;
  workshopId?: string;
};

function normalizeId(value: unknown): string {
  return String(value || '').trim();
}

export function findKnownWorkshopEntry<T extends WorkshopLike>(
  entries: Record<string, T> | undefined,
  workshopId: string | undefined,
): T | undefined {
  const id = normalizeId(workshopId);
  if (!entries || !id) return undefined;

  // Read legacy ".vpk" keys, but keep workshop identity keyed by the raw
  // publishedfileid everywhere new code can control.
  const direct = entries[id] || entries[`${id}.vpk`];
  if (direct) return direct;

  return Object.values(entries).find((entry) => (
    normalizeId(entry?.workshopId) === id ||
    normalizeId(entry?.id) === id ||
    normalizeId(entry?.id) === `${id}.vpk` ||
    normalizeId(entry?.vpkName) === `${id}.vpk`
  ));
}

export function isKnownWorkshopItem<T extends WorkshopLike>(
  entries: Record<string, T> | undefined,
  workshopId: string | undefined,
): boolean {
  return Boolean(findKnownWorkshopEntry(entries, workshopId));
}

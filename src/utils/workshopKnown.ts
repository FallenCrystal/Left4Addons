type WorkshopLike = {
  id?: string;
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

  const direct = entries[id];
  if (direct) return direct;

  return Object.values(entries).find((entry) => (
    normalizeId(entry?.workshopId) === id ||
    normalizeId(entry?.id) === id
  ));
}

export function isKnownWorkshopItem<T extends WorkshopLike>(
  entries: Record<string, T> | undefined,
  workshopId: string | undefined,
): boolean {
  return Boolean(findKnownWorkshopEntry(entries, workshopId));
}

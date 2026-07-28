const STORAGE_KEY = 'camellia-nexus.program-catalog';

export interface ProgramCatalog {
  version: 1;
  order: string[];
}

const emptyCatalog = (): ProgramCatalog => ({ version: 1, order: [] });

export function loadCatalog(): ProgramCatalog {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '') as Partial<ProgramCatalog>;
    if (!parsed || parsed.version !== 1) return emptyCatalog();
    const order = Array.isArray(parsed.order)
      ? parsed.order.filter((id): id is string => typeof id === 'string')
      : [];
    const catalog = { version: 1, order: [...new Set(order)] } satisfies ProgramCatalog;
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(catalog));
    } catch {
      // Keep the recovered order for this session when persistence is unavailable.
    }
    return catalog;
  } catch {
    return emptyCatalog();
  }
}

export function reconcileCatalog(catalog: ProgramCatalog, ids: string[]) {
  const valid = new Set(ids);
  const order = catalog.order.filter((id) => valid.has(id));
  for (const id of ids) if (!order.includes(id)) order.push(id);
  return { version: 1, order } satisfies ProgramCatalog;
}

export function saveCatalog(catalog: ProgramCatalog) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(catalog));
  } catch {
    // Organization metadata is non-critical; keep the in-memory state available.
  }
}

export function moveCatalogItem(catalog: ProgramCatalog, id: string, beforeId?: string) {
  const order = catalog.order.filter((candidate) => candidate !== id);
  const target = beforeId ? order.indexOf(beforeId) : -1;
  if (target < 0) order.push(id);
  else order.splice(target, 0, id);
  return { ...catalog, order };
}

export function moveCatalogItemBy(catalog: ProgramCatalog, id: string, offset: -1 | 1) {
  const index = catalog.order.indexOf(id);
  const target = index + offset;
  if (index < 0 || target < 0 || target >= catalog.order.length) return catalog;
  const order = [...catalog.order];
  [order[index], order[target]] = [order[target], order[index]];
  return { ...catalog, order };
}

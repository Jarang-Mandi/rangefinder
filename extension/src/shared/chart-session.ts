/** dataReady tidak selalu memanggil callback ketika iframe dibuang. */
export function waitDataReady(register: (ready: () => void) => boolean, timeoutMs: number): Promise<boolean> {
  return new Promise(resolve => {
    let done = false;
    const finish = (ready: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve(ready);
    };
    const timer = setTimeout(() => finish(false), Math.max(0, timeoutMs));
    try { if (register(() => finish(true))) finish(true); }
    catch { finish(false); }
  });
}

type Side = "upper" | "lower";
/** Hasil createShape yang terlambat dibersihkan dari chart pemiliknya. */
export async function drawOwnedPair(
  create: (side: Side) => Promise<string>,
  remove: (id: string) => void,
  current: () => boolean,
): Promise<Record<Side, string> | null> {
  const owned: string[] = [];
  let committed = false;
  try {
    if (!current()) return null;
    const upper = await create("upper");
    owned.push(upper);
    if (!current()) return null;
    const lower = await create("lower");
    owned.push(lower);
    if (!current()) return null;
    committed = true;
    return { upper, lower };
  } catch {
    return null;
  } finally {
    if (!committed) for (const id of owned) {
      try { remove(id); } catch { /* Iframe atau shape sudah hilang. */ }
    }
  }
}

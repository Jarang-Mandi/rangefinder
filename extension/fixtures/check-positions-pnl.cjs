async (page) => {
  await page.reload();
  const inputs = page.locator('.position-card .kotak-setor input');

  if (await inputs.count() !== 2) throw Error('Tambah likuiditas harus memuat dua input');
  if (await page.locator('.range-glyph, .token-emblem').count() !== 0) {
    throw Error('Tampilan masih memuat ikon range atau logo token pengganti');
  }
  if (await page.getByText('Jurnal transaksi').count() !== 0) {
    throw Error('PnL masih memuat jurnal transaksi');
  }
  if (await page.locator('.eyebrow, details').count() !== 0) {
    throw Error('Positions atau PnL masih memuat eyebrow/detail teknis');
  }
  for (const label of ['Dibuka', 'Berjalan', 'Ditutup', 'Disetor', 'Nilai sekarang', 'Diterima', 'Fee diperoleh', 'PnL']) {
    if (await page.getByText(label, { exact: true }).count() === 0) {
      throw Error('Data kartu PnL belum lengkap: ' + label);
    }
  }
  if (await page.getByRole('heading', { name: 'ETH/PONS' }).count() !== 0) {
    throw Error('Pasangan posisi tertutup terbalik dari orientasi token kutipan');
  }
  if (!((await page.locator('.pnl-position').first().textContent()) ?? '').includes('-0.21%')) {
    throw Error('PnL persen posisi aktif tidak searah dengan PnL bersih');
  }
  for (const text of ['Versi kalibrasi', 'Impermanent loss', 'Slippage zap', 'Token id', 'Tick spacing']) {
    if (await page.getByText(text, { exact: false }).count() !== 0) {
      throw Error('Keterangan teknis masih terlihat: ' + text);
    }
  }

  await inputs.nth(0).fill('0.00029931');
  await page.waitForFunction(() => {
    const fields = document.querySelectorAll('.position-card .kotak-setor input');
    return fields[1]?.value === '1';
  });
  const dariEth = await inputs.evaluateAll((fields) => fields.map((field) => field.value));

  await inputs.nth(1).fill('2');
  await page.waitForFunction(() => {
    const fields = document.querySelectorAll('.position-card .kotak-setor input');
    return fields[0]?.value === '0.00059862';
  });
  const dariPons = await inputs.evaluateAll((fields) => fields.map((field) => field.value));

  const actions = [
    ['Rebalance', 'rebalance'],
    ['Klaim fee', 'klaim_fee'],
    ['Tutup posisi', 'tutup_posisi'],
  ];
  for (const [label, expected] of actions) {
    const button = page.getByRole('button', { name: label, exact: true });
    if (await button.count() !== 1) throw Error(label + ' harus memakai satu tombol');
    await button.click();
    if (await page.locator('.fixture-grid').getAttribute('data-started') !== expected) {
      throw Error(label + ' tidak langsung memulai alur engine');
    }
  }

  return { dariEth, dariPons };
}

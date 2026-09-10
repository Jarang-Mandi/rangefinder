async (page) => {
  await page.reload();
  const state = async () => JSON.parse(await page.locator('output').textContent());
  const toggle = page.getByRole('button', { name: 'Zap in' });

  for (const width of [320, 360, 480]) {
    await page.setViewportSize({ width, height: 700 });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
    if (overflow) throw Error('Scroll horizontal pada ' + width);
  }

  if (await page.locator('.kotak-setor').count() !== 1) throw Error('Zap aktif harus menampilkan satu input');
  const quote = page.getByRole('spinbutton', { name: 'USDG Amount' });
  await quote.fill('7500');
  await quote.press('Enter');
  const deposit = await state();
  if (deposit.side !== 'quote' || deposit.amount !== '7500' || deposit.zap !== true) {
    throw Error('Jumlah zap tidak diteruskan dengan benar');
  }

  await page.getByRole('button', { name: 'Ganti aset setoran dari USDG' }).click();
  if (!await page.getByRole('spinbutton', { name: 'PONS Amount' }).isVisible()) {
    throw Error('Pemilih aset tidak mengganti sisi setoran');
  }

  await toggle.click();
  if (await page.locator('.kotak-setor').count() !== 2) throw Error('Zap mati harus menampilkan dua input');

  return { widths: [320, 360, 480], zapInputs: 1, manualInputs: 2, deposit };
}

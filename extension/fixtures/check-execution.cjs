async (page) => {
  await page.reload();
  const state = async () => JSON.parse(await page.locator('output').textContent());
  const toggle = page.getByRole('button', { name: 'Zap in' });

  if (await page.locator('.range-glyph').count() !== 0) throw Error('Header masih memuat ikon range');
  if (await page.locator('.token-emblem').count() !== 0) throw Error('Token masih memakai logo pengganti');
  if (await page.getByRole('button', { name: 'Susun rencana' }).count() !== 0) {
    throw Error('Setoran masih meminta langkah Susun rencana');
  }

  for (const width of [320, 360, 480]) {
    await page.setViewportSize({ width, height: 700 });
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
    if (overflow) throw Error('Scroll horizontal pada ' + width);
  }

  if (await page.locator('.kotak-setor').count() !== 1) throw Error('Zap aktif harus menampilkan satu input');
  const quote = page.getByRole('spinbutton', { name: 'USDG Amount' });
  await quote.fill('7500');
  await page.waitForFunction(() => {
    const value = document.querySelector('output')?.textContent ?? 'null';
    const deposit = JSON.parse(value);
    return deposit?.side === 'quote' && deposit?.amount === '7500' && deposit?.zap === true;
  });
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

  const meme = page.getByRole('spinbutton', { name: 'PONS Amount' });
  const quoteManual = page.getByRole('spinbutton', { name: 'USDG Amount' });
  await meme.fill('1');
  await page.waitForFunction(() => {
    const fields = [...document.querySelectorAll('.kotak-setor input')];
    return fields.some(field => field.value === '0.0000042');
  });
  const dariMeme = [await meme.inputValue(), await quoteManual.inputValue()];

  await quoteManual.fill('0.0000084');
  await page.waitForFunction(() => {
    const fields = [...document.querySelectorAll('.kotak-setor input')];
    return fields.some(field => field.value === '2');
  });
  const dariQuote = [await meme.inputValue(), await quoteManual.inputValue()];

  return { widths: [320, 360, 480], zapInputs: 1, manualInputs: 2, deposit, dariMeme, dariQuote };
}

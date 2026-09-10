async (page) => {
  await page.reload();
  await page.setViewportSize({ width: 320, height: 740 });
  const output = page.getByRole('status', { name: 'Hasil interaksi' });
  const result = async () => JSON.parse(await output.textContent());
  const lower = page.getByRole('slider', { name: 'Batas bawah range' });
  const box = await lower.boundingBox();
  await page.mouse.move(box.x + box.width - 12, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width - 12, box.y + box.height / 2 - 30);
  await page.mouse.up();
  await page.waitForFunction(() => JSON.parse(document.querySelector('output').textContent).commits === 1);
  const before = await result();
  if (before.commits !== 1 || before.bounds[0] <= .9) throw Error('Drag tidak tersimpan');
  await lower.focus();
  await lower.press('ArrowUp');
  await page.waitForFunction(() => JSON.parse(document.querySelector('output').textContent).commits === 2);
  const after = await result();
  if (after.commits !== 2 || Math.abs(after.bounds[0] / before.bounds[0] - 1.0001 ** 60) > 1e-12) throw Error('Keyboard tidak mengubah satu spacing');
  await page.getByRole('button', { name: 'Reset zoom histogram' }).click();
  if (await page.getByRole('button', { name: 'Reset zoom histogram' }).textContent() !== '1×') throw Error('Reset zoom gagal');
  for (let i = 0; i < 3; i++) await page.getByRole('button', { name: 'Perbesar histogram' }).click();
  if (!await page.getByRole('button', { name: 'Perbesar histogram' }).isDisabled()) throw Error('Zoom melampaui batas');
  await page.getByRole('button', { name: 'Reset zoom histogram' }).click();
  for (const width of [320, 360, 480]) {
    await page.setViewportSize({ width, height: 740 });
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw Error('Scroll horizontal pada ' + width);
    await page.screenshot({ path: `output/playwright/histogram-${width}.png` });
  }
  console.log(JSON.stringify({ before, after, widths: [320, 360, 480], zoom: '1x sampai 8x, reset benar' }));
}

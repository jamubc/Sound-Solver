import { expect, test } from '@playwright/test';
import { mockBackend } from './mock';

test.beforeEach(async ({ page }) => {
  await mockBackend(page);
  await page.goto('/');
  await expect(page.getByTestId('project-name')).toHaveText('W205 C300 stock exhaust (approximate)');
});

test('opens the reference project with its layout, preview and provenance', async ({ page }) => {
  await expect(page.getByTestId('elements')).toContainText('muffler');
  await expect(page.getByTestId('viewport').locator('canvas')).toBeVisible();
  const plots = page.getByTestId('plot');
  await expect(plots.first().locator('canvas')).toBeVisible();
  // Every plot that draws numbers states where they come from.
  for (const plot of await plots.all()) {
    if (await plot.locator('canvas').count()) {
      await expect(plot.getByTestId('provenance')).not.toBeEmpty();
      await expect(plot.getByTestId('provenance')).not.toContainText('no provenance');
    }
  }
  await expect(plots.first().getByTestId('provenance')).toContainText('four-pole preview');
  await expect(page.getByTestId('drone-peak').first()).toContainText('rpm');
  await expect(page.getByTestId('interior-unavailable')).toBeVisible();
  await page.screenshot({ path: 'test-results/reference.png' });
});

test('element properties come from its manifest', async ({ page }) => {
  await page.getByTestId('elements').getByText('muffler').click();
  const properties = page.getByTestId('properties');
  await expect(properties).toContainText('Shell inner diameter');
  await expect(properties).toContainText('40–600 mm');
});

test('a time-domain sweep streams in and is labelled as the reference', async ({ page }) => {
  await page.getByRole('button', { name: 'Solve time domain' }).click();
  const firing = page.getByTestId('plot').first().getByTestId('provenance');
  await expect(firing).toContainText('time domain');
  await expect(firing).toContainText('3/3 converged');
  await expect(page.getByTestId('drone')).toContainText('time domain');
  await page.screenshot({ path: 'test-results/time-domain.png' });
});

test('fabrication lists the parts and takes reference points picked on a scan', async ({ page }) => {
  await page.getByRole('tab', { name: 'Fabrication' }).click();
  await expect(page.getByTestId('parts')).toContainText('rear-pipe/B1');
  // The reference project is not saved: there is nowhere to write the package.
  await expect(page.getByRole('button', { name: 'Export package' })).toBeDisabled();
  await page.getByRole('button', { name: 'Load scan…' }).click();
  await expect(page.getByTestId('scan')).toContainText('2 triangles');
  await expect(page.getByTestId('clearance-unavailable')).toContainText('in a line');
  const first = page.getByTestId('reference-point').first();
  await first.getByRole('button', { name: 'Pick on scan' }).click();
  await page.getByTestId('viewport').locator('canvas').click();
  // On the scan's plane, in scan units.
  await expect(first).toContainText(/-?\d+\.\d{3}, -?\d+\.\d{3}, 0\.200/);
  await page.screenshot({ path: 'test-results/fabrication.png' });
});

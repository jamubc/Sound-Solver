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

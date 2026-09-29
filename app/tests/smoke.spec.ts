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
  // Shown and entered in inches, held in millimetres.
  await page.getByTestId('units').click();
  await expect(properties).toContainText('1.575–23.622 in');
  const diameter = properties.locator('input[max="23.622"]');
  await expect(diameter).toHaveValue('7.874');
  await diameter.fill('8');
  await diameter.press('Tab');
  await page.getByTestId('units').click();
  await expect(properties.locator('input[max="600"]')).toHaveValue('203.2');
  await page.getByTestId('undo').click();
  await expect(properties.locator('input[max="600"]')).toHaveValue('200');
  await page.getByTestId('redo').click();
  await expect(properties.locator('input[max="600"]')).toHaveValue('203.2');
});

test('a time-domain sweep streams in and is labelled as the reference', async ({ page }) => {
  await page.getByRole('tab', { name: 'Simulate' }).click();
  await page.getByRole('button', { name: 'Solve time domain' }).click();
  await expect(page.getByTestId('sweep-result')).toContainText('Sweep complete');
  const firing = page.getByTestId('plot').first().getByTestId('provenance');
  await expect(firing).toContainText('time domain');
  await expect(firing).toContainText('3/3 converged');
  await expect(page.getByTestId('drone')).toContainText('time domain');
  await expect(page.getByTestId('status')).toContainText('3 speeds, 3 converged');
  await page.screenshot({ path: 'test-results/time-domain.png' });
});

test('the prediction is heard, steady or as a run-up', async ({ page }) => {
  await page.getByRole('tab', { name: 'Listen' }).click();
  const listen = page.getByTestId('listen');
  await listen.getByTestId('play').click();
  await expect(listen.getByTestId('level-current')).toContainText('dB re 20 µPa');
  await expect(listen).toContainText('playing current');
  await listen.getByTestId('play').click();
  await expect(listen).toContainText('stopped');
  await page.screenshot({ path: 'test-results/listen.png' });
});

test('a render is heard with every band labelled resolved or unresolved', async ({ page }) => {
  await page.getByRole('tab', { name: 'Listen' }).click();
  const listen = page.getByTestId('listen');
  await listen.getByTestId('source-render').check();
  await listen.getByTestId('play').click();
  await expect(listen.getByTestId('level-current')).toContainText('time-domain render');
  const bands = listen.getByTestId('bands');
  await expect(bands.locator('span')).toHaveCount(34);
  await expect(bands.locator('span.resolved').first()).toBeVisible();
  await expect(listen).toContainText('cross-modes cut on');
  await expect(listen.getByTestId('render-inputs')).toContainText('estimated inputs:');
  await listen.getByTestId('play').click();
  await expect(listen).toContainText('stopped');
  await page.screenshot({ path: 'test-results/render.png' });
});

test('a pinned baseline is compared order by order and overlaid', async ({ page }) => {
  await page.getByRole('button', { name: 'Pin as baseline' }).click();
  await expect(page.getByTestId('differences').locator('tbody tr')).toHaveCount(8);
  await expect(page.getByTestId('plot').first().getByTestId('provenance')).toContainText('baseline');
});

test('recordings give measured order tracks and the cabin transfer function', async ({ page }) => {
  await page.getByRole('tab', { name: 'Measure', exact: true }).click();
  const measurements = page.getByTestId('measurements');
  await measurements.getByRole('button', { name: 'Add recording…' }).click();
  await measurements.getByRole('button', { name: 'Add recording…' }).click();
  const recording = page.getByTestId('recording');
  await expect(recording).toHaveCount(2);
  await expect(recording.first().getByTestId('provenance')).toContainText('rpm estimated from the recording');
  await expect(recording.first().getByTestId('measured-drone')).toContainText('2603 rpm');
  await recording.nth(1).locator('select').selectOption('interior');
  await page.getByTestId('cabin-tf').getByRole('button', { name: 'Measure' }).click();
  await expect(page.getByTestId('cabin-tf').getByTestId('provenance')).toContainText('order ratio');
  await page.screenshot({ path: 'test-results/measurements.png' });
});

test('the inputs are swept and ranked by what they do to the sound', async ({ page }) => {
  await page.getByRole('tab', { name: 'Accuracy' }).click();
  const accuracy = page.getByTestId('accuracy');
  await accuracy.getByTestId('run-sensitivity').click();
  await expect(accuracy.getByTestId('combined')).toContainText('±');
  await expect(accuracy.getByTestId('improve')).toContainText('system.elements.muffler');
  await page.getByRole('tab', { name: 'Listen' }).click();
  await page.getByTestId('source-render').check();
  await page.getByTestId('play').click();
  await expect(page.getByTestId('uncertainty')).toContainText('uncertainty from the inputs at 2100 rpm');
  await page.screenshot({ path: 'test-results/accuracy.png' });
});

test('the benchmarks run on demand and verify each subsystem', async ({ page }) => {
  await page.getByRole('tab', { name: 'Measure', exact: true }).click();
  const verification = page.getByTestId('verification');
  await verification.getByRole('button', { name: 'Run the benchmarks' }).click();
  await expect(verification).toContainText('verified: 1 checks pass');
  await expect(verification).toContainText('not modelled');
});

test('fabrication lists the parts and takes reference points picked on a scan', async ({ page }) => {
  await page.getByRole('tab', { name: 'Fabricate', exact: true }).click();
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

test.describe('with a stub on the mid-pipe', () => {
  test.beforeEach(async ({ page }) => {
    await mockBackend(page, 'project-stub');
    await page.goto('/');
  });

  test('a stub is tuned onto the drone with its thermal band', async ({ page }) => {
    await page.getByTestId('elements').getByText('stub', { exact: true }).click();
    const tune = page.getByTestId('tune');
    await tune.getByRole('button', { name: 'Tune' }).click();
    const result = page.getByTestId('tuning');
    await expect(result).toContainText('Length 1189 mm');
    await expect(result).toContainText('1189–2055 mm');
    await result.getByRole('button', { name: 'Apply 1189 mm' }).click();
    await expect(page.getByTestId('properties').locator('input[max="3000"]')).toHaveValue('1189');
    await page.screenshot({ path: 'test-results/tune.png' });
  });
});

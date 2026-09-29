// Fixtures for the browser smoke test: the real core's answers for the reference project,
// from exhaustctl. Run from app/.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';

const project = 'tests/cases/w205_stock.json';
const out = (name) => `app/tests/fixtures/${name}.json`;
const cli = (...args) =>
  execFileSync('cargo', ['run', '--release', '-q', '-p', 'exhaustctl', '--', ...args], {
    cwd: '..',
    stdio: ['ignore', 'ignore', 'inherit'],
  });
const cliJson = (name, ...args) =>
  writeFileSync(
    `tests/fixtures/${name}.json`,
    execFileSync('cargo', ['run', '--release', '-q', '-p', 'exhaustctl', '--', ...args], {
      cwd: '..',
      stdio: ['ignore', 'pipe', 'inherit'],
    }),
  );

mkdirSync('tests/fixtures', { recursive: true });
copyFileSync(`../${project}`, 'tests/fixtures/project.json');
cli('export', 'layout', project, '-o', out('layout'));
cli('export', 'fabrication', project, '-o', out('fabrication'));
cli('export', 'manifests', '-o', out('manifests'));
cli('solve', project, '--solver', 'four-pole', '-o', out('preview'));
cli('solve', project, '--sweep', '2000:3000:500', '-o', out('solve'));

// The reference project with a stub on the mid-pipe, to tune and to compare against.
const stub = JSON.parse(readFileSync(`../${project}`, 'utf8'));
const { elements, routes } = stub.system;
elements.splice(4, 0, {
  id: 'stub', type: 'quarter_wave_stub', position_mm: [-1000, 0, -420], axis: [-1, 0, 0],
  id_mm: 50, wall_mm: 1.5, length_mm: 500, material: '409', direction: [0, 0, -1],
});
const mid = routes.find((r) => r.id === 'mid-pipe');
routes.splice(routes.indexOf(mid) + 1, 0, { ...mid, id: 'mid-pipe-2', from: { element: 'stub', port: 'out' } });
mid.to = { element: 'stub', port: 'in' };
writeFileSync('tests/fixtures/project-stub.json', JSON.stringify(stub, null, 1));
const stubProject = 'app/tests/fixtures/project-stub.json';
cli('export', 'layout', stubProject, '-o', out('layout-stub'));
cli('solve', stubProject, '--solver', 'four-pole', '-o', out('preview-stub'));
cliJson('tune', 'tune', stubProject, '--element', 'stub', '--rpm', '2100');
cliJson('compare', 'compare', project, out('preview'), out('preview-stub'));

// A synthetic run-up (1200 → 4800 rpm, firing-order peak at 2600 rpm) recorded outside and
// inside (−10 dB), as 16-bit WAV, and its order tracks and cabin transfer function.
function wav(file, samples, fs) {
  const data = Buffer.alloc(44 + 2 * samples.length);
  data.write('RIFF', 0);
  data.writeUInt32LE(36 + 2 * samples.length, 4);
  data.write('WAVEfmt ', 8);
  data.writeUInt32LE(16, 16);
  data.writeUInt16LE(1, 20);
  data.writeUInt16LE(1, 22);
  data.writeUInt32LE(fs, 24);
  data.writeUInt32LE(2 * fs, 28);
  data.writeUInt16LE(2, 32);
  data.writeUInt16LE(16, 34);
  data.write('data', 36);
  data.writeUInt32LE(2 * samples.length, 40);
  samples.forEach((s, i) => data.writeInt16LE(Math.round(Math.max(-1, Math.min(1, s)) * 32767), 44 + 2 * i));
  writeFileSync(file, data);
}
function runUp(gain, fs = 8000) {
  const phase = new Array(8).fill(0);
  return Array.from({ length: 36 * fs }, (_, i) => {
    const rpm = 1200 + (100 * i) / fs;
    const x = (rpm - 2600) / 150;
    return phase.reduce((sum, _, k) => {
      phase[k] += (2 * Math.PI * (k + 1) * rpm) / 60 / fs;
      return sum + gain * 0.01 * (k === 1 ? 10 / Math.sqrt(1 + x * x) : 1) * Math.sin(phase[k]);
    }, 0);
  });
}
wav('tests/fixtures/exterior.wav', runUp(1), 8000);
wav('tests/fixtures/interior.wav', runUp(0.316), 8000);
const measured = JSON.parse(readFileSync(`../${project}`, 'utf8'));
measured.measurements = {
  recordings: [
    { path: 'exterior.wav', position: 'exterior' },
    { path: 'interior.wav', position: 'interior' },
  ],
};
writeFileSync('tests/fixtures/project-measured.json', JSON.stringify(measured, null, 1));
cliJson('tracks', 'tracks', 'app/tests/fixtures/project-measured.json', '--recording', '0');
cliJson('cabin-tf', 'cabin-tf', 'app/tests/fixtures/project-measured.json', '--orders', '0', '1');

// Fixtures for the browser smoke test: the real core's answers for the reference project,
// from exhaustctl. Run from app/.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';

const project = 'tests/cases/w205_stock.json';
const out = (name) => `app/tests/fixtures/${name}.json`;
const cli = (...args) =>
  execFileSync('cargo', ['run', '--release', '-q', '-p', 'exhaustctl', '--', ...args], {
    cwd: '..',
    stdio: ['ignore', 'ignore', 'inherit'],
  });

mkdirSync('tests/fixtures', { recursive: true });
copyFileSync(`../${project}`, 'tests/fixtures/project.json');
cli('export', 'layout', project, '-o', out('layout'));
cli('export', 'manifests', '-o', out('manifests'));
cli('solve', project, '--solver', 'four-pole', '-o', out('preview'));
cli('solve', project, '--sweep', '2000:3000:500', '-o', out('solve'));

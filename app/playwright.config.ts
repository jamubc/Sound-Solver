import { defineConfig, devices } from '@playwright/test';

// The app's web build in WebKit (the engine of Tauri's macOS webview), with the Rust backend
// replaced by the core's recorded answers (tests/mock.ts, `npm run fixtures`).
export default defineConfig({
  testDir: 'tests',
  webServer: {
    command: 'npm run dev',
    url: 'http://localhost:1420',
    reuseExistingServer: !process.env.CI,
  },
  use: { baseURL: 'http://localhost:1420' },
  projects: [{ name: 'webkit', use: { ...devices['Desktop Safari'], viewport: { width: 1440, height: 900 } } }],
});

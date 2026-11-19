/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {defineConfig} from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// The standalone bundle: a static site that any server can host, as long as
// it also answers the `/api/*` routes in `src/standalone/localBackend.ts`.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
  },
  // Workers are bundled as ES modules so they can share chunks with the page.
  worker: {format: 'es'},
  server: {
    // `yarn dev` against a running `buck2 log trailcam` on its default port.
    proxy: {'/api': process.env.TRAILCAM_API ?? 'http://127.0.0.1:44100'},
  },
});

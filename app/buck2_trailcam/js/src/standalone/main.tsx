/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {StrictMode} from 'react';
import {createRoot} from 'react-dom/client';
import '../styles/standalone.css';
import {applyStoredTheme} from '../lib/theme';
import StandaloneApp from './StandaloneApp';

// Before the first paint, so a dark-theme user does not see a light flash.
applyStoredTheme();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <StandaloneApp />
  </StrictMode>,
);

/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import { isInternal } from 'docusaurus-plugin-internaldocs-fb/internal';

const baseRedirects = [
    {
      to: '/docs/about/why',
      from: '/docs/why',
    },
    {
      to: '/docs/about/benefits/compared_to_buck1',
      from: '/docs/benefits',
    },
    {
      to: '/docs/about/bootstrapping',
      from: '/docs/bootstrapping',
    },
    {
      to: '/docs/prelude/rules',
      from: '/docs/api/rules',
    },
    {
      to: '/docs/prelude/rules',
      from: '/docs/prelude/globals',
    }
  ];

// Redirects that need to be introduced following changes to the generated API docs in D61778036
const globalsBasedApiDocs = [
  {
    from: '/docs/api/build/actions',
    to: '/docs/api/build/AnalysisActions',
  },
  {
    from: ['/docs/api/build/anon_target', '/docs/api/bxl/anon_target'],
    to: '/docs/api/build/AnonTarget',
  },
  {
    from: ['/docs/api/build/anon_targets', '/docs/api/bxl/anon_targets'],
    to: '/docs/api/build/AnonTargets',
  },
  {
    from: ['/docs/api/build/artifact', '/docs/api/bxl/artifact'],
    to: '/docs/api/build/Artifact',
  },
  {
    from: ['/docs/api/build/buck_regex', '/docs/api/bxl/buck_regex', '/docs/api/build/regex'],
    to: '/docs/api/build/BuckRegex',
  },
  {
    from: ['/docs/api/build/configured_target_label', '/docs/api/bxl/configured_target_label'],
    to: '/docs/api/build/ConfiguredTargetLabel',
  },
  {
    from: ['/docs/api/build/context', '/docs/api/bxl/context'],
    to: '/docs/api/build/AnalysisContext',
  },
  {
    from: ['/docs/api/build/dependency', '/docs/api/bxl/dependency'],
    to: '/docs/api/build/Dependency',
  },
  {
    from: '/docs/api/build/globals',
    to: '/docs/api/build',
  },
  {
    from: ['/docs/api/build/label', '/docs/api/bxl/label'],
    to: '/docs/api/build/Label',
  },
  {
    from: ['/docs/api/build/promise', '/docs/api/bxl/promise'],
    to: '/docs/api/build/Promise',
  },
  {
    from: ['/docs/api/build/provider_collection', '/docs/api/bxl/provider_collection'],
    to: '/docs/api/build/ProviderCollection',
  },
  {
    from: ['/docs/api/build/providers_label', '/docs/api/bxl/providers_label'],
    to: '/docs/api/build/ProvidersLabel',
  },
  {
    from: ['/docs/api/build/target_label', '/docs/api/bxl/target_label'],
    to: '/docs/api/build/TargetLabel',
  },
  {
    to: '/docs/api/bxl/Actions',
    from: '/docs/api/bxl/actions',
  },
  {
    from: '/docs/api/bxl/bxl.ActionQueryNode',
    to: '/docs/api/bxl/ActionQueryNode',
  },
  {
    from: '/docs/api/bxl/bxl.Actions',
    to: '/docs/api/bxl/Actions',
  },
  {
    from: '/docs/api/bxl/bxl.AnalysisResult',
    to: '/docs/api/bxl/AnalysisResult',
  },
  {
    from: '/docs/api/bxl/bxl.AqueryContext',
    to: '/docs/api/bxl/AqueryContext',
  },
  {
    from: '/docs/api/bxl/bxl.AuditContext',
    to: '/docs/api/bxl/AuditContext',
  },
  {
    from: '/docs/api/bxl/bxl.BuildResult',
    to: '/docs/api/bxl/BuildResult',
  },
  {
    from: '/docs/api/bxl/bxl.ConfiguredTargetNode',
    to: '/docs/api/bxl/ConfiguredTargetNode',
  },
  {
    from: '/docs/api/bxl/bxl.Context',
    to: '/docs/api/bxl/Context',
  },
  {
    from: '/docs/api/bxl/bxl.CqueryContext',
    to: '/docs/api/bxl/CqueryContext',
  },
  {
    from: '/docs/api/bxl/bxl.EnsuredArtifact',
    to: '/docs/api/bxl/EnsuredArtifact',
  },
  {
    from: '/docs/api/bxl/bxl.Error',
    to: '/docs/api/bxl/Error',
  },
  {
    from: '/docs/api/bxl/bxl.FileNode',
    to: '/docs/api/bxl/FileNode',
  },
  {
    from: '/docs/api/bxl/bxl.Filesystem',
    to: '/docs/api/bxl/Filesystem',
  },
  {
    from: '/docs/api/bxl/bxl.LazyContext',
    to: '/docs/api/bxl/LazyContext',
  },
  {
    from: '/docs/api/bxl/bxl.Lazy',
    to: '/docs/api/bxl/Lazy',
  },
  {
    from: '/docs/api/bxl/bxl.LazyResolvedAttrs',
    to: '/docs/api/bxl/LazyResolvedAttrs',
  },
  {
    from: '/docs/api/bxl/bxl.OutputStream',
    to: '/docs/api/bxl/OutputStream',
  },
  {
    from: '/docs/api/bxl/bxl.Result',
    to: '/docs/api/bxl/Result',
  },
  {
    from: '/docs/api/bxl/bxl.TargetUniverse',
    to: '/docs/api/bxl/TargetUniverse',
  },
  {
    from: '/docs/api/bxl/bxl.UnconfiguredTargetNode',
    to: '/docs/api/bxl/UnconfiguredTargetNode',
  },
  {
    from: '/docs/api/bxl/bxl.UqueryContext',
    to: '/docs/api/bxl/UqueryContext',
  },
  {
    from: '/docs/api/bxl/CellPath',
    to: '/docs/api/build/CellPath',
  },
  {
    from: '/docs/api/bxl/cmd_args',
    to: '/docs/api/build/cmd_args',
  },
  {
    to: '/docs/api/bxl',
    from: '/docs/api/bxl/globals',
  },
  {
    to: '/docs/api/bxl/LazyResolvedAttrs',
    from: '/docs/api/bxl/lazy_attrs',
  },
  {
    from: '/docs/api/starlark/globals',
    to: '/docs/api/starlark',
  },
  {
    from: '/docs/api/starlark/string',
    to: '/docs/api/starlark/str',
  },
];

// Internal-only redirects
const internalRedirects = !isInternal() ? [] : [
      {
        to: '/docs/about/knowledge_sharing',
        from: '/docs/knowledge_sharing',
      },
];

export const redirects = [...baseRedirects, ...globalsBasedApiDocs, ...internalRedirects];

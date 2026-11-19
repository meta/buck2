# buck2-trailcam

The host-independent part of Trailcam, the Buck2 build invocation viewer:
event log decoding (streaming zstd + protobuf, web workers, IndexedDB cache)
and the React components that render an invocation.

Everything in here stays free of host specifics so that the same code can be
served by different hosts:

- the Nest app in the parent directory, which adds Meta auth, Manifold and
  Scuba backed API routes, and InternGraph-only panels. It compiles these
  sources directly through the `@trailcam/*` path alias in its `tsconfig.json`;
- `buck2 log trailcam`, which embeds the standalone bundle in the buck2 binary
  and serves a local event log;
- open-source deployments with their own log storage.

Concretely: no imports from outside this directory, no `@nest/*` or `next/*`
imports, no hard-coded URLs. Data comes through `TrailcamBackend`
(`src/backend.ts`); host-only UI comes through the slots on `InvocationView`.
`src/index.ts` is the surface hosts build on.

## Standalone bundle

`src/standalone/` is the entry point for hosts that serve a static bundle. It
expects the two routes documented in `src/standalone/localBackend.ts`.

```
yarn install          # see below for the offline mirror inside fbsource
yarn build            # dist/
yarn check            # tsc
yarn test             # vitest
yarn dev              # dev server, proxying /api to a running buck2 log trailcam
```

Inside fbsource the package manager is yarn classic and installs must come
from the offline mirror, which `nest/.yarnrc` turns off for everything under
`nest/`. Point yarn at it explicitly:

```
YARN_YARN_OFFLINE_MIRROR=$(sl root)/xplat/third-party/yarn/offline-mirror \
  $(sl root)/xplat/third-party/yarn/yarn install --offline
```

Adding a dependency means adding its tarball to that mirror; the
`resolutions` in `package.json` keep transitive versions on tarballs that are
already there. The lockfile's resolved URLs point at the public registry so
the directory installs unchanged outside fbsource.

The Buck build of the bundle is `fbcode//buck2/app/buck2_trailcam:bundle_tar`,
next to its consumer; the parent directory's BUCK file only exposes these
sources and the lockfile. There is no BUCK file in here on purpose: it would
make `core/` a separate Buck package and hide it from the Nest app's source
glob.

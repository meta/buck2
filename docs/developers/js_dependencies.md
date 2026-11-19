---
oncalls: ['build_infra']
---

# JS Dependencies

buck2 contains six independent JS packages. They do not share a package manager, a manifest key,
or a lockfile format, so a dependency bump — typically a transitive CVE — has to be applied per
package rather than once.

## The packages

Paths are relative to `fbcode/buck2`.

| Package | Pin with | Lockfile |
|---|---|---|
| `website` | `overrides` **and** `resolutions` | `package-lock.json` **and** `yarn.lock` |
| `explorer` | `overrides` | `package-lock.json` |
| `starlark-rust/vscode` | `overrides` | `package-lock.json` |
| `starlark-rust/vscode/client` | `overrides` | `package-lock.json` |
| `app/buck2_explain/js` | `resolutions` | `yarn.lock` |
| `app/buck2_explain/output_format_js` | `resolutions` | `yarn.lock` |

`website` is both an npm and a yarn package. Its two lockfiles are independent and do drift — they
have held different resolved versions of the same dependency. Add the floor to both keys, regenerate
both lockfiles, and check both afterwards; updating one leaves the other silently unpatched.

## Pinning a transitive dependency

Add a floor to the manifest key, then regenerate. Prefer a range every member of which is fixed,
e.g. `">=1.1.18 <2.0.0"` when an advisory is patched separately on the 1.x and 5.x lines — a bare
`">=1.1.18"` is also satisfied by unpatched 2.x and 3.x releases.

```bash
# npm packages
npm install --package-lock-only --ignore-scripts
# yarn packages
yarn install --ignore-scripts --ignore-engines
```

Run each in a scratch directory holding only that package's manifest and lockfile, then copy the
lockfile back. That keeps `node_modules` out of the repo, and for `website` it is required — see
below.

## Two major lines of the same package

A flat floor cannot patch a package that appears twice on different majors. `website` carries
`js-yaml` on 3.x through `gray-matter` and on 4.x through `cosmiconfig` and three `@docusaurus/*`
packages, and each line has its own patched release. One range wide enough for both admits the
unpatched versions of the lower line; one narrow enough for the upper line forces the lower consumer
onto a major it never asked for. Scope the floor to the parent instead.

npm nests the parent inside `overrides`, and the more specific entry wins:

```json
"overrides": {
  "gray-matter": { "js-yaml": ">=3.15.1 <4.0.0" },
  "js-yaml": ">=4.3.1 <5.0.0"
}
```

yarn matches `resolutions` patterns against the path from the root, so a parent that is itself
transitive needs a `**/` prefix — and so does the general rule, or it outranks the scoped entry:

```json
"resolutions": {
  "**/gray-matter/js-yaml": ">=3.15.1 <4.0.0",
  "**/js-yaml": ">=4.3.1 <5.0.0"
}
```

Both ways of getting this wrong are quiet:

- Writing only the flat floor. yarn hoists 4.3.1 over `gray-matter`'s `^3.13.1` and installs no
  nested copy, so `gray-matter` throws `Function yaml.safeLoad is removed in js-yaml 4` on the first
  front-matter parse — every page of the site.
- Dropping the `**/` from the scoped entry. `"gray-matter/js-yaml"` matches nothing, and yarn
  ignores it while still writing a lockfile entry at the version you asked for. Both copies stay
  unpatched behind a lockfile that greps clean.

## Never run npm where a `yarn.lock` is present

`npm install` rewrites a co-located `yarn.lock` into npm's own format: quoted keys,
`registry.npmjs.org` URLs, no `#sha1` suffixes. `--package-lock-only` does it too. In `website` that
converts all 1528 entries, and npm reports nothing. Running yarn afterwards partly normalises the
file again, leaving a plausible-looking ~5000 line diff that is easy to mistake for a real update.

`website` is the only package holding both lockfiles, so it is the only one where this can happen —
which is why it needs a separate scratch directory per tool, not one shared directory.

A correct single-dependency bump touches a handful of lines. If the diff is thousands of lines wide,
discard it and regenerate; do not try to repair it. This has reached the repo before — a stray
`is-alphabetical@^2.0.0` entry sat in `website/yarn.lock` in npm's format for some time before a
later regeneration normalised it.

## Checking the result

A package can appear several times in one tree, so read every entry, not the first.

```bash
# npm — all resolved copies of a package
python3 -c "import json,sys; d=json.load(open('package-lock.json')); \
print(sorted({m.get('version') for p, m in d['packages'].items() \
if p.split('node_modules/')[-1] == sys.argv[1]}))" <package>
# yarn — the entry and its resolved version
grep -A1 '^"\?<package>@' yarn.lock
```

For yarn the lockfile alone is not enough, because a `resolutions` pattern that matched nothing
still writes an entry at the version you asked for. Read the installed tree as well:

```bash
yarn install --ignore-scripts --ignore-engines
node -p "require('<package>/package.json').version"
node -p "require('<parent>/node_modules/<package>/package.json').version"
```

Two checks catch most mistakes without a full build:

```bash
# the lockfile still satisfies package.json
yarn install --frozen-lockfile --ignore-scripts
# the npm lockfile is stable: re-resolving must leave it byte for byte identical
npm install --package-lock-only --ignore-scripts
```

Neither builds anything. Only CI exercises the Docusaurus build, `tsc`, and the electron package.

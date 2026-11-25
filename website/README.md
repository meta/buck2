# Website

This website is built using [Docusaurus](https://docusaurus.io/), a modern
static website generator. Its pages are the Markdown files in `../docs`, plus
the pages that `gen_docs.py` generates from a `buck2` binary: the rule
reference, the API reference and the command reference.

## Previewing a change

You need [Node.js](https://nodejs.org/) (version 22 is known to work), `yarn`,
Python 3, and a `buck2` on your `PATH` (a
[release](https://github.com/meta/buck2/releases) binary will do).

```shell
$ cd website
$ yarn install
$ yarn generate
$ yarn start
```

`yarn generate` writes the generated pages into `../docs`, where git ignores
them. `yarn start` serves the site at http://localhost:3000 and reloads a page
when its Markdown file changes.

The generated pages show the doc comments and the prelude that the `buck2`
binary was built from. To preview a change to either, generate the pages with
`./gen_docs.py --cargo`, which builds `buck2` from your checkout with `cargo`.

Before sending a pull request, run `yarn docusaurus_build`. It builds the site
into `build/` and fails on a link to a page that does not exist. A new page
shows up in the navigation once it is listed in `sidebars.ts`, or if it is in a
directory that `sidebars.ts` lists as a whole.

## Deployment

```shell
$ GIT_USER=<Your GitHub username> USE_SSH=true yarn deploy
```

If you are using GitHub pages for hosting, this command is a convenient way to
build the website and push to the `gh-pages` branch.

## Fixing GitHub Security Alerts

This package carries both `package-lock.json` and `yarn.lock`, and a dependency
pin has to be applied to both. See
[`docs/developers/js_dependencies.md`](../docs/developers/js_dependencies.md).

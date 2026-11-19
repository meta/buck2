# Website

This website is built using [Docusaurus 2](https://docusaurus.io/), a modern
static website generator.

## Mac Setup

`yarn` and `node` are installed by default on devservers, but not on Macs. To
work on this site on Macs, add `fbsource/xplat/third-party/node/bin` and
`fbsource/third-party/yarn` to your `PATH`.

## Installation

The very first time:

```shell
$ yarn global add node-gyp
```

And if on Eden:

```shell
$ eden redirect add $PWD/node_modules bind
$ eden redirect add $PWD/build bind
```

Also on your first time, and potentially after each large rebase:

```shell
$ yarn install
```

## Building Generated Content

Again, on your first time:

```shell
$ yarn generate
```

You will need to re-run this each time you want to see changes to generated
content, primarily the API docs. You can alternatively `yarn generate_local` to
update generated content using a built-from-source buck2.

## Local Development

```shell
$ yarn start-fb
```

This command starts a local development server, and if on Mac, opens up a local
browser window.

On a devserver, the window does not open for you, but you can head to
https://devvmXX.foo.com:9094 in your browser. This requires lighthouse or VPN.

To get your changes reflected on the local server:

1.  For non-generated markdown content in the `docs/` directory, reload the
    page.
2.  For generated markdown content, re-run `yarn generate` as above and reload
    the page.
3.  For other changes to the site configuration, Ctrl+C and restart
    `yarn start`. Then, hard-reload the page (Ctrl+Shift+R).

## OSS Variants

To see the external versions of the page, do:

```shell
$ yarn start
```

If on a devserver, this will require manually setting up an SSH tunnel by
running the following **from your mac**:

```shell
ssh -L 3000:localhost:3000 $DEVSERVER
```

In all cases, you'll need to be either on lighthouse or VPN for this to work.

## Prod Build

You can perform a production build via `yarn build` or `yarn build-fb`. This
generates a static site into `build/`. This site can be served via any static
site viewer - docusaurus has one built in that you can run via `yarn serve`. As
of Oct 2024 this only works on Macs, not on devservers.

Iterating on the prod build is slower than on the local `yarn start` server.

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

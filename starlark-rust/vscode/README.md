# Starlark VS Code LSP extension

A VSCode LSP extension that talks over stdin/stdout to a binary. This can either
be the starlark binary itself, or any binary that has implemented
`starlark::lsp::server::LspContext` and runs
`starlark::lsp::server::stdio_server()`.

If using another binary, the settings to be aware of are `starlark.lspPath` (the
binary path) and `starlark.lspArguments` (the arguments to that binary). These
are available in the VSCode extension settings UI.

Based on a combination of:

- Tutorial at
  https://code.visualstudio.com/api/language-extensions/language-server-extension-guide
- Code for the tutorial at
  https://github.com/microsoft/vscode-extension-samples/tree/master/lsp-sample
- Syntax files from https://github.com/phgn0/vscode-starlark (which are the
  Microsoft Python ones with minor tweaks)

## Pre-requisites

You need Node.js 22+ and npm v7+ installed to run the extension packager.
Afterwards, run `npm install` in this folder and in `client`.

## Debugging

- Follow steps in Pre-requisites section.
- Open VS Code on this folder.
- Press Ctrl+Shift+B to compile the client and server.
- Switch to the Debug viewlet.
- Select `Launch Client` from the drop down.
- Run the launch config.

## Installing

- Follow steps in Pre-requisites section.
- Run `npm install @vscode/vsce vscode-languageclient`
- Run `npm exec @vscode/vsce package`
- In VS Code, go to Extensions, click on the "..." button in the Extensions bar,
  select "Install from VSIX" and then select the `starlark-1.0.0.vsix` file that
  was produced.
- Build the starlark binary with `cargo build --bin=starlark` and then do one
  of:
  - Put it on your `$PATH`, e.g.
    `cp $CARGO_TARGET_DIR/debug/starlark ~/.cargo/bin/starlark`.
  - Configure the setting `starlark.lspPath` for this extension to point to the
    starlark binary. e.g. `$CARGO_TARGET_DIR/debug/starlark`.

## Updating

Every few months security advisories will arrive about pinned versions of
packages. This folder and `client` are separate packages and are pinned
independently; see
[`docs/developers/js_dependencies.md`](../../docs/developers/js_dependencies.md).

Run `npm exec vsce package` afterwards to confirm everything still works.

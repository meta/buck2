Buck2 logs all the commands it runs. So, after you've run a build, you can query
Buck2 to get access to the exact command it used.

To do so, do your build as normal, then run `buck2 log what-ran`.

## What Ran output format

This will output a table showing all the commands that were executed, and how
they were executed.

The structure is as follows:

```sh
REASON  <TAB> TARGET <TAB> IDENTIFIER <TAB> EXECUTOR <TAB> REPRODUCER
```

Which should be used as follows:

- REASON - value is either `build` (for building a thing) or `test` (for running
  a test).
- TARGET - the name of the build target that declared an action.
- IDENTIFIER - depends on the target but will usually be something like a file
  name or a module.
- EXECUTOR - value is either `cache`, `re` or `local`.
- REPRODUCER - how you can re-run this yourself.

## Using the What Ran output

Use What Ran as follows:

- Start by identifying the command you're looking for:
  - You can grep the output for a given target.
  - You can then grep by identifier if necessary. For example, if you're after
    C++ compilation, try grepping for the basename of your file (for example,
    for `fbcode/my/stuff.cpp`, grep for `stuff.cpp`).
- Once you found it, reproduce as follows:
  - If the executor was `local`, the command is in the output, so just run it.
    It's expected that you'll do this from the root of your project (use
    `buck2 root --kind project` to find where that is).
  - If the executor was `re` or `cache`, you're provided a RE digest of the form
    `HASH:SIZE`.

<FbInternalOnly>
:::note
  For `re` or `cache`, run `cas download-action HASH:SIZE` to retrieve the action, then follow the instructions to run it.
:::
</FbInternalOnly>

## Examples

The following ran locally:

```bash
build  fbcode//scripts/torozco/getenv:getenv-san-conf-__generated-lib__ (archive_thin libgetenv-san-conf-__generated-lib__.pic.a)  local  fbcode/third-party-buck/platform010/build/llvm-fb/bin/llvm-ar qcsTD buck-out/v2/gen/fbcode/d839c731f5505c62/scripts/torozco/getenv/__getenv-san-conf-__generated-lib____/libgetenv-san-conf-__generated-lib__.pic.a buck-out/v2/gen/fbcode/d839c731f5505c62/scripts/torozco/getenv/__getenv-san-conf-__generated-lib____/__objects__/san-conf.c.pic.o
```

To repro, you'd run:

```bash
fbcode/third-party-buck/platform010/build/llvm-fb/bin/llvm-ar qcsTD buck-out/v2/gen/fbcode/d839c731f5505c62/scripts/torozco/getenv/__getenv-san-conf-__generated-lib____/libgetenv-san-conf-__generated-lib__.pic.a buck-out/v2/gen/fbcode/d839c731f5505c62/scripts/torozco/getenv/__getenv-san-conf-__generated-lib____/__objects__/san-conf.c.pic.
```

The following ran on RE:

```bash
build  fbcode//common/init:kill (cxx_compile Kill.cpp (pic))  re  97feca9d014155a80ec55fe27e6bb17f9d2f8574:94
```

<FbInternalOnly>
To repro, you'd run:

```bash
cas download-action 97feca9d014155a80ec55fe27e6bb17f9d2f8574:94
```

</FbInternalOnly>
<OssOnly>
Reproducing this command will depend on the particular RE implementation you use.
</OssOnly>

## Expired Digests

Note that if the action was a cache hit on RE, you might get an error when
downloading it, indicating that it's not found. If that happens, it's because
the cache entry is there but the inputs have expired.

If this happens to you, run your build with `--upload-all-actions`.

This page contains generic debugging advice for developers of Buck2; this advice is descriptive
(based on what people usually do today) not prescriptive (you're welcome to come up with your own
ideas).

## Normal logic bugs

We usually debug normal logic bugs by looking at the code, writing finer grained tests, or standard
`println!` debugging.

Use of a traditional debugger is not common, but it probably works using standard tools in OSS<FbInternalOnly>,
internally see [debuggers_internally](./debuggers_internally.fb.md) if you want to try</FbInternalOnly>.

Buck2 has many commands to retrieve information about the build, particularly `buck2 log`, `buck2
audit` and `buck2 debug`, which can be helpful.

## Running builds

`./buck2.py <command>` builds buck2 from source and runs `<command>` using that buck2. The command
is run in a different isolation dir, to prevent the command from stepping on your existing buck
daemon. This means large builds will get no cache hits and be very slow.

Alternatively, `buck2 build @fbcode//mode/opt fbcode//buck2:buck2 --out /tmp/buck2` to build buck2
on its own. Then, use `/tmp/buck2` to run builds in a *different* checkout of fbsource from the one
you're editing code in.

## Logging

buck2 emits most of its logs in a structured form that is best interacted with via `buck2 logs`
commands.

We additionally have some tracing logging, though it's sparse and not in very widespread use. Use
the `BUCK_LOG` environment variable to enable trace logging. Requires daemon restart:

```bash
buck2 kill
BUCK_LOG=module_name=trace buck2 <command>
# Example
BUCK_LOG=starlark=trace buck2 uquery cell//path/to:target
BUCK_LOG=buck2_execute_impl::materializers=trace buck2 build cell//path/to:target
```

Or use `./buck2.py` instead of `buck2` to run local changes

See
[tracing-subscriber docs](https://docs.rs/tracing-subscriber/0.2.17/tracing_subscriber/filter/struct.EnvFilter.html)
for filter syntax.

<FbInternalOnly>

### Investigating configuration transitions

If you're trying to work out where a transition happens within a dependency
chain, you may find the following script useful:

```sh
scripts/torozco/parse_deps
```

## Making a change to Buck2 Tpx

Buck2 invokes Tpx when running tests. If you're changing Tpx, you can build your
own Tpx and then have Buck2 use it, as follows:

```bash
# Build Tpx
buck2 build @fbcode//mode/opt fbcode//buck2/buck2_tpx_cli:buck2_tpx_cli --out /tmp/tpx

# Use Tpx
buck2 test -c test.v2_test_executor=/tmp/tpx
```

Alternatively, you can build buck and tpx in one go with `fbcode/buck2/buck2.py` and use it like buck:

```sh
fbcode/buck2/buck2.py test ...
```

To get access to Tpx's stderr and stdout if you are print-debugging, you need to also get Buck2 to have the right log level for it:

```sh
BUCK_LOG=buck2_test=debug buck2 test
```

Remember that you need a daemon restart to change `BUCK_LOG`.

Refer to the [tpx wiki](https://www.internalfb.com/wiki/TAE/tpx/Hacking_on_Tpx/) for more details.

</FbInternalOnly>

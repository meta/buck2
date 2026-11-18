# Build Event Service check

`check.py` runs buck2 against `fake_bes_server.py`, a Build Event Service that
records every event it receives, and checks what arrived:

- a build that succeeds, one that fails, and a test run each stream a complete
  invocation, from `started` to `build_finished` and the end of the stream,
  with the right exit code and test statuses;
- `buck2 kill --bes-drain-timeout` delivers a stream that the daemon still held
  when its command ended, to a server that reads slowly and acknowledges only
  at the end of a stream.

```sh
python3 -m venv /tmp/bes-venv
/tmp/bes-venv/bin/pip install grpcio==1.76.0 grpcio-tools==1.76.0
/tmp/bes-venv/bin/python examples/bes/check.py --buck2 <path to buck2>
```

The script generates the Python stubs from the protos under
`remote_execution/oss` and writes this project's `.buckconfig.local`, which it
removes when the checks pass. CI runs it in the `build_example_bes` action.

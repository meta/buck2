# Commands and output in action details

Execution offers Command, stdout, and stderr tabs. It shows recorded evidence
first; opening a popup or tab never sends a remote request. The command report
selector preserves the order Buck recorded. Separately observed command starts
are not extra command reports or proof of execution success.

## Recorded versus unavailable

The original event log can contain a local invocation even when the action-end
record says its command was omitted. Retrace retains local/worker command-start
observations as well as command reports, with explicit environment entries and
worker fallback executables kept separate. A fallback executable is not itself
evidence that fallback ran. Logged environment entries need not include the
entire inherited environment. Response-file arguments remain response-file
references; this viewer does not claim to expand their contents.

Command recovery requires compatible command kind/digest and unambiguous command
contents. It never pairs starts and reports by list position. Hybrid execution
and resource-control restarts can otherwise attach the wrong command or output
to a report. A remote digest placeholder is not a runnable command.

For ordinary successful build actions, Buck deliberately omits successful-command
stdout while retaining available stderr. Failed command reports can retain both.
Cancellation can leave empty buffers or no completed report. Plain protobuf string
fields do not distinguish an actually empty stream from unavailable output, so
Retrace labels that uncertainty instead of displaying a fabricated zero-byte
measurement. Buck's own oversized-output/download-error notices are retained as
notices, not interpreted as program output or automatically executed commands.
The separate test-run output policy is not equivalent to build-action retention.

## Bounded, durable storage

Native conversion writes a third artifact, `<build-id>-execution.bin`, beside
`<build-id>-trace.json` and `<build-id>-graph.json`. Preserve all three when moving
a converted capture. The original event log remains sufficient to regenerate them.
Older traces without execution details remain readable; they cannot recover data
discarded by their earlier converter without reconversion from the original log.

The execution file contains command metadata and output bytes; trace rows contain
small offsets and lengths. Ingestion spills records during ordered reduction,
not into a second full in-memory collection of command/output strings. The store
body hash participates in the trace's generation fingerprint. The reader checks
the build, capture, declared size and file identity, and refuses mismatched or
changed files. New conversions invalidate older cache receipts so that an old
cached trace does not silently hide newly supported evidence.

The popup loads bounded command metadata and reads at most 64 KiB for an output
preview. Search and Copy explicitly operate on the preview when truncated. Save
streams all stored output bytes to a new user-selected file without loading them
all into memory, and does not overwrite an existing file. Saved bytes are the
UTF-8 text Buck recorded; Buck may already have decoded original output lossily.
Conversion artifacts and newly saved output use private permissions on Unix because
commands, explicit environment entries, and logs can contain sensitive data.
The 16 MiB metadata budget limits serialized bytes read for one action, not
decoded heap size. Opening checks the stored hash descriptor against the trace
generation and pins file identity; it does not rehash the entire store body.
Invalid stored UTF-8 is refused rather than expanded into an oversized preview.

## Explicit remote lookup

These Meta-specific lookups are available only in internal builds. Open-source
builds retain all local recorded evidence and show an unavailable explanation
without launching the Meta CLI. See [integration boundaries](integrations.md).

Fetch command metadata reads Action and Command metadata only, never the input
tree. Its prose is labeled as a CAS action definition, not as recorded program
output or automatically converted into executable argv. Inspect current cached
result reads the action cache's current entry. That result can differ from the
historical attempt and stays explicitly unverified. Raw CLI prose is never
scraped to invent trusted stdout/stderr blob references.

Fetch helpers use argument vectors, validate digest/use-case tokens, support
cancellation and timeout, and bound retained stdout/tool diagnostics. Limits on
retained metadata do not bound the CLI's own allocations or the total size
of a referenced Command/cache response. CAS objects can expire or be unavailable;
the UI reports lookup failures rather than treating them as empty output.
The CLI is a debugging dependency, not a stable RE service API.

Download inputs for reproduction remains a separate, explicitly expensive action.
Live RE log-stream handles and automatic historical-worker lookup are not added
by this feature.

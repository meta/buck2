# Integration boundaries

The offline reader, dependency model, reports and native viewer are part of
the open-source Buck2 workspace. These do not require Meta services. Build
labels and file paths in a capture are user data, not dependencies on a
particular checkout layout.

| Integration | Internal build | Open-source build |
| --- | --- | --- |
| Local event-log files, local Command ID and newest-log lookup | Available | Available |
| Remote Command ID download | Explicit Meta storage lookup | Refused with a local-file alternative |
| Recorded commands/stdout/stderr | Bounded local execution-store reader | Same local reader |
| Remote command/cache-result inspection | Explicit bounded Meta CLI lookup | Unavailable; no child process launched |
| Full input-tree reproduction | Separate explicit expensive operation | Unavailable through the Meta helper |
| Analysis and replay | Offline, conditional on recorded evidence | Same calculations and limitations |

Meta storage/RE process creation is compiled only under `cfg(fbcode_build)`.
The unavailable branches retain a clear user-facing reason rather than trying
to find internal binaries on an open-source user's PATH. Tests of generic
timeout, cancellation and byte limits use local fake processes, not services.
Adding another remote backend should preserve explicit consent, provenance,
size/time limits and process cleanup; a generic shell executor is not needed.

Conversion artifacts can contain commands, paths, explicit environment values
and program output. Newly created artifacts and saved outputs use private Unix
permissions. On other platforms, the user's filesystem ACLs apply. Copying a
capture is an intentional transfer of that data; inspect it before sharing.

Internal review IDs, machine-specific benchmarks and historical test URLs live
in `.fb.md` files, which Buck2's export configuration excludes. Public docs
describe reproducible workflows rather than depending on those receipts.

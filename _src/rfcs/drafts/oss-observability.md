# Observability for Buck2 OSS

## Context

In large organizations building software is a fundamental part of a developer's
journey and can significantly impact productivity. As such, being able to
monitor and debug builds is a fundamental ability every user and power-user must
have. This ability is further limited by the remote nature of CI builds where
instances are usually remote and most of the times inaccessible due to security
requirements (see
[SLSA L2 and above](https://slsa.dev/spec/v1.0/levels#build-l2)).

Meta already seems to support an internal version of this based on thrift.

## Prior Art

### Bazel - BEP

Bazel provides multiple tools to monitor and debug various aspects of a build,
but most importantly it supports a protocol called BEP (Build Event Protocol) to
send events to a remote server. The API is tool agnostic and only provides a
method to send events of some kind. The type of the events though can be defined
on a per tool basis. Bazel comes with its own set of events that should be
almost completely applicable to Buck2 as well, given the similarities between
the two.

File uploads are performed using the same ByteStream API the RBE protocol uses.

### Open Telemetry / Prometheus

- [gitlab-ci-pipelines-exporter](https://github.com/mvisonneau/gitlab-ci-pipelines-exporter)
- [opentelemetry-gradle-plugin](https://github.com/craigatk/opentelemetry-gradle-plugin)
- [otel-export-trace-action](https://github.com/inception-health/otel-export-trace-action)
- [gitlab-honeycomb-buildevents-webhooks-sink](https://github.com/zoidyzoidzoid/gitlab-honeycomb-buildevents-webhooks-sink)
- [opentelemetry-gradle-plugin](https://github.com/craigatk/opentelemetry-gradle-plugin)

There are a variety of other build systems and CIs that instead of implementing
their own telemetry, rely on a more standardised approach using OTEL or
Prometheus. Though this is potentially a good idea in terms of maintainability,
it does not come with build debugging tooling out of the box, unlike BES.

## Buck2's existing BuckEvent

Buck2 already has a `BuckEvent` it defines. This could be transformed into a
format supported by other APIs or a new API and combined with other options
described in this document.

## Others

- [Chromium Siso's ResultStore](https://github.com/googleapis/googleapis/blob/master/google/devtools/resultstore/v2/resultstore_upload.proto)
- [Reclient's Event](https://github.com/bazelbuild/reclient/blob/ed0afeddb1b5f0d001a6c8578f000217f06534dc/internal/pkg/event/event.go#L19)

#### Tools using BEP

The biggest advantage in adopting this protocol is the number of already
existing tools already able to process it. Anecdotally there are various
internal users, especially large corporations, using BEP to create integrations
and tools of all kinds (eg:
[Salesforce's talk during 'Build Meetup 2021'](https://youtu.be/qboJOW1vZLA?si=w7uC-ZxhGtHHM_m6)).
This, combined with the already existing similarities with Bazel, could be a
driver for adoption. Furtheremore, there are some commercial and free-OSS
options available:

- [EngFlow (commercial)](https://www.engflow.com/)
- BuildBuddy:
  [limited free OSS offering](https://github.com/buildbuddy-io/buildbuddy) and
  [fully featured commercial](https://www.buildbuddy.io/)
- [Bzl and stackb's build UI (commercial)](https://bzl.io/)

## Open problems

### Sync Vs Async event stream

Sending events to a remote end may cause a build to incur delays. The most clear
of which is when a user wants to ensure that by the end of a command's
invocation all events have been successfully uploaded or the command exits
unsuccessfully. Doing this within the command itself will always add additional
time to the build. At a minimum that delay will be the time it takes to send the
last event, but in the worst case scenario this could cause the build to wait
for multiple large blobs to be uploaded. This could take several minute and have
a profound impact on performances.

Alternatively, the events could be send asynchronously. This works really well
when the instance the command is run from can be guaranteed to stay alive for as
long as it takes to upload all the events. This is typically not the case with
ephemeral CI instances which are becoming more and more common. Furthermore,
consecutive invocations that do not wait for the previous events to be sent may
incur higher network latency. Finally, mutating the file system during an
asynchronous stream of events may cause inconsistent uploads and thus incorrect
results.

Bazel implements both options. A third option could be to use async but provide
a `buck2 flush events` subcommand that waits until all the existing events have
been sent, retried if necessary and returns success or failure depending on the
outcome of the upload of events.

All these options are **NOT** mutually exclusive and do in fact combine quite
well.

### Using bazel's events or creating custom new ones?

As previously mentioned, were we to adopt BES, then there'd still be an open
question: should buck2 reuse Bazel's events or should we create new ones? There
are
[some generalization efforts](https://github.com/bazelbuild/remote-apis/issues/318)
happening at the moment, however Bazel and Buck2 share lots of similarities.
Enough that all events are almost perfectly applicable to Buck2, though less
useful at times, like with command line options. On the other hand, a new set of
events would perfectly describe a buck2 build, but would be prone to more issues
initially and would require a bigger investment upfront in terms of design and
development. Alternatively, we could start with a smaller subset of Bazel events
that are equivalent in Buck2 and then proceed to add the additional events
needed to properly express the unique behaviours of a Buck2 build.

## Proposal

Since this observability stack will be needed exclusively by the community, I
reckon adopting an already existing generic protocol would be the best option.
This way contributions on each end would benefit the other and the support and
development burden can be implicitly shared by the contributors of the protocol.
In this case, as it's probably already obvious in the document, said protocol I
am referring to is BES.

The protocol is by nature very generic and the first iteration could be
completely based on a subset of events Bazel uses that apply to Buck2:

- Phase 1: create an invocation and send logs to a remote server
- [Progress](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L291C9-L291C17)
- [Aborted](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L309)
- [BuildStarted](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L361)
- [BuildFinished](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L852C9-L852C22)
- Phase 2: Add information about the command line and what targets it expands to
- [UnstructuredCommandLine](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L406)
- [PatternExpanded](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L462)
- Phase 3: Add target level status and information
- [NamedSetOfFiles](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L539C9-L539C24)
- [TargetComplete](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L623C9-L623C23)
- [TargetConfigured](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L494C9-L494C25)
- [TargetSummary](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L843C9-L843C22)
- [TestResult](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L682)
- [TestSummary](https://github.com/bazelbuild/bazel/blob/38ad73402b213b2a623d0953500b1cfc47c0e851/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto#L781C9-L781C20)

Additionally, as part of `Phase 1` we would need to implement the client itself,
a basic streaming strategy (which to begin with could be the simple sync
approach discussed in `Sync VS Async`) and provide an invocation-id to correlate
events that may be sent to different servers into a single invocation (this is
very similar to Buck2's trace id and could in fact be the same, but we should
make it possible for the user to provide it when invoking the CLI and/or print
it visibly in the logs).

Once this is done, I suggest we look into async uploads and add a
`buck2 flush events` command to wait for all events (or those of a specific
invocation) to be flushed out.

### Approaches

There are multiple ways in which this proposal can be implemented:

1. Buck2 directly sends Bazel's BEP events and implements the BES protocol
2. Buck2 implements the BES protocol but sends BuckEvent events but a shim can
   be built inside Buck2 to turn these events into Bazel's BEP events
3. Same as above but the shim lives outside of Buck2, so as far as Buck2 is
   concerned it sends Buck2 BuckEvent via BES
4. Same as 2 but we implement a new API that is BES-like, but not BES
5. Same as 3 but we implement a new API that is BES-like, but not BES

My current preference is for approach 1 as it is the simplest, but alternatively
my second favourite option is 3.

## Links

- [BEP explanation](https://bazel.build/remote/bep)
- [Bazel's BEP events and glossary](https://bazel.build/remote/bep-glossary)
- [BES proto definition](https://github.com/googleapis/googleapis/blob/master/google/devtools/build/v1/publish_build_event.proto)
- [Bazel's events proto definition](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto)

## Glossary

- BES (Build Event Service): refers to the generic service used to send/receive
  build events by Bazel
- BEP (Build Event Protocol): refers to the combination of BES and Bazel
  specific events
- Invocation: a single execution of a Buck2/Bazel command. An invocation usually
  contains a single build or execution of a command, but may contain many in
  case of multiple retries. Initially, build and invocations overlap

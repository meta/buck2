# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""A Build Event Service that records what buck2 sends it.

Every request on PublishBuildToolEventStream and PublishLifecycleEvent becomes
one JSON line in the record file, and the end of each stream another, so a
check can read back what reached the server and what was acknowledged.

With `--ack-at-eof` the server acknowledges nothing until the client closes the
stream, as BuildBuddy does. A client that exits before closing its streams then
loses them. With `--read-delay` the server reads each event late, so a command
can end while the daemon still holds events for it. `buck2 kill
--bes-drain-timeout` exists to deliver those before the daemon exits.

The stubs come from `grpc_tools.protoc` over the protos under
`remote_execution/oss`; `--stubs` names the directory they were generated into.
"""

import argparse
import json
import os
import sys
import threading
import time
from concurrent import futures


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stubs", required=True)
    parser.add_argument("--record", required=True)
    parser.add_argument("--port-file", required=True)
    parser.add_argument("--ack-at-eof", action="store_true")
    parser.add_argument(
        "--read-delay",
        type=float,
        default=0.0,
        help="seconds to wait before reading each event, as a slow server would",
    )
    args = parser.parse_args()

    sys.path.insert(0, args.stubs)
    import grpc
    import publish_build_event_pb2
    import publish_build_event_pb2_grpc
    from google.protobuf import empty_pb2
    from proto import build_event_stream_pb2

    lock = threading.Lock()
    record = open(args.record, "a", buffering=1)

    def write(entry: dict) -> None:
        entry["time"] = time.time()
        with lock:
            record.write(json.dumps(entry, sort_keys=True) + "\n")

    def stream_fields(stream_id) -> dict:
        return {
            "build_id": stream_id.build_id,
            "invocation_id": stream_id.invocation_id,
            "component": stream_id.component,
        }

    def bazel_fields(any_event) -> dict:
        event = build_event_stream_pb2.BuildEvent()
        if not any_event.Unpack(event):
            return {"bazel_type": any_event.type_url}
        fields = {
            "bazel_id": event.id.WhichOneof("id"),
            "bazel_payload": event.WhichOneof("payload"),
            "last_message": event.last_message,
        }
        if event.HasField("finished"):
            fields["exit_code"] = event.finished.exit_code.code
            fields["exit_name"] = event.finished.exit_code.name
        if event.id.HasField("test_result"):
            fields["label"] = event.id.test_result.label
        if event.id.HasField("test_summary"):
            fields["label"] = event.id.test_summary.label
        if event.HasField("test_result"):
            fields["test_status"] = build_event_stream_pb2.TestStatus.Name(
                event.test_result.status
            )
        if event.HasField("test_summary"):
            fields["test_status"] = build_event_stream_pb2.TestStatus.Name(
                event.test_summary.overall_status
            )
        return fields

    class Recorder(publish_build_event_pb2_grpc.PublishBuildEventServicer):
        def PublishLifecycleEvent(self, request, context):
            event = request.build_event
            write(
                {
                    "rpc": "lifecycle",
                    "kind": event.event.WhichOneof("event"),
                    "seq": event.sequence_number,
                    **stream_fields(event.stream_id),
                }
            )
            return empty_pb2.Empty()

        def PublishBuildToolEventStream(self, request_iterator, context):
            pending = []
            stream = {}
            for request in request_iterator:
                time.sleep(args.read_delay)
                ordered = request.ordered_build_event
                stream = stream_fields(ordered.stream_id)
                entry = {
                    "rpc": "stream",
                    "kind": ordered.event.WhichOneof("event"),
                    "seq": ordered.sequence_number,
                    **stream,
                }
                if ordered.event.HasField("bazel_event"):
                    entry.update(bazel_fields(ordered.event.bazel_event))
                write(entry)
                ack = publish_build_event_pb2.PublishBuildToolEventStreamResponse(
                    stream_id=ordered.stream_id,
                    sequence_number=ordered.sequence_number,
                )
                if args.ack_at_eof:
                    pending.append(ack)
                else:
                    yield ack
            write(
                {
                    "rpc": "stream_end",
                    "acked_at_eof": len(pending),
                    **stream,
                }
            )
            yield from pending

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=32))
    publish_build_event_pb2_grpc.add_PublishBuildEventServicer_to_server(
        Recorder(), server
    )
    port = server.add_insecure_port("127.0.0.1:0")
    server.start()
    with open(args.port_file + ".tmp", "w") as f:
        f.write(str(port))
    os.replace(args.port_file + ".tmp", args.port_file)
    server.wait_for_termination()


if __name__ == "__main__":
    main()

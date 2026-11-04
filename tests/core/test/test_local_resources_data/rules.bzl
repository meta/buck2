# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _broker_impl(ctx):
    json = ctx.actions.write_json(
        "resources.json",
        {
            "resources": [{"my_alias": "42"}],
        },
        has_content_based_path = False,
    )
    return [
        DefaultInfo(),
        LocalResourceInfo(
            setup = cmd_args(["cat", json]),
            resource_env_vars = {
                "MY_RESOURCE_ID": "my_alias",
            },
            setup_timeout_seconds = 5,
        ),
    ]

_broker_attrs = {}

broker = rule(impl = _broker_impl, attrs = _broker_attrs)

def _test_impl(ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["true"],
            local_resources = {
                "my_resource_type": ctx.attrs.broker.label,
            },
            required_local_resources = [
                RequiredTestLocalResource("my_resource_type"),
            ],
        ),
    ]

_test_attrs = {
    "broker": attrs.dep(providers = [LocalResourceInfo]),
}

test = rule(impl = _test_impl, attrs = _test_attrs)

def _daemon_broker_impl(_ctx):
    # Setup command that backgrounds a long-lived process (simulating a real
    # resource broker), prints JSON with its PID, and exits. Without the fix,
    # cgroup cleanup would kill the backgrounded process when setup exits.
    return [
        DefaultInfo(),
        LocalResourceInfo(
            setup = cmd_args([
                "sh",
                "-c",
                'sleep 300 & PID=$!; echo "{\\"pid\\": $PID, \\"resources\\": [{\\"broker_pid\\": \\"$PID\\"}]}"',
            ]),
            resource_env_vars = {
                "BROKER_PID": "broker_pid",
            },
            setup_timeout_seconds = 5,
        ),
    ]

daemon_broker = rule(impl = _daemon_broker_impl, attrs = {})

def _daemon_test_impl(ctx):
    # Test that checks the broker process is still alive via kill -0.
    # Fails if cgroup cleanup killed the broker.
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["sh", "-c", "kill -0 $BROKER_PID"],
            local_resources = {
                "daemon_resource": ctx.attrs.broker.label,
            },
            required_local_resources = [
                RequiredTestLocalResource("daemon_resource"),
            ],
        ),
    ]

daemon_test = rule(
    impl = _daemon_test_impl,
    attrs = {
        "broker": attrs.dep(providers = [LocalResourceInfo]),
    },
)

def _two_types_one_broker_test_impl(ctx):
    # Two resource types served by the same LocalResourceInfo target.
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["sh", "-c", 'test "$MY_RESOURCE_ID" = 42'],
            local_resources = {
                "type_a": ctx.attrs.broker.label,
                "type_b": ctx.attrs.broker.label,
            },
            required_local_resources = [
                RequiredTestLocalResource("type_a"),
                RequiredTestLocalResource("type_b"),
            ],
        ),
    ]

two_types_one_broker_test = rule(
    impl = _two_types_one_broker_test_impl,
    attrs = {"broker": attrs.dep(providers = [LocalResourceInfo])},
)

def _registered_broker_impl(ctx):
    script = ctx.actions.write(
        "registered_broker.py",
        """
import json, os, socket, sys, time
mode = sys.argv[1]
config = json.loads(os.environ['BUCK2_LOCAL_RESOURCE_CLEANUP_V1'])
attempt = config['attempt_id']
ready_read, ready_write = os.pipe()
pid = os.fork()
if pid == 0:
    os.setsid()
    os.close(ready_read)
    for fd in (0, 1, 2):
        os.close(fd)
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(config['socket_path'])
    stream = channel.makefile('rb', buffering=0)
    def event(value):
        with open('registration-events', 'a') as output:
            output.write(value + ('_' + mode if mode in ('ack_hold', 'wait_for_holder') else '') + '\\n')
    if mode == 'wait_for_holder':
        deadline = time.monotonic() + 5
        while not os.path.exists('registration-events') or 'register_ack_hold' not in open('registration-events').read():
            assert time.monotonic() < deadline
            time.sleep(.01)
    channel.sendall(('HELLO 1 ' + attempt + ' ' + str(os.getpid()) + '\\n').encode())
    assert stream.readline() == ('CONNECTED 1 ' + attempt + '\\n').encode()
    event('connected')
    os.write(ready_write, b'C')
    os.close(ready_write)
    record = stream.readline()
    if record:
        assert record == ('REGISTER 1 ' + attempt + '\\n').encode()
        event('register')
        if mode == 'ack_hold':
            deadline = time.monotonic() + 5
            while 'registered_wait_for_holder' not in open('registration-events').read():
                assert time.monotonic() < deadline
                time.sleep(.01)
        if mode not in ('ack_loss', 'withheld'):
            channel.sendall(('REGISTERED 1 ' + attempt + '\\n').encode())
            event('registered')
            assert stream.read(1) == b''
        elif mode == 'withheld':
            assert stream.read(1) == b''
    event('cleanup')
    os._exit(0)
os.close(ready_write)
assert os.read(ready_read, 1) == b'C'
os.close(ready_read)
print(json.dumps({'pid': pid, 'resources': [{'my_alias': '42'}], 'cleanup_registration': {'version': 1, 'attempt_id': 'wrong' if mode == 'bad_descriptor' else attempt}}), flush=True)
if mode == 'timeout':
    time.sleep(60)
sys.exit(1 if mode == 'failure' else 0)
""",
    )
    resource = LocalResourceInfo(
        setup = cmd_args(["python3", script, ctx.attrs.mode]),
        resource_env_vars = {"MY_RESOURCE_ID": "my_alias"},
        setup_timeout_seconds = 1 if ctx.attrs.mode in ("timeout", "withheld") else 5,
    )
    return [DefaultInfo(sub_targets = {"alias": [DefaultInfo(), resource]}), resource]

registered_broker = rule(
    impl = _registered_broker_impl,
    attrs = {"mode": attrs.string(default = "success")},
)

def _dual_registered_test_impl(ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            type = "custom",
            command = ["true"],
            local_resources = {"holder": ctx.attrs.holder.label, "waiter": ctx.attrs.waiter.label},
            required_local_resources = [RequiredTestLocalResource("holder"), RequiredTestLocalResource("waiter")],
        ),
    ]

dual_registered_test = rule(
    impl = _dual_registered_test_impl,
    attrs = {"holder": attrs.dep(providers = [LocalResourceInfo]), "waiter": attrs.dep(providers = [LocalResourceInfo])},
)

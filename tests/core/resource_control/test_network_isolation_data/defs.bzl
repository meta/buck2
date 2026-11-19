# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Try a localhost TCP connection (bind a listener, then connect to it).
# In a new network namespace lo is present but not routing, so the connect
# fails with ENETUNREACH.  Exit 0 if the connection succeeds (network
# accessible), exit 1 if it fails (network isolated).
_LOCALHOST_CONNECT = """\
import socket, sys, threading
srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.bind(('127.0.0.1', 0))
port = srv.getsockname()[1]
srv.listen(1)
def accept():
    conn, _ = srv.accept()
    conn.close()
t = threading.Thread(target=accept, daemon=True)
t.start()
try:
    c = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    c.settimeout(2)
    c.connect(('127.0.0.1', port))
    c.close()
    srv.close()
    sys.exit(0)
except OSError:
    srv.close()
    sys.exit(1)
"""

def _impl_network_isolated(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["python3", "-c", _LOCALHOST_CONNECT],
            default_executor = CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = False,
                remote_cache_enabled = False,
                network_access = "none",
            ),
            type = "custom",
        ),
    ]

network_isolated_test = rule(attrs = {}, impl = _impl_network_isolated)

def _impl_network_accessible(_ctx):
    return [
        DefaultInfo(),
        ExternalRunnerTestInfo(
            command = ["python3", "-c", _LOCALHOST_CONNECT],
            type = "custom",
        ),
    ]

network_accessible_test = rule(attrs = {}, impl = _impl_network_accessible)

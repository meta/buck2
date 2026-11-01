# buckd

Buck runs a persistent daemon process (buckd) to reuse work between commands.
Most work is done by the daemon process. When executing a buck command, the
process running the command is a client to the buckd server. The buckd server
exposes a simple grpc service that the client uses to implement the various buck
commands.

There's a small set of commands/arguments that don't require the daemon
(`buck help`, cli arg parse failures, `buck version`, ...), but most commands
will require it.

For almost all commands, buck requires that the client and server are the same
version of buck and may restart buckd to ensure that's the case.

# daemon process flow

The daemon process is started with the (hidden) `buck daemon` command.

The daemon process has a simple startup. It redirects its stdout and stderr to
files in the "daemon directory" (a directory under `$HOME/.buck/buckd` specific
to that repository+isolation dir), binds the socket for the grpc DaemonApi
server, and daemonizes itself. It then writes its pid to the "buckd.pid" file
and the endpoint it is serving on (along with some other information) to the
"buckd.info" file in the daemon dir, overwriting whatever was there. Once that
is done, the server is ready to be used.

The daemon holds no lock of its own: nothing daemon-side prevents a second
daemon from starting over the same daemon directory. Exclusivity relies on the
client-side lifecycle lock described below, plus the periodic self-check in
point 2 here.

With `BUCK2_SHARE_DAEMON=true`, isolation directories within a repository share
the reserved `._buck2_shared` daemon directory and retain separate output and
cache state.

Sharing is opt-in during initial rollout. Set `BUCK2_SHARE_DAEMON=true` for
selected callers; unset it or set it to `false` to restore separate daemon
directories. Inplace tests and `buck2.py` disable sharing so their local
binary does not replace the daemon running the outer build or test command.
Clients check the daemon executable version and startup settings before reuse.
A mismatch or a failed connection to a live shared daemon is reported without
killing sibling tenants. Stop the tenants with `buck2 killall --repo` before
changing daemon settings or binaries. Unshared daemons retain restart recovery.

Tenant disk ownership uses a persistent OS file lock independent of daemon
routing. A conflicting daemon cannot initialize the same tenant output and cache
paths until the owner releases them. `buck2 clean` evicts the selected shared
tenant and waits for cancellation and cleanup before deletion; sibling tenants and the
daemon-global forkserver state remain available. Lock files are outside the
disposable tenant directories. Shared tenants also retain the legacy isolation
lifecycle lock, fencing startup by older clients. An already running legacy
daemon must be stopped before shared initialization can claim its disk state.


There are 3 ways that the buckd process will shutdown:

1. The grpc api includes a `kill()` call that will shutdown buckd.
2. buckd will periodically (every 60s by default) check that the "buckd.pid"
   file still contains its own pid and that the project root is still
   accessible, and exits if not. This is how a daemon that has been replaced,
   or whose daemon directory was deleted, goes away — up to one check interval
   late, so a replaced daemon can briefly coexist with its replacement.
3. If buckd hits a rust `panic()` the buckd process will exit

# client connection and buckd startup

When the client is processing a command that requires communicating with the
buckd server it will follow this approach:

1. read the "buckd.info" file to get the port the grpc api is being served on
2. connect to the api on that port
3. send a `status()` request to get the version

If there is an error during 1-3, or if there is a version mismatch the client
needs to (re)start the buck daemon. Otherwise, the client can continue as it now
has made a connection with a correctly versioned buckd.

When the client is killing or starting the buckd process, it will grab an
exclusive lock on the "buckd.lifecycle" file in the daemon directory to ensure
that multiple clients aren't racing with each other.

To start/restart the buckd process, the client does:

1. lock the "buckd.lifecycle" file
2. send a kill command to the existing buckd
3. ensure the buckd process has exited (based on pid)
4. run a `buck daemon` command to start buckd
5. wait for the daemon to start up and the grpc server to be ready
6. release the "buckd.lifecycle" file

After that, it will repeat the connection steps (including verifying the version
after connecting).

# buck kill and other daemon restarts

If there are other invocations currently using the buck daemon when it is killed
or restarted by a client, those invocations will fail due to the early
disconnection.

Generally, we support concurrent buck invocations using the same buck version,
but if there are concurrent invocations with different versions, they may
unexpectedly fail or otherwise work incorrectly. This is sufficient for the
normal buck workflow where the buckversion is checked into the repo, in that
case, it's not expected that buck commands will work across a rebase or other
operation that changes the buckversion.

# correctness

We have a couple of guarantees here.

1. Only a single client is killing/starting a buckd for a given daemon
   directory at a time
2. A buckd that is no longer the one named by its daemon directory exits within
   one self-check interval
3. A client only uses a buckd connection after making sure it has a compatible
   version

Note what is **not** guaranteed: that only a single buckd is running at a time.
A replaced daemon lingers for up to one self-check interval, and the daemon
directory lives under `$HOME`, so environments that give clients different
`$HOME`s (containers, sandboxes) can start daemons that never see each other
while sharing the same buck-out. During such overlap the daemons contend on
shared state in buck-out (materializer/dep-file sqlite databases, the
forkserver's miniperf binary).

The main way that we could run into issues would be if there are multiple
clients that are racing and they want different versions of buck. In that case,
one might cause the other two fail to connect to a buckd with the correct
version or one of the client's connections may be prematurely disconnected. A
client **will not** use a server with a mismatched version. While this is a
failure, no expected workflow would hit this case, all concurrent commands
should be using the same buck version.

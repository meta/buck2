# Remote execution integration with Buildbarn

This project provides a small example of what a project that utilizes
[Buildbarn](https://github.com/buildbarn).

In this document, we will go over the key configs used in this setup. Using a
local docker-compose deployment from the
[example deployment repo](https://github.com/buildbarn/bb-deployments). If you
already have a `Buildbarn` deployment you can use that instead.

## Deploy a local Buildbarn

### Set up a basic PATH

`Buildbarn` runs all actions in a completely bare environment by default, and
relies on the build client to specify the `$PATH` and other environment
variables in the REAPI action message. As `buck2` does not send a default
`$PATH` build will fail with errors about missing executables.

Add the environment variables to the worker config you want - normally either
`docker-compose/config/worker-fuse-ubuntu22-04.jsonnet` or
`docker-compose/config/worker-hardlinking-ubuntu22-04.jsonnet` - after the
`workerId` field like this:

```
 { ...
   buildDirectories: [{ ...
     runners: [{
       endpoint: { address: 'unix:///worker/runner' },
       platform: {
         properties: [ ... ],
       },
       workerId: { ... },
+      environment_variables: {
+        PATH: '/bin:/usr/bin:/usr/local/bin',
+      },
     }],
   }],
   filePool: { ... },
 }
```

### Deploy

```
... $ git clone https://github.com/buildbarn/bb-deployments
... $ cd bb-deployments
.../bb-deployments $ cd docker-compose
.../bb-deployments/docker-compose $ ./run.sh
```

This uses `docker-compose` to spin up the required `Buildbarn` services. Using
FUSE based workers, those are generally the fastest as they can load action
files on demand and avoids the overhead of setting up the full input root up
front. In practice many actions do not read all the files in the input root.

If you do not want FUSE workers you can instead switch to hardlinking workers
The example deployments have two worker kinds "fuse", and "hardlinking", you can
see the queues in the `Buildbarn` portal, http://localhost:8081.

```
Buildbarn Scheduler

...

Instance name                                    Platform properties                                    Size  Timeo
   prefix                                                                                               class

"fuse"        OSFamily="linux" container-image="docker://ghcr.io/catthehacker/                          0     ∞
              ubuntu:act-22.04@sha256:dd7654ffb01d5b7b54b23b9ce928a1f7f2d08c7b3d7e320b6574b55d7ccde78b"
"hardlinking" OSFamily="linux" container-image="docker://ghcr.io/catthehacker/                          0     ∞
              ubuntu:act-22.04@sha256:dd7654ffb01d5b7b54b23b9ce928a1f7f2d08c7b3d7e320b6574b55d7ccde78b"
```

More information is available in the
[repo](https://github.com/buildbarn/bb-deployments).

## Relevant configs in .buckconfig

Configure the `Buildbarn` endpoint as follows:

```ini
[buck2_re_client]
engine_address       = grpc://localhost:8980
action_cache_address = grpc://localhost:8980
cas_address          = grpc://localhost:8980
tls                  = false

# Select the instance name, "fuse" or "hardlinking", without quotes
instance_name        = fuse
```

TLS is not used in this example.

## Relevant configs in `ExecutionPlatformInfo`

`Buildbarn` takes in a Docker image and `OSFamily` in its RE properties to
select a worker. This is configured in `root//platforms:platforms`.

In this deployment the properties must be exactly the ones a worker registered
with: the portal lists them, and so does the worker's config in
`docker-compose/config`. Otherwise the build fails with
`No workers exist for instance name prefix "fuse" platform {...}`. The image
here is the one `bb-deployments` used in November 2026: update
`remote_execution_properties` when your deployment uses another.

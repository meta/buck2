# This file is @generated, regenerate by re-running test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command

# DefaultInfo
# //foo_binary.bzl
# //subdir/BUCK
# ":gen_stuff" pulls the default_outputs for //subdir:gen_stuff
# Builds just 'foo' binary. The strip command is never invoked.
# builds the 'foo' binary, because it is needed by the 'strip' command. Ensures that
# both the stripped binary and the debug symbols are built.
## DefaultInfo.default\_outputs
## DefaultInfo.other\_outputs
## DefaultInfo.sub\_targets
## Implements

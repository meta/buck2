Setup
  $ source "$TESTDIR/setup.sh"
  $ export NO_COLOR=1
  $ echo '{"IsSortableListArg": {"deps": true, "srcs": true, "visibility": true}, "SortableBlacklist": {}, "NamePriority": {"name": -99, "deps": 10, "visibility": 90}, "Overrides": [{"Files": ["*.bzl"], "SortListArgs": false, "SortKwargs": false}]}' > test_config.json
  $ alias starlark-fmt-cfg='"$STARLARK_FMT_PATH" --config test_config.json fmt'

Test: .bzl files do NOT get kwargs sorted
  $ cat <<'EOF' > defs.bzl
  > def my_macro():
  >     my_rule(
  >         deps = [],
  >         name = "example",
  >         visibility = ["PUBLIC"],
  >     )
  > EOF
  $ starlark-fmt-cfg defs.bzl
  $ cat defs.bzl
  def my_macro():
      my_rule(
          deps = [],
          name = "example",
          visibility = ["PUBLIC"],
      )

Test: BUILD files DO get kwargs sorted (name first due to priority -99)
  $ cat <<'EOF' > BUILD
  > my_rule(
  >     deps = [],
  >     name = "example",
  >     visibility = ["PUBLIC"],
  > )
  > EOF
  $ starlark-fmt-cfg BUILD
   INFO process_file: BUILD: formatted
  $ cat BUILD
  my_rule(
      name = "example",
      deps = [],
      visibility = ["PUBLIC"],
  )

Test: .bzl files do NOT get list args sorted
  $ cat <<'EOF' > macros.bzl
  > def my_macro():
  >     my_rule(
  >         name = "example",
  >         deps = [":z", ":a", ":m"],
  >     )
  > EOF
  $ starlark-fmt-cfg macros.bzl
  $ cat macros.bzl
  def my_macro():
      my_rule(
          name = "example",
          deps = [":z", ":a", ":m"],
      )

Test: .bzl files DO sort explicit keep-sorted list expressions
  $ cat <<'EOF' > keep_sorted_return.bzl
  > def get_deps():
  >     return [
  >         # buildifier: keep sorted
  >         "fbsource//xplat/sonar/iOS:FlipperKitApple",
  >         "fbsource//fbobjc/Apps/LightSpeed/Libraries/LightSpeedUIUtils:LSMountableViewFlipperSupport",
  >         "fbsource//fbobjc/Libraries/Merlin:FBMerlinFlipperPlugin",
  >     ]
  > EOF
  $ starlark-fmt-cfg keep_sorted_return.bzl
   INFO process_file: keep_sorted_return.bzl: formatted
  $ cat keep_sorted_return.bzl
  def get_deps():
      return [
          # buildifier: keep sorted
          "fbsource//fbobjc/Apps/LightSpeed/Libraries/LightSpeedUIUtils:LSMountableViewFlipperSupport",
          "fbsource//fbobjc/Libraries/Merlin:FBMerlinFlipperPlugin",
          "fbsource//xplat/sonar/iOS:FlipperKitApple",
      ]

Test: BUILD files DO get list args sorted
  $ cat <<'EOF' > TARGETS
  > my_rule(
  >     name = "example",
  >     deps = [":z", ":a", ":m"],
  > )
  > EOF
  $ starlark-fmt-cfg TARGETS
   INFO process_file: TARGETS: formatted
  $ cat TARGETS
  my_rule(
      name = "example",
      deps = [":a", ":m", ":z"],
  )

Test: .bzl files still get loads sorted
  $ cat <<'EOF' > sorted_loads.bzl
  > load("//z:z.bzl", "z_func")
  > load("//a:a.bzl", "a_func")
  > x = a_func() + z_func()
  > EOF
  $ starlark-fmt-cfg sorted_loads.bzl
   INFO process_file: sorted_loads.bzl: formatted
  $ cat sorted_loads.bzl
  load("//a:a.bzl", "a_func")
  load("//z:z.bzl", "z_func")
  x = a_func() + z_func()

Test: .bzl files still get dict keys sorted
  $ cat <<'EOF' > dict_keys.bzl
  > d = {"z": 1, "a": 2}
  > EOF
  $ starlark-fmt-cfg dict_keys.bzl
   INFO process_file: dict_keys.bzl: formatted
  $ cat dict_keys.bzl
  d = {"a": 2, "z": 1}

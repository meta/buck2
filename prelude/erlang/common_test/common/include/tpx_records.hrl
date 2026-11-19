%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format

-record(test_spec_test_info, {
    name :: binary(),
    filter :: binary(),
    breakpoint :: mfa()
}).

-record(test_spec_test_case, {
    suite :: binary(),
    testcases :: [#test_spec_test_info{}]
}).

-type optional(Type) :: undefined | Type.

-record(test, {
    name :: string(),
    suite :: string(),
    type :: string(),
    message :: optional(string()),
    stacktrace :: optional(string()),
    stdout :: optional(string()),
    stderr :: optional(string()),
    time :: optional(integer())
}).

-record(test_case, {
    name :: string(),
    tests :: [#test{}]
}).

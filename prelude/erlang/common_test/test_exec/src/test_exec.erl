%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(test_exec).
-moduledoc """
The test_exec application deals with running
the test as part of a separate sub-process along
with the epmd daemon.
""".
-compile(warn_missing_spec_all).

-behavior(application).

-export([start/2, stop/1, kill_process/1]).

-include_lib("kernel/include/logger.hrl").
-include_lib("common/include/buck_ct_records.hrl").

-spec start(Type, Args) -> {'ok', pid()} | {'error', supervisor:startlink_err()} when
    Type :: application:start_type(),
    Args :: term().
start(_Type, _Args) ->
    case application:get_env(test_exec, test_env) of
        {ok, #test_env{} = TestEnv} ->
            test_exec_sup:start_link(TestEnv);
        _ ->
            %% hack to make startup not fail if no config is set
            {ok, spawn_link(timer, sleep, [infinity])}
    end.

-spec stop(State) -> ok when
    State :: term().
stop(_State) -> ok.

-spec kill_process(port()) -> ok.
kill_process(Port) ->
    case erlang:port_info(Port, os_pid) of
        undefined ->
            ok;
        {os_pid, OsPid} ->
            X = os:cmd(io_lib:format("kill -9 -~tp", [OsPid])),
            ?LOG_DEBUG("Sending kill signal to process group from ~p resulted in ~p", [OsPid, X]),
            spawn(fun() -> port_close(Port) end),
            ok
    end.

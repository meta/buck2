%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

% @format
-module(ct_executor_watchdog).
-moduledoc """
This module is responsible for watching the test binary and killing the beam
if the test binary goes down. This is needed because the test binary can be
terminated by the test runner in ways (SIGKILL) that we cannot intercept and
we want to ensure we don't leave lingering beam instances behind.

NOTE: Ideally, we would use the peer module to start the test node and have
this shutdown handled by it.
""".
-compile(warn_missing_spec_all).

-behaviour(gen_server).

-export([start_link_client/1, start_server/0]).

-export([
    init/1,
    handle_info/2,
    handle_call/3,
    handle_cast/2
]).

-include_lib("kernel/include/logger.hrl").

-record(state, {
    socket :: gen_tcp:socket() | undefined
}).

-spec start_server() -> {ok, number()}.
start_server() ->
    ServerPid = spawn(fun server/0),
    Ref = make_ref(),
    ServerPid ! {self(), Ref, get_port},
    receive
        {Ref, Port} -> {ok, Port}
    end.

-spec server() -> ok.
server() ->
    {ok, LSock} = gen_tcp:listen(0, [binary, {active, false}, inet6]),
    {ok, Port} = inet:port(LSock),
    receive
        {From, Ref, get_port} -> From ! {Ref, Port}
    end,
    {ok, _Sock} = gen_tcp:accept(LSock),
    %% wait forever
    receive
        _ -> ok
    end.

-spec start_link_client(Port :: integer()) -> gen_server:start_ret().
start_link_client(Port) ->
    gen_server:start_link({local, ?MODULE}, ?MODULE, [Port], []).

-spec init([Port :: integer()]) -> {ok, #state{}} | {error, failed_to_connect_watchdog}.
init([Port]) ->
    try
        {ok, Sock} = gen_tcp:connect("localhost", Port, [inet6, {active, true}]),
        {ok, #state{socket = Sock}}
    catch
        Class:Reason:Stack ->
            ?LOG_ERROR("Failed to connect to watchdog server: ~ts", [erl_error:format_exception(Class, Reason, Stack)]),
            {error, failed_to_connect_watchdog}
    end.

-spec handle_call(term(), gen_server:from(), #state{}) -> no_return().
handle_call(_Request, _From, _State) ->
    error(not_implemented).

-spec handle_cast(term(), #state{}) -> no_return().
handle_cast(_Request, _State) ->
    error(not_implemented).

-spec handle_info(term(), #state{}) -> no_return().
handle_info({tcp_closed, Socket}, #state{socket = Socket}) ->
    %% when the test binary goes down, we kill the beam
    ?LOG_CRITICAL("Test binary went down, killing beam"),
    test_logger:flush(),
    erlang:halt(1).

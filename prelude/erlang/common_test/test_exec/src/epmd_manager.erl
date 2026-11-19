%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(epmd_manager).
-moduledoc """
This module interfaces with the epmd daemon. It allows one to start/stop one for
each suite execution.
""".
-compile(warn_missing_spec_all).

-behaviour(gen_server).

%% UI methods
-export([start_link/1, get_epmd_out_path/1, get_port/0]).

%% gen_server callbacks
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-include_lib("common/include/buck_ct_records.hrl").

-define(raw_file_access, prim_file).

-doc """
Find a new port and starts this epmd daemon on this new port, ensures it is up and working,
and set up the env variable ERL_EPMD_PORT to the port this daemon is working.
""".
-spec start_link(#test_env{}) -> gen_server:start_ret().
start_link(#test_env{} = TestEnv) ->
    gen_server:start_link({local, ?MODULE}, ?MODULE, TestEnv, []).

-spec get_port() -> inet:port_number().
get_port() ->
    gen_server:call(?MODULE, get_port).

%% ---------------- gen_server callbacks---------------

-type local_epmd_state() ::
    #{
        global_epmd := false,
        epmd_port := inet:port_number(),
        epmd_erlang_port := erlang:port(),
        log_handle := io:device()
    }.
-type global_epmd_state() :: #{
    global_epmd := true,
    epmd_port := inet:port_number()
}.

-type state() :: local_epmd_state() | global_epmd_state().

-spec init(#test_env{}) -> {ok, state()} | {stop, Reason} when
    Reason :: {epmd_start_failed, Error :: term()}.
init(#test_env{output_dir = OutputDir}) ->
    process_flag(trap_exit, true),

    case application:get_env(test_exec, global_epmd_port) of
        undefined ->
            EpmdOutPath = get_epmd_out_path(OutputDir),
            case start_epmd(EpmdOutPath) of
                {ok, Port, PortEpmd, LogHandle} ->
                    {ok, #{
                        epmd_port => Port,
                        epmd_erlang_port => PortEpmd,
                        log_handle => LogHandle,
                        global_epmd => false
                    }};
                Error ->
                    {stop, {epmd_start_failed, Error}}
            end;
        {ok, Port} ->
            {ok, #{epmd_port => Port, global_epmd => true}}
    end.

-spec handle_cast(Request, State) -> {noreply, State} when
    Request :: term(),
    State :: state().
handle_cast(_Request, State) -> {noreply, State}.

-spec handle_call(get_port, From, State) -> {reply, Port, State} when
    From :: gen_server:from(),
    State :: state(),
    Port :: inet:port_number().
handle_call(get_port, _From, State = #{epmd_port := Port}) -> {reply, Port, State}.

-spec handle_info
    ({PortEpmd, {exit_status, ExitStatus}}, State) -> {stop, {epmd_crashed, ExitStatus}, State} when
        PortEpmd :: erlang:port(),
        ExitStatus :: integer(),
        State :: local_epmd_state();
    ({PortEpmd, closed}, State) -> {stop, epmd_port_closed, State} when
        PortEpmd :: erlang:port(),
        State :: local_epmd_state();
    ({'EXIT', PortEpmd, Reason}, State) -> {stop, {epmd_exit, Reason}, State} | {noreply, State} when
        PortEpmd :: erlang:port(),
        Reason :: term(),
        State :: local_epmd_state();
    ({PortEpmd, {data, TaggedData}}, State) -> {noreply, State} when
        PortEpmd :: erlang:port(),
        TaggedData :: {noeol, Data} | {eol, Data},
        Data :: string(),
        State :: local_epmd_state();
    (none(), State) -> {noreply, State} when
        State :: state().

handle_info({PortEpmd, {exit_status, ExitStatus}}, State) ->
    #{epmd_erlang_port := PortEpmd} = State,
    {stop, {epmd_crashed, ExitStatus}, State};
handle_info({PortEpmd, closed}, State) ->
    #{epmd_erlang_port := PortEpmd} = State,
    {stop, epmd_port_closed, State};
handle_info({'EXIT', PortEpmd, Reason}, #{epmd_erlang_port := PortEpmd} = State) ->
    {stop, {epmd_exit, Reason}, State};
handle_info({PortEpmd, {data, TaggedData}}, State) ->
    #{epmd_erlang_port := PortEpmd, log_handle := LogHandle} = State,
    UntaggedData =
        case TaggedData of
            {noeol, Data} -> Data;
            {eol, Data} -> Data
        end,
    log_input_data(UntaggedData, LogHandle),
    {noreply, State};
handle_info(_Info, State) ->
    {noreply, State}.

-spec terminate(Reason, State) -> ok when
    Reason :: normal | shutdown | {shutdown, term()} | term(),
    State :: state().
terminate(_Reason, #{epmd_erlang_port := EpmdPort}) ->
    test_exec:kill_process(EpmdPort);
terminate(_Reason, _State) ->
    ok.

%% --------------  Utilities methods ------------------

-doc """
Find a new port and starts this epmd daemon on this new port, then ensures it is up and working.
 We may have to retry in case the port becomes busy between the time
it is discovered and the time the epmd is ran.
""".
-spec start_epmd(EpmdOutPath) ->
    {ok, inet:port_number(), erlang:port(), io:device()} | {error, {epmd_starting_failed, Error}}
when
    EpmdOutPath :: file:filename_all(),
    Error :: term().
start_epmd(EpmdOutPath) ->
    start_epmd(EpmdOutPath, 3, no_error).

-spec start_epmd(EpmdOutPath, Attempts, Error) ->
    {ok, inet:port_number(), erlang:port(), io:device()} | {error, {epmd_starting_failed, Error}}
when
    EpmdOutPath :: file:filename_all(),
    Attempts :: integer(),
    Error :: term().
start_epmd(EpmdOutPath, Attempts, _Error) when Attempts > 0 ->
    case find_free_port() of
        {ok, Port} ->
            case start_epmd_instance(Port, EpmdOutPath) of
                {ok, PortEpmd, LogHandle} -> {ok, Port, PortEpmd, LogHandle};
                {failed, Reason} -> start_epmd(EpmdOutPath, Attempts - 1, Reason)
            end;
        Error ->
            start_epmd(EpmdOutPath, Attempts - 1, Error)
    end;
start_epmd(_LogDir, 0, Error) ->
    {error, {epmd_starting_failed, Error}}.

-doc """
Finds a free TCP port.
This methods relies on a special behavior of gen_tcp:listen/0,
See https://www.erlang.org/doc/man/gen_tcp.html#listen-2
""".
-spec find_free_port() -> {ok, inet:port_number()} | {error, Reason} when
    Reason :: system_limit | inet:posix().
find_free_port() ->
    case gen_tcp:listen(0, [inet]) of
        {ok, ListenSocket} ->
            Port = {ok, _} = inet:port(ListenSocket),
            gen_tcp:close(ListenSocket),
            Port;
        {error, Reason} ->
            {error, Reason}
    end.

-doc """
Starts the epmd daemon on the given port,
and writes stdout, stderr to files in the LogDir.
""".
-spec start_epmd_instance(inet:port_number(), file:filename_all()) -> {ok, port(), io:device()} | {failed, term()}.
start_epmd_instance(Port, EpmdOutPath) ->
    %% Note on the -d flag from `man 1 epmd`:
    %% Enables debug output. The more -d flags specified, the more
    %% debug output you will get (to a certain limit). This option is
    %% most useful when the epmd daemon is not started as a daemon.
    LogHandle = get_log_handle(EpmdOutPath),
    ProcessPort = erlang:open_port(
        {
            spawn,
            string:join(
                [
                    "epmd",
                    "-d",
                    "-d",
                    "-port",
                    integer_to_list(Port)
                ],
                " "
            )
        },
        [stderr_to_stdout, exit_status, use_stdio, {line, 4096}]
    ),
    case listen_loop(ProcessPort, LogHandle, []) of
        ok ->
            {ok, ProcessPort, LogHandle};
        {failed, _} = Error when is_pid(LogHandle) ->
            file:close(LogHandle),
            Error;
        {failed, _} = Error ->
            Error
    end.

-spec listen_loop(ProcessPort, LogHandle, Acc) -> {failed, term()} | ok when
    ProcessPort :: erlang:port(),
    LogHandle :: io:device(),
    Acc :: [string()].
listen_loop(ProcessPort, LogHandle, Acc) ->
    receive
        {ProcessPort, {exit_status, Exit}} ->
            {failed, {epmd_exit, Exit}};
        {ProcessPort, {data, {noeol, Data}}} ->
            log_input_data(Data, LogHandle),
            listen_loop(ProcessPort, LogHandle, [Data | Acc]);
        {ProcessPort, {data, {eol, Data}}} ->
            log_input_data(Data, LogHandle),

            FullLine = string:join([Data | lists:reverse(Acc)], ""),
            case string:find(FullLine, "entering the main select() loop") of
                nomatch ->
                    listen_loop(ProcessPort, LogHandle, []);
                _ ->
                    ok
            end
    after 1000 ->
        test_exec:kill_process(ProcessPort),
        {failed, timeout}
    end.

-spec get_log_handle(file:name_all()) -> io:device().
get_log_handle(EpmdOutPath) ->
    case filelib:is_file(EpmdOutPath, ?raw_file_access) of
        true -> ok = file:delete(EpmdOutPath, [raw]);
        false -> ok
    end,
    % Can't open in raw mode, since we want this handle to be consumable elsewhere
    {ok, LogHandle} = file:open(EpmdOutPath, [write]),
    LogHandle.

-spec log_input_data(Data, LogHandle) -> ok when
    Data :: unicode:chardata(),
    LogHandle :: io:device().
log_input_data(Data, LogHandle) ->
    io:format(LogHandle, "~ts", [Data]).

-spec get_epmd_out_path(OutDir) -> file:filename_all() when
    OutDir :: file:filename_all().
get_epmd_out_path(OutputDir) ->
    filename:join(OutputDir, "epmd_out.log").

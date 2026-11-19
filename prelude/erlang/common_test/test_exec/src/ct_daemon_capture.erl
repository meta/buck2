%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(ct_daemon_capture).
-moduledoc """
Group leader implementation that supports ct:capture_* API for the test shell.

This module provides a group leader process that can intercept I/O output
when capture mode is enabled. It implements the Erlang I/O protocol and
forwards messages to the original group leader while optionally capturing
them.

The OTP ct:capture_start() sends {capture, Pid} to the group leader.
This module handles that message to enable/disable capture mode.
""".
-compile(warn_missing_spec_all).

-behaviour(gen_server).

%% Public API
-export([start_link/1, stop/1]).

%% gen_server callbacks
-export([init/1, handle_call/3, handle_cast/2, handle_info/2, terminate/2]).

-record(state, {
    original_gl :: pid(),
    capture_pid :: pid() | false
}).

-type state() :: #state{}.

%%--------------------------------------------------------------------
%% Public API
%%--------------------------------------------------------------------

-doc """
Start a capture-aware group leader that forwards to the original group leader.
Returns {ok, Pid} where Pid is the new group leader that can be set with
erlang:group_leader/2.
""".
-spec start_link(pid()) -> gen_server:start_ret().
start_link(OriginalGL) ->
    gen_server:start_link(?MODULE, OriginalGL, []).

-doc """
Stop the capture group leader.
""".
-spec stop(pid()) -> ok.
stop(Pid) ->
    gen_server:stop(Pid, normal, 5000).

%%--------------------------------------------------------------------
%% gen_server callbacks
%%--------------------------------------------------------------------

-spec init(pid()) -> {ok, state()}.
init(OriginalGL) ->
    {ok, #state{original_gl = OriginalGL, capture_pid = false}}.

-spec handle_call(term(), gen_server:from(), state()) -> {reply, term(), state()}.
%% Handle capture via gen_server:call (for compatibility with callers using call instead of message)
handle_call({capture, Pid}, _From, State) when is_pid(Pid) ->
    {reply, ok, State#state{capture_pid = Pid}};
handle_call({capture, false}, _From, State) ->
    {reply, ok, State#state{capture_pid = false}};
%% Handle I/O requests via gen_server:call (some io operations use this)
handle_call({io_request, _From, _ReplyAs, Request}, _GenFrom, State) ->
    {Reply, NewState} = handle_io_request(Request, State),
    {reply, Reply, NewState};
handle_call(_Request, _From, State) ->
    {reply, {error, unknown_request}, State}.

-spec handle_cast(term(), state()) -> {noreply, state()}.
handle_cast(_Msg, State) ->
    {noreply, State}.

-spec handle_info(term(), state()) -> {noreply, state()}.
%% Handle capture start message from OTP ct:capture_start() / test_server:capture_start()
handle_info({capture, Pid}, State) when is_pid(Pid) ->
    {noreply, State#state{capture_pid = Pid}};
%% Handle capture stop message from OTP ct:capture_stop() / test_server:capture_stop()
handle_info({capture, false}, State) ->
    {noreply, State#state{capture_pid = false}};
%% Handle I/O requests - the core I/O protocol
handle_info({io_request, From, ReplyAs, Request}, State) when is_pid(From) ->
    {Reply, NewState} = handle_io_request(Request, State),
    From ! {io_reply, ReplyAs, Reply},
    {noreply, NewState};
%% Ignore io_reply messages - these are responses from the original group leader
%% to requests we forwarded, and we don't need to do anything with them
handle_info({io_reply, _ReplyAs, _Result}, State) ->
    {noreply, State};
%% Forward unknown messages to original group leader
handle_info(Msg, State = #state{original_gl = GL}) ->
    GL ! Msg,
    {noreply, State}.

-spec terminate(term(), state()) -> ok.
terminate(_Reason, _State) ->
    ok.

%%--------------------------------------------------------------------
%% I/O Protocol Handling
%%--------------------------------------------------------------------

-spec handle_io_request(term(), state()) -> {term(), state()}.
handle_io_request({put_chars, Encoding, Chars}, State) when is_atom(Encoding), is_binary(Chars) ->
    handle_put_chars(Chars, State);
handle_io_request({put_chars, Encoding, Chars}, State) when is_atom(Encoding), is_list(Chars) ->
    handle_put_chars_list(Chars, State);
handle_io_request({put_chars, Encoding, M, F, A}, State) when
    is_atom(Encoding), is_atom(M), is_atom(F), is_list(A)
->
    try erlang:apply(M, F, A) of
        Chars when is_binary(Chars) -> handle_put_chars(Chars, State);
        Chars when is_list(Chars) -> handle_put_chars_unsafe(Chars, State);
        _ -> {{error, {M, F, A}}, State}
    catch
        _:_ -> {{error, {M, F, A}}, State}
    end;
handle_io_request({put_chars, Chars}, State) when is_binary(Chars) ->
    handle_put_chars(Chars, State);
handle_io_request({put_chars, Chars}, State) when is_list(Chars) ->
    handle_put_chars_list(Chars, State);
handle_io_request({put_chars, M, F, A}, State) when is_atom(M), is_atom(F), is_list(A) ->
    handle_io_request({put_chars, latin1, M, F, A}, State);
handle_io_request({get_chars, _, _, _}, State) ->
    {eof, State};
handle_io_request({get_line, _, _}, State) ->
    {eof, State};
handle_io_request({get_until, _, _, _, _, _}, State) ->
    {eof, State};
handle_io_request({get_geometry, _}, State) ->
    {{error, enotsup}, State};
handle_io_request(getopts, State) ->
    {[{encoding, unicode}], State};
handle_io_request({setopts, _}, State) ->
    {ok, State};
handle_io_request({requests, Reqs}, State) when is_list(Reqs) -> handle_requests(Reqs, State, ok);
handle_io_request(_, State) ->
    {{error, request}, State}.

-spec handle_put_chars(iodata(), state()) -> {ok, state()}.
handle_put_chars(Chars, State = #state{original_gl = GL, capture_pid = Cap}) when
    is_binary(Chars); is_list(Chars)
->
    %% Always forward to the original group leader
    GL ! {io_request, self(), make_ref(), {put_chars, unicode, Chars}},
    %% If capturing, send captured output to capture_pid's mailbox
    %% This is how OTP's test_server_sup:capture_get/1 expects to receive it
    case Cap of
        false -> ok;
        Pid when is_pid(Pid) -> Pid ! {captured, chars_to_string(Chars)}
    end,
    {ok, State}.

%% Handle list from guards - eqWAlizer sees [term()] not iodata()
-spec handle_put_chars_list([term()], state()) -> {ok, state()}.
handle_put_chars_list(Chars, State = #state{original_gl = GL, capture_pid = Cap}) ->
    %% Always forward to the original group leader
    GL ! {io_request, self(), make_ref(), {put_chars, unicode, Chars}},
    %% If capturing, send captured output to capture_pid's mailbox
    case Cap of
        false -> ok;
        Pid when is_pid(Pid) -> Pid ! {captured, chars_to_list_unsafe(Chars)}
    end,
    {ok, State}.

%% Handle list returned from apply - may not be strict iodata
-spec handle_put_chars_unsafe([term()], state()) -> {ok, state()}.
handle_put_chars_unsafe(Chars, State = #state{original_gl = GL, capture_pid = Cap}) ->
    %% Always forward to the original group leader
    GL ! {io_request, self(), make_ref(), {put_chars, unicode, Chars}},
    %% If capturing, send captured output to capture_pid's mailbox
    case Cap of
        false -> ok;
        Pid when is_pid(Pid) -> Pid ! {captured, chars_to_list_unsafe(Chars)}
    end,
    {ok, State}.

-spec chars_to_string(iodata()) -> string().
chars_to_string(Chars) ->
    case unicode:characters_to_list(Chars) of
        Result when is_list(Result) -> Result;
        _ ->
            try
                binary_to_list(iolist_to_binary(Chars))
            catch
                _:_ -> lists:flatten(io_lib:format("~tp", [Chars]))
            end
    end.

%% Handle potentially non-strict iodata from apply results or guarded lists
%% eqWAlizer sees [term()] from guards/apply, but at runtime it's iodata()
-spec chars_to_list_unsafe([term()]) -> string().
chars_to_list_unsafe(Chars) ->
    try
        %% Use iolist_to_binary to validate and convert iodata to binary
        % eqwalizer:ignore - Chars is iodata() at runtime, guard just provides [term()]
        Bin = iolist_to_binary(Chars),
        unicode:characters_to_list(Bin)
    of
        Result when is_list(Result) -> Result;
        _ -> lists:flatten(io_lib:format("~tp", [Chars]))
    catch
        _:_ -> lists:flatten(io_lib:format("~tp", [Chars]))
    end.

-spec handle_requests([term()], state(), term()) -> {term(), state()}.
handle_requests([], State, LastReply) ->
    {LastReply, State};
handle_requests([Req | Rest], State, _LastReply) ->
    case handle_io_request(Req, State) of
        {{error, _} = Err, NewState} -> {Err, NewState};
        {Reply, NewState} -> handle_requests(Rest, NewState, Reply)
    end.

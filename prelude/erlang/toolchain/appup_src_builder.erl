%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(appup_src_builder).
-moduledoc """
Build an .appup file from a given .appup.src template.

usage:
  appup_src_builder.escript appup_info.json output.appup

appup_info.json format:

 #{
     <<"name">>         := <application_name>,
     <<"template">>     := <path to an .appup.src file>,
     <<"version">>      := <version string>,
     <<"app_src_vsn">>  := <placeholder string used in the template's vsn field>
 }

The .appup.src template is expected to contain a single Erlang term of the form
`{Vsn, UpInstructions, DownInstructions}`. When `Vsn` matches the configured
placeholder (either as an atom or as a string), it is substituted with the
buck-supplied `version`. Otherwise it must already equal `version`.
""".
-compile(warn_missing_spec_all).

-export([main/1]).

-type filename() :: binary().
-type appup_info() :: #{
    name := binary(),
    template := filename(),
    version := binary(),
    app_src_vsn := binary()
}.

-spec main(Args) -> ok when Args :: [string()].
main([AppupInfoFile, Output]) ->
    try
        do(dependency_utils:chars_to_binary(AppupInfoFile), dependency_utils:chars_to_binary(Output))
    catch
        Type:{abort, Reason} ->
            io:format(standard_error, "~ts:~ts~n", [Type, Reason]),
            erlang:halt(1)
    end;
main(_) ->
    usage().

-spec usage() -> ok.
usage() ->
    io:format("appup_src_builder.escript appup_info.json output.appup~n").

-spec do(AppupInfoFile, Output) -> ok when
    AppupInfoFile :: filename(),
    Output :: filename().
do(AppupInfoFile, Output) ->
    #{
        name := Name,
        template := Template,
        version := Version,
        app_src_vsn := AppSrcVsn
    } = parse_appup_info(AppupInfoFile),
    Term = consult_template(Name, Template),
    Substituted = substitute_vsn(Name, Term, Version, AppSrcVsn),
    ToWrite = io_lib:format("~tp.~n", [Substituted]),
    ok = prim_file:write_file(Output, ToWrite).

-spec parse_appup_info(AppupInfoFile) -> appup_info() when AppupInfoFile :: filename().
parse_appup_info(AppupInfoFile) ->
    case prim_file:read_file(AppupInfoFile) of
        {ok, Content} ->
            case json:decode(Content) of
                #{
                    <<"name">> := Name,
                    <<"template">> := Template,
                    <<"version">> := Version,
                    <<"app_src_vsn">> := AppSrcVsn
                } ->
                    #{
                        name => Name,
                        template => Template,
                        version => Version,
                        app_src_vsn => AppSrcVsn
                    };
                Terms ->
                    file_corrupt_error(AppupInfoFile, Terms)
            end;
        Error ->
            open_file_error(AppupInfoFile, Error)
    end.

-spec consult_template(Name, TemplateFile) -> term() when
    Name :: binary(),
    TemplateFile :: filename().
consult_template(Name, TemplateFile) ->
    case file:consult(TemplateFile) of
        {ok, [Term]} -> Term;
        {ok, Terms} -> file_corrupt_error(TemplateFile, Terms);
        {error, Reason} -> consult_error(Name, TemplateFile, Reason)
    end.

-spec substitute_vsn(Name, Term, Version, AppSrcVsn) -> term() when
    Name :: binary(),
    Term :: term(),
    Version :: binary(),
    AppSrcVsn :: binary().
substitute_vsn(Name, {Vsn, UpInstructions, DownInstructions}, Version, AppSrcVsn) ->
    VersionStr = binary_to_list(Version),
    case is_app_src_vsn(Vsn, AppSrcVsn) orelse Vsn =:= VersionStr of
        true -> {VersionStr, UpInstructions, DownInstructions};
        false -> vsn_mismatch_error(Name, Vsn, Version, AppSrcVsn)
    end;
substitute_vsn(Name, Term, _Version, _AppSrcVsn) ->
    template_shape_error(Name, Term).

-spec is_app_src_vsn(Vsn, AppSrcVsn) -> boolean() when
    Vsn :: term(),
    AppSrcVsn :: binary().
is_app_src_vsn(Vsn, AppSrcVsn) when is_atom(Vsn) ->
    atom_to_binary(Vsn) =:= AppSrcVsn;
is_app_src_vsn(Vsn, AppSrcVsn) when is_list(Vsn) ->
    Vsn =:= binary_to_list(AppSrcVsn);
is_app_src_vsn(_, _) ->
    false.

-spec open_file_error(File, Error) -> no_return() when
    File :: filename(),
    Error :: term().
open_file_error(File, Error) ->
    Msg = io_lib:format("cannot open file ~ts: ~tp", [File, Error]),
    erlang:error({abort, Msg}).

-spec file_corrupt_error(File, Contents) -> no_return() when
    File :: filename(),
    Contents :: term().
file_corrupt_error(File, Contents) ->
    Msg = io_lib:format("corrupt information in ~ts: ~tp", [File, Contents]),
    erlang:error({abort, Msg}).

-spec consult_error(Name, File, Reason) -> no_return() when
    Name :: binary(),
    File :: filename(),
    Reason :: term().
consult_error(Name, File, Reason) ->
    Msg = io_lib:format(
        "error when building ~ts.appup for application ~ts: cannot read template ~ts: ~tp",
        [Name, Name, File, Reason]
    ),
    erlang:error({abort, Msg}).

-spec template_shape_error(Name, Term) -> no_return() when
    Name :: binary(),
    Term :: term().
template_shape_error(Name, Term) ->
    Msg = io_lib:format(
        "error when building ~ts.appup for application ~ts: expected template to be {Vsn, UpInstructions, DownInstructions}, got: ~tp",
        [Name, Name, Term]
    ),
    erlang:error({abort, Msg}).

-spec vsn_mismatch_error(Name, Vsn, Version, AppSrcVsn) -> no_return() when
    Name :: binary(),
    Vsn :: term(),
    Version :: binary(),
    AppSrcVsn :: binary().
vsn_mismatch_error(Name, Vsn, Version, AppSrcVsn) ->
    Msg = io_lib:format(
        "error when building ~ts.appup for application ~ts: template vsn (~tp) is neither the placeholder (~ts) nor the target version (~ts)",
        [Name, Name, Vsn, AppSrcVsn, Version]
    ),
    erlang:error({abort, Msg}).

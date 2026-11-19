%% Copyright (c) Meta Platforms, Inc. and affiliates.
%%
%% This source code is licensed under both the MIT license found in the
%% LICENSE-MIT file in the root directory of this source tree and the Apache
%% License, Version 2.0 found in the LICENSE-APACHE file in the root directory
%% of this source tree.

%% @format
-module(artifact_annotations).
-moduledoc """
This file acts as a manual erlang sync for the thrift type struct
TestResultArtifactAnnotations defined in  https://fburl.com/code/r2t4vclb

 == How To Update This File ==
We mostly expect next iterations of thrift data structure to include
more testArtifactTypes. Those should be manually added to the
test_artifact_type() here.
""".
-compile(warn_missing_spec_all).

%% Public API
-export([serialize/1, create_artifact_annotation/2, default_annotation/1, test_metrics_artifact_annotation/2]).
-export_type([
    annotation_function/0,
    test_result_artifact_annotations/0
]).

-import(common_util, [unicode_characters_to_binary/1]).

-include_lib("common/include/buck_ct_records.hrl").

-type generic_blob() :: #{generic_blob := #{}}.
-type generic_text_log() :: #{generic_text_log := #{}}.
-type test_artifact_type() :: generic_blob() | generic_text_log().

-type test_result_artifact_annotations() :: #{
    type := test_artifact_type(),
    description := binary(),
    % The `test_case` is matched against on the tpx side to attach artifacts related
    % to a particular `test_case`. If the `test_case` field is missing,
    % the artifact will be attached to all the test cases run.
    test_case => binary()
}.

-type annotation_function() :: fun((file:filename()) -> test_result_artifact_annotations()).

-spec serialize(test_result_artifact_annotations()) -> iodata().
serialize(ArtifactAnnotation) -> json:encode(ArtifactAnnotation).

-spec create_artifact_annotation(FileName, AnnotationFunction) -> test_result_artifact_annotations() when
    FileName :: file:filename(),
    AnnotationFunction :: annotation_function().
create_artifact_annotation(FileName, AnnotationFunction) ->
    AnnotationFunction(FileName).

-spec default_annotation(FileName :: file:filename()) -> test_result_artifact_annotations().
default_annotation(FileName) ->
    Type =
        case lists:member(filename:extension(FileName), [".json", ".html", ".log", ".spec", ".txt"]) of
            true -> #{generic_text_log => #{}};
            _ -> #{generic_blob => #{}}
        end,
    #{
        type => Type,
        description => unicode_characters_to_binary(FileName)
    }.

-spec test_metrics_artifact_annotation(file:filename(), nonempty_list(#ct_test{})) ->
    test_result_artifact_annotations().
test_metrics_artifact_annotation(FileName, Tests) ->
    FirstTest = hd(Tests),
    FirstTestName = unicode_characters_to_binary(
        common_util:qualified_name(
            lists:reverse(FirstTest#ct_test.groups),
            FirstTest#ct_test.test_name
        )
    ),
    % All tests share this artifact. Attach to first test only to prevent the Scuba uploader from uploading the same data multiple times.
    #{type => #{generic_blob => #{}}, description => list_to_binary(FileName), test_case => FirstTestName}.

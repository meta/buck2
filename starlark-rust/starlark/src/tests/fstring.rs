/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

mod pass {
    use crate::assert::Assert;
    use crate::syntax::Dialect;

    fn assert() -> Assert<'static> {
        let mut a = Assert::new();
        a.dialect(&Dialect::AllOptionsInternal);
        a
    }

    #[test]
    fn basic() {
        assert().is_true(
            r#"
x = "a"
f"{x}b" == "ab"
"#,
        );

        assert().is_true(
            r#"
x = "a"
f"{ x }b" == "ab"
"#,
        );

        assert().is_true(
            r#"
x = "a"
f"{x}b{x}" == "aba"
"#,
        );
    }

    #[test]
    fn escape() {
        assert().is_true(
            r#"
x = "a"
f"{{}}{x}b{{x}}" == "{}ab{x}"
"#,
        );
    }

    #[test]
    fn function_parameter() {
        assert().is_true(
            r#"
def f(x):
  return f"q{x}"

f("1") == "q1"
"#,
        );
    }

    #[test]
    fn multiple() {
        assert().is_true(
            r#"
x = "x"
y = "y"
f"{x}{y}" == "xy"
"#,
        );

        assert().is_true(
            r#"
x = "x"
y = "y"
f"{x}{y}{x}" == "xyx"
"#,
        );
    }

    #[test]
    fn tuple() {
        assert().is_true(
            r#"
x = ("x",)
f"{x}" == '("x",)'
"#,
        );
    }

    #[test]
    fn conv() {
        assert().is_true(r#"x = 'a'; f"{x}" == 'a'"#);
        assert().is_true(r#"x = 'a'; f"{x!s}" == 'a'"#);
        assert().is_true(r#"x = 'a'; f"{x!r}" == '"a"'"#);
    }

    #[test]
    fn arithmetic_expressions() {
        assert().is_true(r#"x = 2; y = 3; f"{x + y}" == "5""#);
        assert().is_true(r#"x = 10; y = 4; f"{x - y}" == "6""#);
        assert().is_true(r#"x = 3; y = 4; f"{x * y}" == "12""#);
        assert().is_true(r#"x = 15; y = 3; f"{x // y}" == "5""#);
        assert().is_true(r#"x = 17; y = 5; f"{x % y}" == "2""#);
        assert().is_true(r#"f"{2 + 3 * 4}" == "14""#);
        assert().is_true(r#"f"{(2 + 3) * 4}" == "20""#);
    }

    #[test]
    fn unary_operators() {
        assert().is_true(r#"x = 5; f"{-x}" == "-5""#);
        assert().is_true(r#"x = -3; f"{-x}" == "3""#);
    }

    #[test]
    fn comparison_expressions() {
        assert().is_true(r#"f"{1 < 2}" == "True""#);
        assert().is_true(r#"f"{2 > 1}" == "True""#);
        assert().is_true(r#"f"{1 == 1}" == "True""#);
        assert().is_true(r#"f"{1 != 2}" == "True""#);
        assert().is_true(r#"f"{1!=2}" == "True""#);
        assert().is_true(r#"f"{2 <= 2}" == "True""#);
        assert().is_true(r#"f"{3 >= 3}" == "True""#);
    }

    #[test]
    fn boolean_expressions() {
        assert().is_true(r#"f"{True and False}" == "False""#);
        assert().is_true(r#"f"{True or False}" == "True""#);
        assert().is_true(r#"f"{not False}" == "True""#);
        assert().is_true(r#"f"{True and True}" == "True""#);
    }

    #[test]
    fn conditional_expression() {
        assert().is_true(r#"x = True; f"{'yes' if x else 'no'}" == "yes""#);
        assert().is_true(r#"x = False; f"{'yes' if x else 'no'}" == "no""#);
        assert().is_true(r#"f"{1 if 2 > 1 else 0}" == "1""#);
    }

    #[test]
    fn function_calls() {
        assert().is_true(r#"f"{str(123)}" == "123""#);
        assert().is_true(r#"f"{len([1, 2, 3])}" == "3""#);
        assert().is_true(r#"f"{int('42')}" == "42""#);
        assert().is_true(r#"f"{repr('hello')}" == "\"hello\"""#);
        assert().is_true(r#"f"{max(1, 5, 3)}" == "5""#);
        assert().is_true(r#"f"{min(1, 5, 3)}" == "1""#);
    }

    #[test]
    fn method_calls() {
        assert().is_true(r#"s = "hello"; f"{s.upper()}" == "HELLO""#);
        assert().is_true(r#"s = "WORLD"; f"{s.lower()}" == "world""#);
        assert().is_true(r#"s = "hello world"; f"{s.replace('world', 'there')}" == "hello there""#);
        assert().is_true(r#"items = ["a", "b", "c"]; f"{','.join(items)}" == "a,b,c""#);
    }

    #[test]
    fn list_operations() {
        assert().is_true(r#"arr = [1, 2, 3]; f"{arr[0]}" == "1""#);
        assert().is_true(r#"arr = [1, 2, 3]; f"{arr[-1]}" == "3""#);
        assert().is_true(r#"arr = [1, 2, 3, 4, 5]; f"{arr[1:3]}" == "[2, 3]""#);
        assert().is_true(r#"arr = [1, 2, 3, 4, 5]; f"{arr[::2]}" == "[1, 3, 5]""#);
        assert().is_true(r#"f"{[1, 2, 3]}" == "[1, 2, 3]""#);
        assert().is_true(r#"f"{len([1, 2, 3])}" == "3""#);
        assert().is_true(r#"bar = [0, 123, 456]; f"foo {bar[1]}" == "foo 123""#);
    }

    #[test]
    fn dict_operations() {
        assert().is_true(r#"d = {"a": 1}; f"{d['a']}" == "1""#);
        assert().is_true(r#"d = {"key": "value"}; f"{d['key']}" == "value""#);
        assert().is_true(r#"f"{str({'a': 1} )}" == '{"a": 1}'"#);
    }

    #[test]
    fn same_quote_in_expression() {
        // PEP 701 style: same quote type inside expression
        assert().is_true(r#"d = {"x": 42}; f"{d["x"]}" == "42""#);
        assert().is_true(r#"d = {'y': 99}; f'{d['y']}' == "99""#);
    }

    #[test]
    fn attribute_access() {
        // Using a struct-like value
        assert().is_true(
            r#"
def make_obj():
    return struct(name="test", value=42)
obj = make_obj()
f"{obj.name}" == "test"
"#,
        );
        assert().is_true(
            r#"
def make_obj():
    return struct(nested=struct(x=1))
obj = make_obj()
f"{obj.nested.x}" == "1"
"#,
        );
    }

    #[test]
    fn nested_function_calls() {
        assert().is_true(r#"f"{str(len([1, 2, 3]))}" == "3""#);
        assert().is_true(r#"f"{int(str(42))}" == "42""#);
    }

    #[test]
    fn complex_expressions() {
        assert().is_true(r#"x = 5; y = 3; f"result: {x * y + 2}" == "result: 17""#);
        assert()
            .is_true(r#"items = [1, 2, 3]; f"sum: {items[0] + items[1] + items[2]}" == "sum: 6""#);
        assert().is_true(r#"f"nested: {str(1 + 2)}" == "nested: 3""#);
    }

    #[test]
    fn multiple_expressions_in_string() {
        assert().is_true(r#"a = 1; b = 2; c = 3; f"{a} + {b} = {c}" == "1 + 2 = 3""#);
        assert().is_true(r#"x = "hello"; y = "world"; f"{x}, {y}!" == "hello, world!""#);
        assert()
            .is_true(r#"f"start {1} middle {2} end {3} done" == "start 1 middle 2 end 3 done""#);
    }

    #[test]
    fn empty_and_text_only() {
        assert().is_true(r#"f"" == """#);
        assert().is_true(r#"f'' == ''"#);
        assert().is_true(r#"f"just text" == "just text""#);
        assert().is_true(r#"f'no expressions' == 'no expressions'"#);
    }

    #[test]
    fn escaped_braces() {
        assert().is_true(r#"f"{{" == "{""#);
        assert().is_true(r#"f"}}" == "}""#);
        assert().is_true(r#"f"{{}}" == "{}""#);
        assert().is_true(r#"f"{{literal}}" == "{literal}""#);
        assert().is_true(r#"x = 1; f"{x} {{escaped}} {x}" == "1 {escaped} 1""#);
    }

    #[test]
    fn whitespace_in_expressions() {
        assert().is_true(r#"x = 5; f"{ x }" == "5""#);
        assert().is_true(r#"x = 2; y = 3; f"{  x + y  }" == "5""#);
        assert().is_true(r#"f"{   1   }" == "1""#);
    }

    #[test]
    fn list_comprehension() {
        assert().is_true(r#"f"{[x * 2 for x in [1, 2, 3]]}" == "[2, 4, 6]""#);
        assert().is_true(r#"f"{[x for x in range(3)]}" == "[0, 1, 2]""#);
        assert().is_true(r#"f"{[str(x) for x in [1, 2]]}" == '["1", "2"]'"#);
    }

    #[test]
    fn membership_test() {
        assert().is_true(r#"f"{1 in [1, 2, 3]}" == "True""#);
        assert().is_true(r#"f"{4 in [1, 2, 3]}" == "False""#);
        assert().is_true(r#"f"{'a' in 'abc'}" == "True""#);
    }

    #[test]
    fn string_concatenation_in_expr() {
        assert().is_true(r#"f"{'hello' + ' ' + 'world'}" == "hello world""#);
        assert().is_true(r#"f"{'ab' * 3}" == "ababab""#);
    }

    #[test]
    fn raw_fstring() {
        assert().is_true(r#"x = 1; fr"{x}\n" == "1\\n""#);
        assert().is_true(r#"x = 2; fr'{x}\t' == "2\\t""#);
    }

    #[test]
    fn triple_quoted_fstring() {
        assert().is_true(
            r#"
x = "test"
f"""{x}""" == "test"
"#,
        );
        assert().is_true(
            r#"
x = 1
y = 2
f"""
{x}
{y}
""" == "\n1\n2\n"
"#,
        );
    }

    #[test]
    fn conversion_with_expressions() {
        assert().is_true(r#"f"{1 + 2!s}" == "3""#);
        assert().is_true(r#"f"{'hello'!r}" == "\"hello\"""#);
        assert().is_true(r#"items = [1, 2]; f"{items!r}" == "[1, 2]""#);
    }

    #[test]
    fn chained_method_calls() {
        assert().is_true(r#"s = "  hello  "; f"{s.strip().upper()}" == "HELLO""#);
        assert().is_true(r#"s = "hello-world"; f"{s.replace('-', ' ').title()}" == "Hello World""#);
    }

    #[test]
    fn range_in_expression() {
        assert().is_true(r#"f"{list(range(3))}" == "[0, 1, 2]""#);
        assert().is_true(r#"f"{list(range(1, 4))}" == "[1, 2, 3]""#);
        assert().is_true(r#"f"{list(range(0, 10, 2))}" == "[0, 2, 4, 6, 8]""#);
    }
}

mod fail {
    use starlark_syntax::golden_test_template::golden_test_template;

    use crate::assert::Assert;
    use crate::syntax::Dialect;

    fn fstring_golden_test_with_dialect(test_name: &str, text: &str, dialect: &Dialect) {
        let mut a = Assert::new();
        a.dialect(dialect);

        let err = a.fails(text, &[]);

        golden_test_template(
            &format!("src/tests/fstring/golden/{test_name}.err.golden.md"),
            &format!("{err}"),
        );
    }

    fn fstring_golden_test(test_name: &str, text: &str) {
        fstring_golden_test_with_dialect(test_name, text, &Dialect::AllOptionsInternal);
    }

    #[test]
    fn undeclared_variable() {
        fstring_golden_test("undeclared_variable", "f'foo {bar}'");
    }

    #[test]
    fn invalid_expression() {
        fstring_golden_test("invalid_expression", "f'foo {bar baz}'");
    }

    #[test]
    fn invalid_expression_triple_quotes() {
        fstring_golden_test("invalid_expression_triple_quotes", "f'''foo {bar baz}'''");
    }

    #[test]
    fn invalid_expression_raw() {
        fstring_golden_test("invalid_expression_raw", "fr'foo {bar baz}'");
    }

    #[test]
    fn invalid_expression_multiline() {
        fstring_golden_test("invalid_expression_multiline", "f''''foo \n {bar baz}'''");
    }

    #[test]
    fn invalid_format() {
        fstring_golden_test("invalid_format", "f'foo {bar'");
    }

    #[test]
    fn escape() {
        // NOTE: this is wrong, we put the squiggly lines in the wrong place.
        fstring_golden_test("escape", "f'foo \\n {bar baz}'");
    }

    #[test]
    fn not_enabled() {
        fstring_golden_test_with_dialect("not_enabled", "f'{foo}'", &Dialect::Standard);
    }

    #[test]
    fn invalid_conversion_missing() {
        fstring_golden_test("invalid_conversion_missing", "f'{x!}'");
    }

    #[test]
    fn invalid_conversion_non_identifier() {
        fstring_golden_test("invalid_conversion_non_identifier", "f'{x!1}'");
    }
}

# buck2 core tests

The primary integration tests for buck2.

The best way to write new tests is to copy-paste from an existing one and modify it.

## Testing guidelines

- Make sure you are clear to yourself what the tests you are writing are verifying. Write them to
  actually verify that and not something else.
- Make sure future readers of your test can figure out what your test is verifying.
- Keep this directory organized around what the tests are verifying. Someone looking for tests of
  behavior X should have a reasonable shot at finding such tests from the directory structure alone.
  Copy pasting fixtures from another test is endorsed, and preferred over having a test in the wrong
  place or having two unrelated tests reuse each others fixtures, making each hard to change.
- Golden tests, particularly for error messages, are good.
- Fixtures get the nano prelude (`e2e_util/nano_prelude`), never the real one: these are tests of
  buck2 itself, and the harness rejects any other bundled cell. A test that needs prelude code is
  not a core test and goes in `e2e/`.

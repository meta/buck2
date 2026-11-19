# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import re
from dataclasses import dataclass
from typing import Dict, List, Optional, Tuple

"""
A few simple helpers that allow us to use simple markup to
point to ranges in a file. This is especially useful in the
LSP tests where we want to make sure that clicking on a link
will go to a specific location. It also helps make building
those structures easier.

See `Fixture` for the markkup format.
"""


@dataclass
class Span:
    """A span within a template file"""

    start_line: int
    start_col: int
    end_line: int
    end_col: int


class Lines:
    """
    Helper class to take a multiline string, and find the line and
    column based on the string index
    """

    lines: List[Tuple[int, int]]

    def __init__(self, content: str) -> None:
        lines = []
        idx = 0
        for line in content.splitlines(keepends=True):
            new_idx = idx + len(line)
            lines.append((idx, new_idx))
            idx = new_idx
        lines.append((idx, len(content)))

        self.lines = lines

    def find_range(self, start_pos: int, end_pos: int) -> Span:
        def pos_in_line(line_start: int, line_end: int, pos: int) -> Optional[int]:
            if pos < line_start or pos >= line_end:
                return None
            else:
                return pos - line_start

        start_line = None
        start_col = None
        end_line = None
        end_col = None

        for lineno, (line_start, line_end) in enumerate(self.lines):
            s_col = pos_in_line(line_start, line_end, start_pos)
            e_col = pos_in_line(line_start, line_end, end_pos)
            if s_col is not None:
                start_line = lineno
                start_col = s_col
            if e_col is not None:
                end_line = lineno
                end_col = e_col

        if (
            start_line is not None
            and start_col is not None
            and end_line is not None
            and end_col is not None
        ):
            return Span(
                start_line=start_line,
                start_col=start_col,
                end_line=end_line,
                end_col=end_col,
            )
        else:
            raise ValueError(
                f"Could not find range for position `{start_pos}` and `{end_pos}`"
            )


"""
Representation of a template file with named spans to make things
like clicking and getting a range back in e2e LSP tests easier

Example template file:
```
def <foo>f<f>o</f>o</foo>(x: "string"):
    pass
```

Would yield "new_content" of:
```
def foo(x: "string"):
    pass
```

and spans of {"foo": Span(1, 4, 1, 7), "f": Span(1, 5, 1, 6)}
"""


@dataclass
class Fixture:
    original_content: str
    content: str
    spans: Dict[str, Span]

    def __init__(self, content: str) -> None:
        raw_positions = {}
        new_content = ""
        idx = 0
        regex = re.compile(r"<(/)?(\w[-\w_]*)>")
        while idx < len(content):
            m = regex.search(content, idx)
            if m:
                new_content += content[idx : m.start()]
                idx = m.end()

                identifier = m.group(2)
                if m.group(1):
                    # Closing tag. Must have seen an open tag
                    if identifier not in raw_positions:
                        raise ValueError(
                            f"Found closing, but not starting tag, for `{identifier}`"
                        )
                    existing = raw_positions[identifier]
                    if existing[1] is not None:
                        raise ValueError(
                            f"Found duplicate closing tags for `{identifier}`"
                        )
                    raw_positions[identifier] = (existing[0], len(new_content))
                else:
                    # Open tag. Duplicates are not allowed.
                    if identifier in raw_positions:
                        raise ValueError(f"Duplicate span `{identifier}`")
                    raw_positions[identifier] = (len(new_content), None)
            else:
                break
        new_content += content[idx:]

        lines = Lines(new_content)
        spans = {
            identifier: lines.find_range(start, end)
            for (identifier, (start, end)) in raw_positions.items()
        }

        self.original_content = content
        self.content = new_content
        self.spans = spans

    def start_line(self, id: "str") -> int:
        return self.spans[id].start_line

    def start_col(self, id: "str") -> int:
        return self.spans[id].start_col

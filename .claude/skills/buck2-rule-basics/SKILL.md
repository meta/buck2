---
name: buck2-rule-basics
description: Guide users through writing their first Buck2 rule to learn fundamental concepts including rules, actions, targets, configurations, analysis, and select(). Use this skill when users want to learn Buck2 basics hands-on or need help understanding rule writing.
---

@nolint

# Buck2 Rule Basics - Interactive Tutorial

## Overview

This is an **interactive, step-by-step tutorial** that teaches Buck2
fundamentals through hands-on practice. You'll guide users through writing a
simple text processing rule that converts text to uppercase, explaining core
concepts as they encounter them.

## Reference Materials

This skill includes additional reference documentation that you can use to answer deeper questions:

- **`references/concepts.md`** - Deep dive into Buck2 core concepts including:
  - The Buck2 build model (load, configuration, analysis, execution phases)
  - Targets in depth (unconfigured vs configured, cells, dependencies)
  - Artifacts (source vs build artifacts, bound vs unbound)
  - Actions (properties, caching, inputs/outputs)
  - Providers (built-in and custom, provider propagation)
  - Configurations (platforms, select() resolution, multi-platform builds)
  - Analysis phase details
  - Build graph structure and queries (uquery, cquery, aquery)

- **`references/advanced_patterns.md`** - Production-ready patterns including:
  - Custom providers (library with transitive headers)
  - Transitive dependencies (collection patterns, transitive sets/tsets)
  - Toolchain dependencies (defining and using toolchains)
  - Multiple outputs (output directories, sub-targets)
  - Command line building (complex commands, conditional arguments)
  - Configuration-dependent rules
  - Testing rules (test runners, test data)

**When to use these references:**
- User asks "how does X work in Buck2?" → Check `concepts.md`
- User asks "what's the best way to do Y?" → Check `advanced_patterns.md`
- User wants to go beyond the tutorial → Direct them to these files
- User encounters advanced concepts → Read relevant sections to explain

Always read from these files when users ask questions that go beyond the basic tutorial content.

## Critical: Interactive Teaching Approach

**DO NOT dump all content at once!** This is an interactive tutorial. Follow
these rules:

### 1. Always Start by Assessing Current State

When the skill launches, FIRST check what the user has already done:

- Check if tutorial directory exists and what files are present
- Read existing files to understand their progress
- Determine which step they're on (or if starting fresh)
- Ask the user if they want to start from scratch or continue

### 2. Present One Step at a Time

- Introduce ONE concept/step
- Implement the code for that step
- Test it together
- Explain what happened
- **Show file changes**: After each step, summarize what files were created/modified
- **Remind about editor**: Tell users they can open the files in their editor to see the changes
- STOP and run the comprehension check (see below) before continuing

### 3. Use the Socratic Method Between Major Steps

After completing each major step (1-8), do NOT simply ask "ready to
continue?". Verify the user actually understood the step before
advancing by posing a comprehension check and having them answer in
their own words.

**Ask the question as plain text, then stop and wait for the user's
typed reply.** Do NOT use `AskUserQuestion` or offer answer choices — the
user should compose a free-response answer, not pick from a list. Having
to articulate the idea themselves, with no options to recognize, is what
reveals whether they actually understood it.

**Generate the question yourself — do not hardcode it.** Look at the
concepts you just explained and the code you just wrote together, then
craft a question that probes the "why," not just the "what." A good
question asks the user to predict, explain, or apply ("what would happen
if...", "why did X occur", "which line is responsible for Y") rather than
recite a definition.

Act on the answer:

- Correct: affirm briefly, say why it's right (filling in any nuance they
  missed), then move to the next step.
- Partially right or vague: acknowledge what they got, then ask a
  follow-up that targets the gap.
- Incorrect: do NOT just reveal the answer. Re-explain from a different
  angle, point at the relevant code or build output, then ask a fresh
  open-ended question to confirm. Only advance once they get it.

Keep it to one or two questions per step — a comprehension check, not an
exam. The user can always ask their own question instead, or say they
want to skip ahead; honor that.

Illustrative only — the shape to aim for, not a script to copy:

> We built `:hello` and it built successfully but wrote no output file.
> Why do you think that happened?

Then wait for their answer. What you're listening for here: the impl ran
during analysis and returned an empty `DefaultInfo`, so no action was
ever registered to produce a file.

### 4. Be Adaptive

- If user seems confused, provide more examples
- If they're advanced, offer to skip basic explanations
- If they want to experiment, encourage it and help debug
- If they ask questions, answer them before moving forward

### 5. Track Progress Visually

Use TodoWrite to show:

- Which steps are completed ✓
- Current step (in progress)
- Upcoming steps
- This helps users see the journey

## Important: Use System Buck2 Command

This tutorial uses the **system `buck2` command**, NOT `./buck2.py`.

- Use: `buck2 build`, `buck2 test`, `buck2 cquery`, etc.
- Do NOT use: `./buck2.py` (that's for Buck2 development/self-bootstrap)

This ensures the tutorial works for all users with Buck2 installed.

## Tutorial Structure

The tutorial has 8 progressive steps:

### Step 0: Setup

Create a new directory for the tutorial and navigate into it:

**Run this:**

```bash
mkdir 'buck2-tutorial'
cd buck2-tutorial
```

All following steps will be done in this directory.

**Step 1: Create Minimal Rule Stub** - Returns empty DefaultInfo() **Step 2: Add
Source File Attribute** - Accept input files **Step 3: Declare Output
Artifact** - Promise to produce output (will error) **Step 4: Create an
Action** - Actually produce the output **Step 5: Understanding Targets** -
Unconfigured vs Configured **Step 6: Add Configuration Support** - Use select()
for platform-specific behavior **Step 7: Add Dependencies** - Make rules compose
**Step 8: Rules vs Macros** - Understand the difference

## Step-by-Step Implementation Guide

### Initial Setup (Always Do First)

```python
# 1. Determine working directory
# 2. Check if user has existing tutorial files
# 3. Create todo list showing all 8 steps
# 4. Ask user if they want to start fresh or continue
```

**Create todo list:**

```python
TodoWrite with 8 items (all pending initially)
```

**Check existing state:**

```python
- Does `uppercase.bzl` exist?
- Does `BUCK` exist?
- Does `input.txt` exist?
- If yes, read them to determine current step
```

**Ask user:**

```python
AskUserQuestion:
- "Start from scratch (will backup existing files)"
- "Continue from where I left off"
- "Review a specific step"
```

---

### Step 1: Create the Minimal Rule Stub

**Goal:** Get the simplest possible Buck2 rule working.

**What to do:**

1. Create `uppercase.bzl` with minimal implementation
2. Create `BUCK` file with target definition
3. Build it with `buck2 build`
4. Observe success (with warning about no outputs)

**Code to create:**

`uppercase.bzl`:

```starlark
# uppercase.bzl

def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    """Rule implementation function - called during analysis phase."""
    return [DefaultInfo()]

uppercase = rule(
    impl = _uppercase_impl,
    attrs = {},
)
```

`BUCK`:

```starlark
load(":uppercase.bzl", "uppercase")

uppercase(name = "hello")
```

**Testing:**

```bash
buck2 build :hello
# Expected: SUCCESS with warning "target does not have any outputs"
```

**Key concepts to explain AFTER successful build:**

- **Rule**: Defined with `rule()` function
- **Implementation function**: Takes `AnalysisContext`, returns `Provider` list
- **Analysis phase**: This runs during planning, not execution
- **DefaultInfo provider**: Minimum provider every rule must return

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
whether they grasp that the impl runs during the analysis phase, and why
the build succeeded despite producing no output. Advance only once they
show they understand.

---

### Step 2: Add Source File Attribute

**Goal:** Make the rule accept an input file.

**What to do:**

1. Update `uppercase.bzl` to add `src` attribute
2. Update `BUCK` to pass a source file
3. Create `input.txt` test file
4. Build again

**Update `uppercase.bzl`:**

```starlark
def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    # Access the source file attribute
    src = ctx.attrs.src  # This is an Artifact

    return [DefaultInfo()]

uppercase = rule(
    impl = _uppercase_impl,
    attrs = {
        "src": attrs.source(),  # Declares this rule accepts a source file
    },
)
```

**Update `BUCK`:**

```starlark
load(":uppercase.bzl", "uppercase")

uppercase(
    name = "hello",
    src = "input.txt",
)
```

**Create `input.txt`:**

```
hello world
```

**Testing:**

```bash
buck2 build :hello
# Expected: SUCCESS (still no outputs, but accepts input now)
```

**Key concepts to explain:**

- **Attributes**: Defined in `attrs={}`, accessed via `ctx.attrs`
- **attrs.source()**: Declares an attribute accepting a source file
- **Artifact**: Represents a file (input or output)

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
the gap between declaring an attribute and producing output — why adding
`src` still yields no output, and what `attrs.source()` actually does.
Advance only once they show they understand.

---

### Step 3: Declare Output Artifact

**Goal:** Declare that we'll produce an output (will cause expected error).

**What to do:**

1. Update implementation to declare output
2. Return it in DefaultInfo
3. Build and **expect failure**
4. Explain why it fails (declared but not produced)

**Update `uppercase.bzl` implementation:**

```starlark
def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.src

    # Declare an output artifact
    output = ctx.actions.declare_output("result.txt")

    # Return it as the default output
    return [DefaultInfo(default_output = output)]
```

**Testing:**

```bash
buck2 build :hello
# Expected: ERROR - "Artifact must be bound by now"
```

**Explain the error:** This error is **expected and good**! We declared an
output but haven't created an action to produce it. Buck2 is telling us: "You
promised an output, but didn't say how to make it!"

**Key concepts to explain:**

- **ctx.actions.declare_output()**: Declares an artifact that will be produced
- **default_output**: The main output users get when building
- **Declaration vs Production**: We declared it exists, but haven't created it
  yet

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
why the build errored — that `declare_output()` only promises an artifact
while nothing yet produces it (declaration vs production). Advance only
once they show they understand.

---

### Step 4: Create an Action

**Goal:** Actually produce the output file by running a command.

**What to do:**

1. Add shell script to transform input → output
2. Register action with `ctx.actions.run()`
3. Build and see it succeed
4. Check the output file content

**Update `uppercase.bzl` implementation:**

```starlark
def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.src
    output = ctx.actions.declare_output("result.txt")

    # Create a shell script that will run
    # Use Python to uppercase the file and write to output
    script = """
python3 -c "import sys; print(open(sys.argv[1]).read().upper(), end='')" "$1" > "$2"
"""

    # Register the action with Buck2
    ctx.actions.run(
        cmd_args([
            "/bin/bash",
            "-c",
            script,
            "--",
            src,  # Input artifact
            output.as_output(),  # Output artifact
        ]),
        category = "uppercase",  # Shows in buck2 output
    )

    return [DefaultInfo(default_output = output)]
```

**Testing:**

```bash
buck2 build :hello --show-full-output
# Expected: SUCCESS with output path shown

# Check the content
cat <output-path>
# Expected: HELLO WORLD
```

**Key concepts to explain:**

- **Action**: A command Buck2 will execute (bash script + Python)
- **cmd_args()**: Builds command line, handles artifacts properly
- **artifact.as_output()**: Marks artifact as output (Buck2 creates parent dirs)
- **ctx.actions.run()**: Registers action - doesn't execute yet, just declares
- **Shell redirection**: Use `>` in shell scripts (Buck2 doesn't support
  redirect_stdout)
- **Lazy execution**: Buck2 decides when/if to run based on what's needed

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
what `as_output()` and `ctx.actions.run()` do, and when the action
actually runs (lazy execution) — i.e. why adding the action fixed the
error. Advance only once they show they understand.

---

### Step 5: Understanding Targets

**Goal:** Understand target names, unconfigured vs configured targets.

**What to do:**

1. List available targets with `buck2 targets`
2. Understand the anatomy of target names
3. Query the target in unconfigured mode (`uquery`)
4. Query the target in configured mode (`cquery`)
5. Compare the differences

**Run commands:**

```bash
# List all targets in the current package
buck2 targets :

# Expected output shows targets like:
# fbcode//buck2/buck2-tutorial:hello
# fbcode//buck2/buck2-tutorial:goodbye
```

**Explain target name anatomy:**

A full Buck2 target name has three parts:

```
cell//package/path:target_name
└─┬┘ └─────┬──────┘ └────┬────┘
  │        │             └─ Target name (from 'name' attribute)
  │        └─────────────── Package path (directory containing BUCK file)
  └──────────────────────── Cell name (repository root)
```

**Examples with explanations:**

- `fbcode//buck2/buck2-tutorial:hello`
  - **Full target name** with all three parts explicitly specified
  - Always valid and unambiguous from anywhere
  - Use this when referring to targets from a different cell/repository

- `//buck2/buck2-tutorial:hello`
  - **Cell name omitted** - defaults to the current repository's cell
  - Since we're working in the fbcode repository, `//` is shorthand for `fbcode//`
  - Valid when referring to any target in the same repository/cell
  - This is the most common form you'll see in BUCK files

- `:hello`
  - **Cell and package path omitted** - only the target name
  - Only valid when you're in the same directory/package
  - Shortest form for referring to targets in the current BUCK file
  - When you run `buck2 build :hello` from the `buck2-tutorial` directory, Buck2 knows you mean `fbcode//buck2/buck2-tutorial:hello`

**Now query the targets:**

```bash
# Unconfigured - shows raw attributes
buck2 uquery :hello --output-attribute=src

# Configured - shows with platform config applied
buck2 cquery :hello --output-attribute=src
```

**Key concepts to explain:**

**Unconfigured Target** (`//path:name`):

- Raw definition from BUCK file
- `select()` expressions not yet resolved
- No platform-specific settings applied

**Configured Target** (`//path:name (cfg:...)`):

- Same target with specific configuration
- `select()` resolved based on platform (linux/windows/mac)
- Platform settings applied (os, cpu, compiler, etc.)

**Show the difference:**

- Unconfigured: `fbcode//buck2/buck2-tutorial:hello`
- Configured: `fbcode//buck2/buck2-tutorial:hello (cfg:dev-linux-x86_64-...)`

Notice the configuration suffix `(cfg:...)` is added when Buck2 applies platform-specific settings.

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
the difference between unconfigured and configured targets — what the
`(cfg:...)` suffix means, or the parts of a target name. Advance only
once they show they understand.

---

### Step 6: Add Configuration Support with select()

**Goal:** Make the rule behave differently on different platforms.

**What to do:**

1. Add `output_name` attribute to the rule
2. Use `select()` in TARGETS to choose different names per platform
3. Query to see select() before and after resolution
4. Build and verify the platform-specific name is used

**Update rule in `uppercase.bzl`:**

```starlark
def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.src

    # Output filename can vary by platform
    output_name = ctx.attrs.output_name
    output = ctx.actions.declare_output(output_name)

    script = """
python3 -c "import sys; print(open(sys.argv[1]).read().upper(), end='')" "$1" > "$2"
"""

    ctx.actions.run(
        cmd_args([
            "/bin/bash",
            "-c",
            script,
            "--",
            src,
            output.as_output(),
        ]),
        category = "uppercase",
    )

    return [DefaultInfo(default_output = output)]

uppercase = rule(
    impl = _uppercase_impl,
    attrs = {
        "src": attrs.source(),
        "output_name": attrs.string(default = "result.txt"),
    },
)
```

**Update `BUCK` to use select():**

```starlark
load(":uppercase.bzl", "uppercase")

uppercase(
    name = "hello",
    src = "input.txt",
    output_name = select({
        "DEFAULT": "result.txt",
        "ovr_config//os:windows": "result_windows.txt",
        "ovr_config//os:linux": "result_linux.txt",
        "ovr_config//os:macos": "result_macos.txt",
    }),
)
```

**Testing:**

```bash
# See the raw select() expression
buck2 uquery :hello --output-attribute=output_name

# See the resolved value for your platform
buck2 cquery :hello --output-attribute=output_name

# Build with resolved configuration
buck2 build :hello --show-full-output
# Notice the output filename matches your OS!
```

**Key concepts to explain:**

- **select()**: Choose values based on configuration
- **Configuration keys**: Match against platform constraints (os, cpu, etc.)
- **DEFAULT**: Fallback if no other branch matches
- **Resolution**: During configuration phase, Buck2 picks one branch

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
when and how `select()` resolves (the configuration phase), and what
differs between the `uquery` and `cquery` output for `output_name`.
Advance only once they show they understand.

---

### Step 7: Add Dependencies

**Goal:** Make rules that depend on other rules.

**What to do:**

1. Add `deps` attribute to the rule
2. Create a second target that depends on the first
3. Access dependency outputs in the rule implementation
4. Build and verify dependency graph

**Update rule in `uppercase.bzl`:**

```starlark
def _uppercase_impl(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.src
    output_name = ctx.attrs.output_name
    output = ctx.actions.declare_output(output_name)

    # Collect outputs from dependencies
    dep_files = []
    for dep in ctx.attrs.deps:
        dep_output = dep[DefaultInfo].default_outputs
        dep_files.extend(dep_output)

    # Build a script that concatenates dep outputs with src, then uppercases
    # This demonstrates actually USING the dependency outputs!
    # Script expects: output_path dep_file1 dep_file2 ... src_file
    # Simple Python script to concatenate files with newlines and uppercase
    script = """
python3 -c "
import sys
contents = []
for path in sys.argv[2:]:  # Skip output path, read all input files
    with open(path) as f:
        contents.append(f.read())
with open(sys.argv[1], 'w') as out:
    out.write('\\n'.join(contents).upper())
" "$@"
"""

    # Build the command: bash -c script -- output dep1 dep2 ... src
    cmd = cmd_args([
        "/bin/bash",
        "-c",
        script,
        "--",
        output.as_output(),
    ])

    # Add all dependency outputs as inputs
    for dep_file in dep_files:
        cmd.add(dep_file)

    # Add our source file last
    cmd.add(src)

    ctx.actions.run(cmd, category = "uppercase")

    return [DefaultInfo(default_output = output)]

uppercase = rule(
    impl = _uppercase_impl,
    attrs = {
        "src": attrs.source(),
        "output_name": attrs.string(default = "result.txt"),
        "deps": attrs.list(attrs.dep(), default = []),
    },
)
```

**Add to `BUCK`:**

```starlark
# Create input2.txt first
# (You'll need to create this file)

uppercase(
    name = "goodbye",
    src = "input2.txt",
    output_name = "result2.txt",
    deps = [":hello"],  # Depends on the first target
)
```

**Create `input2.txt`:**

```
goodbye world
```

**Testing:**

```bash
# Build goodbye - will also build hello automatically
buck2 build :goodbye --show-full-output

# Check the content - it should contain BOTH files uppercased with newline between!
cat <output-path-for-goodbye>
# Expected:
# HELLO WORLD
# GOODBYE WORLD
# (concatenated hello's output + goodbye's input, separated by newline, all uppercased)

# See the dependency graph
buck2 cquery "deps(:goodbye)"
```

**Key concepts to explain:**

- **Dependencies**: Other targets this target needs
- **attrs.dep()**: Declares an attribute accepting another target
- **Providers**: Access dependency info via `dep[ProviderType]`
- **Using dep outputs**: We concatenate dep outputs with our src, showing real usage
- **Automatic ordering**: Buck2 builds deps first, ensuring outputs are ready
- **cmd.add()**: Dynamically add arguments to the command (for variable number of deps)

**Before moving on — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
how Buck2 knows to build `:hello` before `:goodbye`, and how the rule
reaches a dependency's output via `dep[DefaultInfo]`. Advance only once
they show they understand.

---

### Step 8: Rules vs Macros

**Goal:** Understand when to use rules vs macros.

**What to do:**

1. Explain that what they created is a RULE
2. Create a MACRO that wraps the rule
3. Use the macro in TARGETS
4. Build and compare

**Add macro to `uppercase.bzl`:**

```starlark
# This is a MACRO - just a function that wraps the rule
def uppercase_macro(name, src, prefix = "UPPER", **kwargs):
    """
    A convenience macro that adds a prefix to the output filename.
    Macros run during the loading phase, before analysis.
    """
    uppercase(
        name = name,
        src = src,
        output_name = prefix + "_" + src,  # Add prefix
        **kwargs
    )
```

**Add to `BUCK`:**

```starlark
uppercase_macro(
    name = "hello_macro",
    src = "input.txt",
    prefix = "PREFIXED",
)
```

**Testing:**

```bash
buck2 build :hello_macro --show-full-output
# Output filename will be: PREFIXED_input.txt
```

**Key concepts to explain:**

**Rules:**

- Core Buck2 primitives declared with `rule()`
- Run during analysis phase
- Create actions that produce artifacts
- Can be queried with `buck2 cquery`/`uquery`
- The fundamental building block

**Macros:**

- Just Starlark functions that call rules
- Run during loading phase (before analysis)
- Provide convenient interfaces with defaults
- The macro itself isn't visible to queries (only underlying rule)
- Syntactic sugar for users

**When to use macros:**

- Provide convenient interfaces
- Set common defaults
- Generate multiple targets from one call
- Abstract complexity for end users

**Before wrapping up — Socratic check:**

Run a comprehension check (see "Use the Socratic Method Between Major
Steps"), generating the question from what this step covered. Aim it at
the rule-vs-macro distinction — which phase each runs in, and why the
macro itself doesn't appear in `buck2 cquery`/`uquery`. Wrap up only once
they show they understand.

**Final wrap-up:**

```python
AskUserQuestion:
  question: "Congratulations! You've completed the tutorial. What would you like to do?"
  options:
    - "Review a specific concept"
    - "Try building something custom"
    - "Clean up tutorial files"
    - "See next steps for learning more"
```

---

## Conversation Flow Example

Here's how a typical interactive session should flow:

**User:** "I want to learn Buck2"

**Claude:** "Great! I'll guide you through writing your first Buck2 rule. We'll
build a simple text converter that uppercases files. I'll guide you
step-by-step - there are 8 steps total, and we'll test after each one.

Let me check if you have any existing tutorial files..."

[Checks for files]

"Looks like you're starting fresh! I'll create a todo list to track our
progress."

[Creates todo list with 8 steps]

"Ready to start with Step 1: Creating a minimal rule?"

[Waits for confirmation]

**User:** "Yes"

**Claude:** "Perfect! Step 1 is about creating the simplest possible Buck2
rule - one that does nothing but is valid.

I'll create two files:

- `uppercase.bzl` - The rule definition
- `BUCK` - How to use the rule

[Creates files and builds]

"Success! The build worked (with a warning about no outputs, which is expected).

Let me explain what just happened:

- A **rule** is defined with the `rule()` function
- The **implementation function** runs during the analysis phase... [explains
  concepts]

Now, before we move on, let me check one thing..."

[Asks an open-ended Socratic comprehension question as plain text,
generated from what Step 1 taught — e.g. why the build succeeded with no
output — then waits for the user to answer in their own words. If the
answer is right, affirm and go to Step 2; if not, re-explain and ask
again.]

[And continues this pattern through all 8 steps]

---

## Tips for Teaching

1. **Celebrate small wins**: "Great!", "Perfect!", "It worked!"
2. **Normalize errors**: "This error is expected!", "Let's see what Buck2 is
   telling us"
3. **Check understanding with Socratic questions**: Gate each step on a
   comprehension check, not a yes/no "ready?" — see "Use the Socratic
   Method Between Major Steps"
4. **Offer choices**: Let users explore or move forward
5. **Show don't tell**: Build and run, then explain
6. **Connect concepts**: "Remember in Step 2 when we learned about artifacts?
   Here's where that matters..."
7. **Show file changes after each step**: After creating or modifying files, briefly list what changed:
   - Example: "I just created `uppercase.bzl` with the rule definition and updated `BUCK` to use it."
   - Example: "I modified `uppercase.bzl` to add the `src` attribute."
8. **Remind users about the editor**: Tell users they can open the files in their editor to see the complete code:
   - Example: "You can open `uppercase.bzl` in your editor to see the full implementation."
   - Example: "Feel free to open these files in your editor to review the changes."

## Next Steps to Suggest After Completion

1. **Read the reference materials**: This skill includes detailed documentation:
   - `references/concepts.md` - Deep dive into Buck2 core concepts (build model, targets, artifacts, actions, providers, configurations, build graph)
   - `references/advanced_patterns.md` - Production-ready patterns (custom providers, transitive sets, toolchains, multiple outputs, testing rules)
2. **Explore Buck2's prelude**: See real production rules in `fbcode/buck2/prelude/`
3. **Try more complex rules**: Multiple outputs, custom providers, transitive dependencies
4. **Learn BXL**: Buck Extension Language for build introspection
5. **Build something real**: Apply what you learned to your project

**Pro tip:** After completing the tutorial, ask questions like "how do providers work in detail?" or "what are transitive sets?" and I'll reference the appropriate documentation to give you deeper explanations.

## Handling Special Cases

**User is stuck:** Offer to review previous concepts or try simpler examples

**User wants to skip ahead:** Allow it, but ensure prerequisites are met

**User found a bug:** Help debug and explain what went wrong

**User wants to experiment:** Encourage it! Help them try variations

**User is confused:** Go back a step, provide more examples, or explain
differently

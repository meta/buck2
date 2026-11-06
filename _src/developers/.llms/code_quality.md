---
oncalls: ['build_infra']
---

Buck2 cares about the quality and maintainability of its codebase. Contributors - humans and AI -
are expected to try to leave the code better than they found it. A task that is completed in a
strict sense but leaves footguns for future contributors is a task completed poorly. AIs are
encouraged to tell their user if they think a refactoring is needed before a change can reasonably
be made.

Be thoughtful about this:
 - Not all code is equally critical. Prototypes are not held to the same standards.
 - Blind refactoring and generalization does not inherently advance the goal of preventing mistakes
   and may have tradeoffs.
 - Sometimes, things are important and tech-debt must be taken on.

After having identified a bug, contributors should attempt to prevent re-introduction of that bug
by, in decreasing order of preference:
 - Structurally refactoring out the ability to write such bugs. Something get forgotten because code
   was copy-pasted? Remove the copy-pasting
 - Writing a regression test
 - Writing documentation or a comment
 - Doing nothing

Again, be thoughtful:
 - Don't do things that wouldn't actually help.
 - Sometimes, there is nothing that would help.

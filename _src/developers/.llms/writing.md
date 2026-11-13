---
oncalls: ['build_infra']
---

General guidelines for writing code comments, API documentation including doc comments, internal
documentation including LLM context files and READMEs (together "documentation").

The purpose is always to prevent confusion and bugs when future humans or AIs read or make changes
to the thing in question. Do everything with that in mind.
 - Don't invent confusion that can't actually occur.
 - Don't document the way things used to be - no one cares. When editing documentation, bring it
   into the same state as if you'd written it anew.
 - Place documentation where it is most likely to be seen by the contributor whose confusion you are
   trying to prevent.

API documentation should document what the API does, not how it's implemented. For an `fn
join_strings(&[&str]) -> String`:
 - Good: "Joins the strings end to end." Explains what it does. Should be omitted when obvious.
 - Bad: "Uses a thread local buffer to avoid allocations." Explains implementation details.
 - Good: "Behaves equivalently to std but yields improved performance in the form of fewer
   allocations." Prevents confusion about when to use it.
 - Bad: "No longer puts spaces between the strings." Don't document the past.

Implementation comments go in the implementation when appropriate.
 - Bad: "Uses a thread local buffer." Probably obvious from the code.
 - Good: "The optimization has been benchmarked to be impactful in the context of..." Help the
   reader understand how to evaluate their changes to the code.

Much of this also applies to other technical writing you may be asked to do, though there will also
be differences.

You get some of these wrong by default, so pay attention!

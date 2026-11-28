---
id: in_memory_cache
title: In Memory Cache
---

Buck2 maintains an in-memory cache of actions it executed. This allows actions
to skip re-running even when they are (transitively) affected by file changes.

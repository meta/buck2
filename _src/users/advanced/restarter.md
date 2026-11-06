---
id: restarter
title: Restarter
---

The Restarter can automatically restart Buck2 when Buck2 detects that it hit a
condition that may be recovered by restarting the Buck2 daemon.

This is particularly useful with
[Deferred Materialization](deferred_materialization.md), which may require a
daemon restart if your daemon holds references to artifacts that have expired in
your Remote Execution backend.

## Enabling the Restarter

To enable, add this to your Buckconfig:

```ini
[buck2]
restarter = true
```

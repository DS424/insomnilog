# ADR 0008: Bounded per-consumer work per pass for fairness

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

The legacy backend drained each consumer queue *fully* before moving to the
next. Whenever one thread produced faster than the backend drained, that
consumer was never emptied and the loop never advanced to later consumers —
starving every other thread's records indefinitely.

## Considered Options

- **Drain-each-fully (legacy).** Rejected: unbounded per-consumer work starves
  later consumers under load.
- **Bounded batch per consumer per pass.** Process at most a fixed number of
  records from each consumer, then move on; revisit on the next pass.
- **Global timestamp-sorted draining (Quill-style).** Drain a batch from every
  consumer into a buffer, sort by timestamp, flush. More work; only needed for
  cross-thread total ordering, which v1 explicitly does not promise.

## Decision Outcome

Chosen: **bounded per-consumer batch per pass.**

- Each worker pass drains up to a fixed cap of records from each consumer before
  moving to the next, so a high-volume thread can no longer block other threads'
  records. The cap is a heuristic constant flagged for later benchmark tuning.
- Cross-thread total ordering is **not** promised: round-robin draining
  processes records in arrival-per-consumer order, so two records produced close
  in time on different threads may be written out of order. This was accepted as
  the simple answer (keep round-robin, accept minor cross-thread reorder); the
  Quill-style sort-by-timestamp approach remains available later if ever needed.

### Consequences

- No single producer can starve the others.
- The cap value is a tuning knob, not a correctness boundary — changing it only
  shifts the fairness/throughput balance.
- Records are not globally timestamp-ordered across threads; per-thread order is
  preserved (a single SPSC queue is FIFO).

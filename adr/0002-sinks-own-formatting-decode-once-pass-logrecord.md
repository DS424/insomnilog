# ADR 0002: Sinks own formatting; decode once and pass a `LogRecord`

- Status: Accepted
- Date: 2026-05-25

## Context and Problem Statement

The legacy backend rendered each record to a `String` and handed sinks
pre-formatted text — modelling Quill's logger-formatter coupling, where the
formatter is fixed by the logger. That prevents a sink from choosing its own
output shape (JSON, SQL parameters, metrics counters) and forces a text
rendering even for sinks that never emit text.

We needed to decide what crosses the sink boundary and who owns formatting.

## Considered Options

- **Backend renders text, sinks receive `&str` (legacy / Quill coupling).**
  Rejected: every sink is forced into a text shape; structured/non-text sinks
  cannot exist; the logger, not the sink, dictates format.
- **Sinks receive the decoded record; each sink owns its own formatting.**
  The backend decodes each record once and fans the decoded value out to every
  sink; each sink decides what to do with it. Text sinks compose a `Formatter`
  internally; non-text sinks ignore formatting entirely.

## Decision Outcome

Chosen: **sinks receive a decoded record and own their formatting.**

- `Sink` is a trait whose core method receives a decoded `LogRecord` by
  reference. Concrete sinks compose whatever they need: a console/text sink
  holds a `Formatter` + an output target; a SQL sink would hold a connection; a
  metrics sink holds counters. The trait only promises `write_record`, `flush`,
  and a `level`.
- `Formatter` is a separate leaf trait, `fn format(&self, record, out: &mut String)`,
  used *only* by text-producing sinks. Not all sinks have one.
- Each record is decoded **once** in the backend, then fanned out to every sink
  on the logger — decoding is not repeated per sink.

### Consequences

- New output shapes are added by writing a `Sink` impl, with no backend change.
- The "one sink, one formatter" relationship lives inside the sink (Python
  `logging` model), not in the logger.
- Decoding cost is paid once per record regardless of sink count.
- The decoded `LogRecord` currently **owns** its data (decoded args, owned
  logger name) rather than borrowing the queue buffer. Owning keeps the dispatch
  loop simple and decouples sink execution from the queue read window; switching
  to a borrowing form to avoid the per-record allocation remains a possible
  future optimisation.

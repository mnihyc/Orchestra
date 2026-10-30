# orch-telemetry

Structured logging and metrics shared by components; what a system records stays inside it.

Logs go to standard error as text or JSON under one filter variable. A mirrored recorder serves
Prometheus and supplies interval aggregates to the services that store minute samples. Counters
store growth, gauges their current value, and distributions an interval mean without retaining raw
observations. Durable absolute totals establish a first baseline; fractional credits retain their
decimal growth. Recorder tests cover repeated sampling and the collector's HTTP listener.

| Module | Planned responsibility |
|--------|------------------------|
| [logging](src/logging.rs) | Structured component and request context with sensitive data excluded, written where the component runs and sent nowhere. |
| [database](src/database.rs) | Counts SQLx's slow-statement and slow-acquisition warnings without retaining SQL or bindings. |
| [metrics](src/metrics.rs) | Metric registration, the exporter for an operator's own collector, and the readings a service samples into the database. |
| [metrics::samples](src/metrics/samples.rs) | Bounded interval aggregates beside the Prometheus recorder, without scraping or parsing the exporter's text. |

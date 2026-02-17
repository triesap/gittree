use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;

#[derive(Debug, Clone)]
pub struct RelayMetrics {
    message_total: Counter<u64>,
    event_total: Counter<u64>,
}

impl RelayMetrics {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("gittree-relay");
        let message_total = meter
            .u64_counter("gittree_relay_message_total")
            .with_description("Total number of relay messages received")
            .init();
        let event_total = meter
            .u64_counter("gittree_relay_event_total")
            .with_description("Total number of events handled by the relay")
            .init();
        Self {
            message_total,
            event_total,
        }
    }

    pub fn record_message(&self, kind: &str) {
        let labels = [KeyValue::new("type", kind.to_string())];
        self.message_total.add(1, &labels);
    }

    pub fn record_event(&self, status: &str) {
        let labels = [KeyValue::new("status", status.to_string())];
        self.event_total.add(1, &labels);
    }
}

#[cfg(test)]
mod tests {
    use super::RelayMetrics;

    #[test]
    fn metrics_record_accepts_calls() {
        let metrics = RelayMetrics::new();
        metrics.record_message("REQ");
        metrics.record_event("accepted");
    }
}

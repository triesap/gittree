use crate::StorageError;
use gittree_core::{RelayCapability, RelayCompatibilityReport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCompatibilityRecord {
    pub relay_url: String,
    pub compatible: bool,
    pub supported_capabilities: Vec<String>,
    pub missing_required: Vec<String>,
    pub missing_optional: Vec<String>,
    pub report_json: String,
    pub checked_at: i64,
}

impl RelayCompatibilityRecord {
    pub fn new(
        report: &RelayCompatibilityReport,
        checked_at: i64,
    ) -> Result<Self, StorageError> {
        if report.relay_url.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "relay_url",
                value: "empty".to_string(),
            });
        }

        let report_json = serde_json::to_string(report).map_err(|source| {
            StorageError::Serialization {
                field: "report",
                source,
            }
        })?;

        Ok(Self {
            relay_url: report.relay_url.clone(),
            compatible: report.is_compatible(),
            supported_capabilities: Self::capabilities_as_strings(&report.supported),
            missing_required: Self::capabilities_as_strings(&report.missing_required),
            missing_optional: Self::capabilities_as_strings(&report.missing_optional),
            report_json,
            checked_at,
        })
    }

    pub fn report(&self) -> Result<RelayCompatibilityReport, StorageError> {
        serde_json::from_str(&self.report_json).map_err(|source| StorageError::Serialization {
            field: "report",
            source,
        })
    }

    fn capabilities_as_strings(caps: &[RelayCapability]) -> Vec<String> {
        caps.iter().map(|cap| cap.as_str().to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::RelayCompatibilityRecord;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};

    #[test]
    fn record_maps_report_fields() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: vec![RelayCapability::Nip11],
        };

        let record = RelayCompatibilityRecord::new(&report, 123).expect("record");
        assert_eq!(record.relay_url, report.relay_url);
        assert!(record.compatible);
        assert_eq!(record.supported_capabilities, vec!["nip-01", "nip-34"]);
        assert_eq!(record.missing_optional, vec!["nip-11"]);
        assert_eq!(record.checked_at, 123);
    }

    #[test]
    fn record_round_trips_report_json() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01],
            missing_required: vec![RelayCapability::Nip34],
            missing_optional: Vec::new(),
        };

        let record = RelayCompatibilityRecord::new(&report, 0).expect("record");
        let parsed = record.report().expect("report");
        assert_eq!(parsed, report);
    }

    #[test]
    fn record_rejects_empty_url() {
        let report = RelayCompatibilityReport {
            relay_url: " ".to_string(),
            supported: Vec::new(),
            missing_required: vec![RelayCapability::Nip34],
            missing_optional: Vec::new(),
        };

        assert!(RelayCompatibilityRecord::new(&report, 0).is_err());
    }
}

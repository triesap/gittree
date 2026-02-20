use crate::StorageError;
use gittree_core::{RelayCapability, RelayCompatibilityReport};

fn map_serde_error(field: &'static str, source: serde_json::Error) -> StorageError {
    StorageError::Serialization { field, source }
}

fn serialize_report(report: &RelayCompatibilityReport) -> String {
    // RelayCompatibilityReport is fully JSON-serializable by construction.
    serde_json::to_string(report).expect("serialize relay compatibility report")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayProbeMetadata {
    pub nip11_url: Option<String>,
    pub nip11_available: bool,
    pub active_probe_ok: Option<bool>,
    pub active_probe_error: Option<String>,
}

impl Default for RelayProbeMetadata {
    fn default() -> Self {
        Self {
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: None,
            active_probe_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCompatibilityRecord {
    pub relay_url: String,
    pub compatible: bool,
    pub supported_capabilities: Vec<String>,
    pub missing_required: Vec<String>,
    pub missing_optional: Vec<String>,
    pub report_json: String,
    pub nip11_url: Option<String>,
    pub nip11_available: bool,
    pub active_probe_ok: Option<bool>,
    pub active_probe_error: Option<String>,
    pub checked_at: i64,
}

impl RelayCompatibilityRecord {
    pub fn new(
        report: &RelayCompatibilityReport,
        checked_at: i64,
        metadata: &RelayProbeMetadata,
    ) -> Result<Self, StorageError> {
        if report.relay_url.trim().is_empty() {
            return Err(StorageError::InvalidField {
                field: "relay_url",
                value: "empty".to_string(),
            });
        }

        let report_json = serialize_report(report);

        Ok(Self {
            relay_url: report.relay_url.clone(),
            compatible: report.is_compatible(),
            supported_capabilities: Self::capabilities_as_strings(&report.supported),
            missing_required: Self::capabilities_as_strings(&report.missing_required),
            missing_optional: Self::capabilities_as_strings(&report.missing_optional),
            report_json,
            nip11_url: metadata.nip11_url.clone(),
            nip11_available: metadata.nip11_available,
            active_probe_ok: metadata.active_probe_ok,
            active_probe_error: metadata.active_probe_error.clone(),
            checked_at,
        })
    }

    pub fn report(&self) -> Result<RelayCompatibilityReport, StorageError> {
        serde_json::from_str(&self.report_json).map_err(|source| map_serde_error("report", source))
    }

    fn capabilities_as_strings(caps: &[RelayCapability]) -> Vec<String> {
        caps.iter().map(|cap| cap.as_str().to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::RelayCompatibilityRecord;
    use super::RelayProbeMetadata;
    use crate::StorageError;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};

    #[test]
    fn record_maps_report_fields() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: vec![RelayCapability::Nip11],
        };

        let record = RelayCompatibilityRecord::new(&report, 123, &RelayProbeMetadata::default())
            .expect("record");
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

        let record = RelayCompatibilityRecord::new(&report, 0, &RelayProbeMetadata::default())
            .expect("record");
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

        assert!(RelayCompatibilityRecord::new(&report, 0, &RelayProbeMetadata::default()).is_err());
    }

    #[test]
    fn record_maps_probe_metadata() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        let metadata = RelayProbeMetadata {
            nip11_url: Some("https://relay.example/".to_string()),
            nip11_available: true,
            active_probe_ok: Some(true),
            active_probe_error: None,
        };
        let record = RelayCompatibilityRecord::new(&report, 1, &metadata).expect("record");
        assert_eq!(record.nip11_url, metadata.nip11_url);
        assert!(record.nip11_available);
        assert_eq!(record.active_probe_ok, Some(true));
    }

    #[test]
    fn record_report_rejects_invalid_json() {
        let record = RelayCompatibilityRecord {
            relay_url: "wss://relay.example".to_string(),
            compatible: true,
            supported_capabilities: vec!["nip-01".to_string()],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
            report_json: "{".to_string(),
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: None,
            active_probe_error: None,
            checked_at: 0,
        };
        let err = record.report().unwrap_err();
        assert!(matches!(err, StorageError::Serialization { field, .. } if field == "report"));
    }

    #[test]
    fn record_maps_active_probe_error_message() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01],
            missing_required: vec![RelayCapability::Nip34],
            missing_optional: Vec::new(),
        };
        let metadata = RelayProbeMetadata {
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: Some(false),
            active_probe_error: Some("timeout".to_string()),
        };
        let record = RelayCompatibilityRecord::new(&report, 5, &metadata).expect("record");
        assert_eq!(record.active_probe_ok, Some(false));
        assert_eq!(record.active_probe_error.as_deref(), Some("timeout"));
    }

    #[test]
    fn probe_metadata_derives_clone_debug_and_eq() {
        let metadata = RelayProbeMetadata {
            nip11_url: Some("https://relay.example/".to_string()),
            nip11_available: true,
            active_probe_ok: Some(true),
            active_probe_error: Some("timeout".to_string()),
        };
        let cloned = metadata.clone();
        assert_eq!(metadata, cloned);
        let rendered = format!("{metadata:?}");
        assert!(rendered.contains("RelayProbeMetadata"));
    }

    #[test]
    fn record_derives_clone_debug_and_eq() {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        let metadata = RelayProbeMetadata {
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: Some(true),
            active_probe_error: Some("timeout".to_string()),
        };
        let record = RelayCompatibilityRecord::new(&report, 7, &metadata).expect("record");
        let cloned = record.clone();
        assert_eq!(record, cloned);
        let rendered = format!("{record:?}");
        assert!(rendered.contains("RelayCompatibilityRecord"));
    }

}

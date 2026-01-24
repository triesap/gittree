use serde::{Deserialize, Serialize};

use crate::nip11::RelayInfoDocument;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelayCapability {
    Nip01,
    Nip11,
    Nip34,
    Nip65,
    Grasp,
}

impl RelayCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayCapability::Nip01 => "nip-01",
            RelayCapability::Nip11 => "nip-11",
            RelayCapability::Nip34 => "nip-34",
            RelayCapability::Nip65 => "nip-65",
            RelayCapability::Grasp => "grasp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayCapabilitySet {
    pub required: Vec<RelayCapability>,
    pub optional: Vec<RelayCapability>,
}

pub fn capabilities_from_nip11(doc: &RelayInfoDocument) -> Vec<RelayCapability> {
    let mut supported = Vec::new();
    if doc.supports_nip(1) {
        supported.push(RelayCapability::Nip01);
    }
    if doc.supports_nip(11) {
        supported.push(RelayCapability::Nip11);
    }
    if doc.supports_nip(34) {
        supported.push(RelayCapability::Nip34);
    }
    if doc.supports_nip(65) {
        supported.push(RelayCapability::Nip65);
    }
    if doc
        .supported_grasps
        .as_ref()
        .is_some_and(|grasps| grasps.iter().any(|entry| !entry.trim().is_empty()))
    {
        supported.push(RelayCapability::Grasp);
    }
    supported
}

impl RelayCapabilitySet {
    pub fn evaluate(
        &self,
        relay_url: impl Into<String>,
        supported: &[RelayCapability],
    ) -> RelayCompatibilityReport {
        let mut supported_set: std::collections::HashSet<RelayCapability> =
            supported.iter().copied().collect();
        let missing_required = self
            .required
            .iter()
            .copied()
            .filter(|cap| !supported_set.contains(cap))
            .collect();
        let missing_optional = self
            .optional
            .iter()
            .copied()
            .filter(|cap| !supported_set.contains(cap))
            .collect();
        supported_set.extend(self.required.iter().copied());
        supported_set.extend(self.optional.iter().copied());
        let mut supported: Vec<RelayCapability> = supported_set.into_iter().collect();
        supported.sort_by_key(|cap| cap.as_str());

        RelayCompatibilityReport {
            relay_url: relay_url.into(),
            supported,
            missing_required,
            missing_optional,
        }
    }
}

impl Default for RelayCapabilitySet {
    fn default() -> Self {
        Self {
            required: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            optional: vec![
                RelayCapability::Nip11,
                RelayCapability::Nip65,
                RelayCapability::Grasp,
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayCompatibilityReport {
    pub relay_url: String,
    pub supported: Vec<RelayCapability>,
    pub missing_required: Vec<RelayCapability>,
    pub missing_optional: Vec<RelayCapability>,
}

impl RelayCompatibilityReport {
    pub fn is_compatible(&self) -> bool {
        self.missing_required.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{RelayCapability, RelayCapabilitySet, capabilities_from_nip11};
    use crate::nip11::RelayInfoDocument;

    #[test]
    fn compatibility_report_marks_missing_required() {
        let requirements = RelayCapabilitySet::default();
        let report = requirements.evaluate(
            "wss://relay.example",
            &[RelayCapability::Nip01, RelayCapability::Nip11],
        );
        assert!(!report.is_compatible());
        assert!(report.missing_required.contains(&RelayCapability::Nip34));
    }

    #[test]
    fn compatibility_report_accepts_required_capabilities() {
        let requirements = RelayCapabilitySet::default();
        let report = requirements.evaluate(
            "wss://relay.example",
            &[RelayCapability::Nip01, RelayCapability::Nip34],
        );
        assert!(report.is_compatible());
        assert!(report.missing_optional.contains(&RelayCapability::Nip11));
    }

    #[test]
    fn capabilities_from_nip11_detects_supported_nips() {
        let doc = RelayInfoDocument {
            name: None,
            description: None,
            banner: None,
            icon: None,
            pubkey: None,
            self_pubkey: None,
            contact: None,
            supported_nips: Some(vec![1, 11, 34, 65]),
            software: None,
            version: None,
            privacy_policy: None,
            terms_of_service: None,
            limitation: None,
            retention: None,
            relay_countries: None,
            language_tags: None,
            tags: None,
            posting_policy: None,
            payments_url: None,
            fees: None,
            supported_grasps: Some(vec!["GRASP-01".to_string()]),
            repo_acceptance_criteria: None,
            curation: None,
        };
        let supported = capabilities_from_nip11(&doc);
        assert!(supported.contains(&RelayCapability::Nip01));
        assert!(supported.contains(&RelayCapability::Nip11));
        assert!(supported.contains(&RelayCapability::Nip34));
        assert!(supported.contains(&RelayCapability::Nip65));
        assert!(supported.contains(&RelayCapability::Grasp));
    }
}

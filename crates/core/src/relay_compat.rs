use serde::{Deserialize, Serialize};

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
    use super::{RelayCapability, RelayCapabilitySet};

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
}

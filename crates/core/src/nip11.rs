use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KindSelector {
    Single(u32),
    Range([u32; 2]),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimitation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_message_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_subscriptions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_subid_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_event_tags: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_content_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_pow_difficulty: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restricted_writes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_lower_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_upper_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<KindSelector>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fee {
    pub amount: u64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayInfoDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,
    #[serde(rename = "self", skip_serializing_if = "Option::is_none")]
    pub self_pubkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_nips: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<RelayLimitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention: Option<Vec<RetentionRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_countries: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posting_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payments_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fees: Option<BTreeMap<String, Vec<Fee>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_grasps: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_acceptance_criteria: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::RelayInfoDocument;
    use serde_json::Value;

    #[test]
    fn nip11_serializes_grasp_fields() {
        let doc = RelayInfoDocument {
            name: Some("gittree".to_string()),
            description: None,
            banner: None,
            icon: None,
            pubkey: None,
            self_pubkey: None,
            contact: None,
            supported_nips: Some(vec![1, 11, 34]),
            software: Some("https://github.com/triesap/gittree".to_string()),
            version: Some("0.1.0".to_string()),
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
            repo_acceptance_criteria: Some("requires clone and relays tags".to_string()),
            curation: Some("no additional curation".to_string()),
        };

        let value = serde_json::to_value(&doc).expect("serialize nip-11");
        assert!(value.get("supported_grasps").is_some());
        assert!(value.get("repo_acceptance_criteria").is_some());
        assert!(value.get("curation").is_some());
        assert!(value.get("supported_nips").is_some());
    }

    #[test]
    fn nip11_uses_self_field_name() {
        let doc = RelayInfoDocument {
            name: None,
            description: None,
            banner: None,
            icon: None,
            pubkey: None,
            self_pubkey: Some("abc".to_string()),
            contact: None,
            supported_nips: None,
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
            supported_grasps: None,
            repo_acceptance_criteria: None,
            curation: None,
        };

        let value = serde_json::to_value(&doc).expect("serialize nip-11");
        assert_eq!(value.get("self"), Some(&Value::String("abc".to_string())));
    }
}

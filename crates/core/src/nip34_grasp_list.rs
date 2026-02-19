use crate::tags::push_unique;
use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserGraspList {
    pub urls: Vec<String>,
}

impl UserGraspList {
    pub fn from_tags_lossy(tags: &[Vec<String>]) -> Self {
        let mut urls = Vec::new();

        for tag in tags {
            if let [t, value, ..] = tag.as_slice() {
                if t == "g" {
                    push_unique(&mut urls, value);
                }
            }
        }

        Self { urls }
    }

    pub fn from_tags(tags: &[Vec<String>]) -> Result<Self> {
        Ok(Self::from_tags_lossy(tags))
    }

    pub fn to_tags(&self) -> Vec<Vec<String>> {
        let mut tags = Vec::new();

        for url in &self.urls {
            tags.push(vec!["g".to_string(), url.clone()]);
        }

        tags
    }

    pub fn validate(&self) -> Result<()> {
        for url in &self.urls {
            let parsed = url::Url::parse(url).map_err(|_| CoreError::InvalidField {
                field: "g",
                value: url.clone(),
            })?;

            let scheme = parsed.scheme();
            if scheme != "ws" && scheme != "wss" {
                return Err(CoreError::InvalidField {
                    field: "g",
                    value: url.clone(),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UserGraspList;
    use crate::CoreError;

    #[test]
    fn grasp_list_round_trips_tags() {
        let list = UserGraspList {
            urls: vec![
                "wss://relay.example".to_string(),
                "ws://localhost:8080".to_string(),
            ],
        };

        let tags = list.to_tags();
        let parsed = UserGraspList::from_tags(&tags).expect("parse");
        assert_eq!(parsed, list);
        parsed.validate().expect("valid");
    }

    #[test]
    fn grasp_list_rejects_http_urls() {
        let list = UserGraspList {
            urls: vec!["https://relay.example".to_string()],
        };

        assert!(list.validate().is_err());
    }

    #[test]
    fn grasp_list_from_tags_ignores_non_g_and_deduplicates() {
        let tags = vec![
            vec!["p".to_string(), "wss://ignored.example".to_string()],
            vec!["g".to_string()],
            vec!["g".to_string(), "wss://relay.example".to_string()],
            vec![
                "g".to_string(),
                "wss://relay.example".to_string(),
                "extra".to_string(),
            ],
            vec!["g".to_string(), "ws://localhost:8080".to_string()],
        ];

        let parsed = UserGraspList::from_tags(&tags).expect("parse tags");
        assert_eq!(
            parsed.urls,
            vec![
                "wss://relay.example".to_string(),
                "ws://localhost:8080".to_string()
            ]
        );
    }

    #[test]
    fn grasp_list_rejects_malformed_url() {
        let list = UserGraspList {
            urls: vec!["not a url".to_string()],
        };

        let err = list.validate().expect_err("invalid URL must fail");
        assert!(matches!(
            err,
            CoreError::InvalidField { field: "g", value } if value == "not a url"
        ));
    }
}

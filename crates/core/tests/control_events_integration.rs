use gittree_core::{
    ActiveProbeEvidence, ControlAction, CoreError, KIND_GITTREE_CONTROL, RelayCapability,
    RelayCapabilitySet, RelayInfoDocument, capabilities_from_nip11, merge_active_probe_evidence,
};

fn empty_relay_doc() -> RelayInfoDocument {
    RelayInfoDocument {
        name: None,
        description: None,
        banner: None,
        icon: None,
        pubkey: None,
        self_pubkey: None,
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
    }
}

#[test]
fn parse_control_action_variants_integration() {
    let user = ControlAction::parse(
        KIND_GITTREE_CONTROL.0,
        r#"{"action":"create_user","username":"alice","email":"alice@example.com","password":"secret"}"#,
        KIND_GITTREE_CONTROL.0,
    )
    .expect("parse user");
    assert!(matches!(user, ControlAction::CreateUser { .. }));

    let repo = ControlAction::parse(
        KIND_GITTREE_CONTROL.0,
        r#"{"action":"create_repo","name":"repo","owner":"alice","identifier":"repo","pubkey":"11e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#,
        KIND_GITTREE_CONTROL.0,
    )
    .expect("parse repo");
    assert!(matches!(repo, ControlAction::CreateRepo { .. }));
}

#[test]
fn parse_control_action_rejects_invalid_cases_integration() {
    let wrong_kind = ControlAction::parse(
        KIND_GITTREE_CONTROL.0 + 1,
        r#"{"action":"create_org","name":"acme"}"#,
        KIND_GITTREE_CONTROL.0,
    )
    .expect_err("wrong kind");
    assert!(matches!(
        wrong_kind,
        CoreError::InvalidField { field: "kind", .. }
    ));

    let invalid_json =
        ControlAction::parse(KIND_GITTREE_CONTROL.0, "not-json", KIND_GITTREE_CONTROL.0)
            .expect_err("invalid json");
    assert!(matches!(
        invalid_json,
        CoreError::InvalidField {
            field: "content",
            ..
        }
    ));

    let invalid_title = ControlAction::parse(
        KIND_GITTREE_CONTROL.0,
        r#"{"action":"create_pull_request","owner":"alice","repo":"repo","head":"feature","base":"main","title":"  "}"#,
        KIND_GITTREE_CONTROL.0,
    )
    .expect_err("invalid title");
    assert!(matches!(
        invalid_title,
        CoreError::InvalidField { field: "title", .. }
    ));
}

#[test]
fn relay_compatibility_default_and_probe_paths_integration() {
    let requirements = RelayCapabilitySet::default();
    let report = requirements.evaluate("wss://relay.example", &[RelayCapability::Nip01]);
    assert!(!report.is_compatible());
    assert!(report.missing_required.contains(&RelayCapability::Nip34));
    assert!(report.missing_optional.contains(&RelayCapability::Nip11));

    let mut supported = vec![RelayCapability::Nip11];
    merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::success());
    assert!(supported.contains(&RelayCapability::Nip01));
    assert!(supported.contains(&RelayCapability::Nip34));

    let before_failure = supported.clone();
    merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::failure());
    assert_eq!(supported, before_failure);
}

#[test]
fn relay_compatibility_grasp_detection_integration() {
    let mut doc = empty_relay_doc();
    doc.supported_nips = Some(vec![1, 34]);
    doc.supported_grasps = Some(vec![" ".to_string(), "GRASP-01".to_string()]);
    let supported = capabilities_from_nip11(&doc);
    assert!(supported.contains(&RelayCapability::Nip01));
    assert!(supported.contains(&RelayCapability::Nip34));
    assert!(supported.contains(&RelayCapability::Grasp));
}

use gittree_core::{
    ActiveProbeEvidence, ControlAction, CoreError, KIND_GITTREE_CONTROL, RelayCapability,
    RelayCapabilitySet, RelayCompatibilityReport, RelayInfoDocument, capabilities_from_nip11,
    merge_active_probe_evidence,
};

fn relay_doc_with_nips_and_grasps(
    nips: Option<Vec<u16>>,
    grasps: Option<Vec<&str>>,
) -> RelayInfoDocument {
    RelayInfoDocument {
        name: None,
        description: None,
        banner: None,
        icon: None,
        pubkey: None,
        self_pubkey: None,
        contact: None,
        supported_nips: nips,
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
        supported_grasps: grasps.map(|values| {
            values
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect()
        }),
        repo_acceptance_criteria: None,
        curation: None,
    }
}

#[test]
fn relay_capability_strings_cover_all_variants() {
    assert_eq!(RelayCapability::Nip01.as_str(), "nip-01");
    assert_eq!(RelayCapability::Nip11.as_str(), "nip-11");
    assert_eq!(RelayCapability::Nip34.as_str(), "nip-34");
    assert_eq!(RelayCapability::Nip65.as_str(), "nip-65");
    assert_eq!(RelayCapability::Grasp.as_str(), "grasp");
}

#[test]
fn capability_set_default_and_evaluate_paths_are_stable() {
    let defaults = RelayCapabilitySet::default();
    assert_eq!(
        defaults.required,
        vec![RelayCapability::Nip01, RelayCapability::Nip34]
    );
    assert_eq!(
        defaults.optional,
        vec![
            RelayCapability::Nip11,
            RelayCapability::Nip65,
            RelayCapability::Grasp,
        ]
    );

    let report = defaults.evaluate("wss://relay.example", &[RelayCapability::Nip01]);
    assert!(!report.is_compatible());
    assert_eq!(report.relay_url, "wss://relay.example");
    assert!(report.missing_required.contains(&RelayCapability::Nip34));
    assert!(report.missing_optional.contains(&RelayCapability::Nip11));
    assert!(report.supported.contains(&RelayCapability::Nip01));
    assert!(report.supported.contains(&RelayCapability::Nip34));
}

#[test]
fn active_probe_helpers_cover_success_failure_and_no_duplicates() {
    assert!(ActiveProbeEvidence::success().write_read_ok);
    assert!(!ActiveProbeEvidence::failure().write_read_ok);

    let mut supported = vec![RelayCapability::Nip11];
    merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::success());
    assert!(supported.contains(&RelayCapability::Nip01));
    assert!(supported.contains(&RelayCapability::Nip34));

    let before = supported.clone();
    merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::failure());
    assert_eq!(supported, before);

    merge_active_probe_evidence(&mut supported, ActiveProbeEvidence::success());
    assert_eq!(
        supported
            .iter()
            .filter(|cap| **cap == RelayCapability::Nip01)
            .count(),
        1
    );
    assert_eq!(
        supported
            .iter()
            .filter(|cap| **cap == RelayCapability::Nip34)
            .count(),
        1
    );
}

#[test]
fn capabilities_from_nip11_handles_grasp_variants() {
    let with_grasp = relay_doc_with_nips_and_grasps(
        Some(vec![1, 11, 34, 65]),
        Some(vec!["   ", "", "GRASP-01"]),
    );
    let supported = capabilities_from_nip11(&with_grasp);
    assert!(supported.contains(&RelayCapability::Nip01));
    assert!(supported.contains(&RelayCapability::Nip11));
    assert!(supported.contains(&RelayCapability::Nip34));
    assert!(supported.contains(&RelayCapability::Nip65));
    assert!(supported.contains(&RelayCapability::Grasp));

    let blank_only = relay_doc_with_nips_and_grasps(Some(vec![1]), Some(vec![" ", ""]));
    let supported_blank = capabilities_from_nip11(&blank_only);
    assert!(supported_blank.contains(&RelayCapability::Nip01));
    assert!(!supported_blank.contains(&RelayCapability::Grasp));

    let missing_grasp = relay_doc_with_nips_and_grasps(Some(vec![34]), None);
    let supported_missing = capabilities_from_nip11(&missing_grasp);
    assert_eq!(supported_missing, vec![RelayCapability::Nip34]);
}

#[test]
fn compatibility_report_round_trip_preserves_optional_fields() {
    let report = RelayCapabilitySet::default().evaluate(
        "wss://relay.example",
        &[RelayCapability::Nip01, RelayCapability::Nip34],
    );
    assert!(report.is_compatible());
    let json = serde_json::to_string(&report).expect("serialize report");
    let decoded: RelayCompatibilityReport = serde_json::from_str(&json).expect("decode report");
    assert_eq!(decoded.missing_optional, report.missing_optional);
}

#[test]
fn control_action_paths_are_exercised_in_relay_compat_binary() {
    let valid_variants = [
        r#"{"action":"create_user","username":"alice","email":"alice@example.com","password":"secret"}"#,
        r#"{"action":"create_org","name":"acme","full_name":"acme corp"}"#,
        r#"{"action":"create_repo","name":"repo","owner":"alice","identifier":"repo","pubkey":"11e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#,
        r#"{"action":"create_pull_request","owner":"alice","repo":"repo","head":"feature","base":"main","title":"hello"}"#,
    ];

    for payload in valid_variants {
        let action = ControlAction::parse(KIND_GITTREE_CONTROL.0, payload, KIND_GITTREE_CONTROL.0)
            .expect("parse valid action");
        action.validate().expect("validate parsed action");
        let encoded = serde_json::to_string(&action).expect("serialize action");
        let decoded: ControlAction = serde_json::from_str(&encoded).expect("decode action");
        decoded.validate().expect("validate decoded action");
    }

    let wrong_kind = ControlAction::parse(
        KIND_GITTREE_CONTROL.0 + 1,
        valid_variants[0],
        KIND_GITTREE_CONTROL.0,
    )
    .expect_err("wrong kind");
    assert!(matches!(
        wrong_kind,
        CoreError::InvalidField { field: "kind", .. }
    ));

    let invalid_json = ControlAction::parse(KIND_GITTREE_CONTROL.0, "not-json", KIND_GITTREE_CONTROL.0)
        .expect_err("invalid json");
    assert!(matches!(
        invalid_json,
        CoreError::InvalidField {
            field: "content",
            ..
        }
    ));
}

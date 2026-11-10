use core_contracts::{DeviceMode, EngineFamily, ErrorCode, LaunchSnapshot, ProxyPolicy};
use serde_json::json;

fn snapshot() -> LaunchSnapshot {
    let mut s: LaunchSnapshot = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/launch-desktop.json"
    ))
    .unwrap();
    let root = std::env::temp_dir().join(format!("Numera فارسی path {}", std::process::id()));
    s.resources.native_profile_dir = root.join("native").to_string_lossy().into();
    s.resources.data_root = root.join("data").to_string_lossy().into();
    s.resources.cache_root = root.join("cache").to_string_lossy().into();
    s
}

#[test]
fn planning_preserves_family_and_does_not_create_directories() {
    let mut s = snapshot();
    for engine in [
        EngineFamily::Chromium,
        EngineFamily::Gecko,
        EngineFamily::Webkit,
    ] {
        s.engine = engine;
        let p = core_runtime::plan(&s).unwrap();
        assert_eq!(
            p["requested_engine"],
            serde_json::to_value(&s.engine).unwrap()
        );
        assert_eq!(p["status"], "blocked");
        assert!(p["selected_build"].is_null());
        assert_eq!(p["side_effects"], false);
        assert!(!std::path::Path::new(&s.resources.data_root).exists());
    }
}
#[test]
fn identity_is_stable_and_changes_only_with_identity_inputs() {
    let mut s = snapshot();
    let first = core_identity::identity_key(&s);
    s.launch_id = "another-launch".into();
    assert_eq!(first, core_identity::identity_key(&s));
    s.fingerprint_seed = s.fingerprint_seed.to_uppercase();
    assert_eq!(first, core_identity::identity_key(&s));
    s.profile_id = "another-profile".into();
    assert_ne!(first, core_identity::identity_key(&s));
}
#[test]
fn impossible_mobile_combination_requires_explicit_acceptance() {
    let mut s = snapshot();
    s.device_mode = DeviceMode::IosEmulated;
    s.identity.touch = true;
    assert!(core_runtime::plan(&s).is_err());
    s.accept_emulation_limitations = true;
    let p = core_runtime::plan(&s).unwrap();
    assert_eq!(p["requested_engine"], "chromium");
    assert_eq!(p["settings_applied"], false);
    assert!(p["warnings"].as_array().unwrap().len() >= 2);
}
#[test]
fn required_proxy_is_blocked_and_never_silently_direct() {
    let mut s = snapshot();
    s.proxy = ProxyPolicy::Required {
        endpoint: "socks5h://127.0.0.1:1080".into(),
        secret_ref: Some("vault-ref-1".into()),
    };
    let p = core_runtime::plan(&s).unwrap();
    assert!(p["blockers"].as_array().unwrap().iter().any(|v| {
        v["message"]
            .as_str()
            .unwrap()
            .contains("direct fallback is forbidden")
    }));
    assert!(!p.to_string().contains("vault-ref-1"));
}
#[test]
fn embedded_credentials_rejected_without_echo() {
    let mut s = snapshot();
    s.startup_urls = vec!["https://user:synthetic-password@example.test/".into()];
    let e = core_runtime::plan(&s).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidConfiguration);
    assert!(!e.to_string().contains("synthetic-password"));
    s.startup_urls = vec!["about:blank".into()];
    s.proxy = ProxyPolicy::Required {
        endpoint: "http://user:synthetic-password@localhost:8000".into(),
        secret_ref: None,
    };
    assert!(
        !core_runtime::plan(&s)
            .unwrap_err()
            .to_string()
            .contains("synthetic-password")
    );
}
#[test]
fn unknown_required_capability_is_unavailable() {
    let mut s = snapshot();
    s.required_capabilities = vec!["unrecognized.required.feature".into()];
    let p = core_runtime::plan(&s).unwrap();
    assert!(
        p["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["code"] == "unsupported_capability")
    );
}
#[test]
fn identity_consistency_rejects_bad_screen_scale_and_touch() {
    let original = snapshot();
    for index in 0..4 {
        let mut s = original.clone();
        match index {
            0 => s.identity.screen_width = 100,
            1 => s.identity.device_scale_factor = f64::NAN,
            2 => s.identity.device_scale_factor = 0.01,
            _ => s.device_mode = DeviceMode::AndroidEmulated,
        }
        assert!(core_runtime::plan(&s).is_err());
    }
}
#[test]
fn snapshot_rejects_unknown_fields_and_engine_enums() {
    let s = snapshot();
    let mut v = serde_json::to_value(s).unwrap();
    v["engine"] = json!("safari");
    assert!(serde_json::from_value::<LaunchSnapshot>(v.clone()).is_err());
    v["engine"] = json!("chromium");
    v["hidden_python_runtime"] = json!(true);
    assert!(serde_json::from_value::<LaunchSnapshot>(v).is_err());
}
#[test]
fn lexical_path_traversal_rejected_before_any_storage_action() {
    let mut s = snapshot();
    s.resources.data_root = std::env::temp_dir().join("a/../b").to_string_lossy().into();
    assert!(core_runtime::plan(&s).is_err());
}

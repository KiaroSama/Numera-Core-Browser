//! Configuration validation and deterministic identity keys. No JS overrides.
use core_contracts::{CoreError, DeviceMode, EngineFamily, LaunchSnapshot};
use sha2::{Digest, Sha256};

pub fn validate(snapshot: &LaunchSnapshot) -> Result<Vec<&'static str>, CoreError> {
    let i = &snapshot.identity;
    if !(100..=16384).contains(&i.viewport_width)
        || !(100..=16384).contains(&i.viewport_height)
        || i.screen_width < i.viewport_width
        || i.screen_height < i.viewport_height
        || i.screen_width > 16384
        || i.screen_height > 16384
        || !i.device_scale_factor.is_finite()
        || !(0.5..=8.0).contains(&i.device_scale_factor)
    {
        return Err(CoreError::invalid(
            "Incoherent viewport, screen or scale factor",
        ));
    }
    if i.language.is_empty()
        || i.language.len() > 64
        || !i
            .language
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || i.timezone.is_empty()
        || i.timezone.len() > 128
        || !i
            .timezone
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
    {
        return Err(CoreError::invalid("Invalid language or timezone syntax"));
    }
    if snapshot.fingerprint_seed.len() != 64
        || !snapshot
            .fingerprint_seed
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(CoreError::invalid(
            "Fingerprint seed must be 32 bytes encoded as hexadecimal",
        ));
    }
    let mut warnings = Vec::new();
    if snapshot.device_mode != DeviceMode::Desktop {
        if !i.touch {
            return Err(CoreError::invalid("Mobile emulation requires touch input"));
        }
        warnings.push("Desktop emulation does not provide a native mobile OS, attestation, TLS stack or physical GPU");
    }
    if snapshot.device_mode == DeviceMode::IosEmulated && snapshot.engine != EngineFamily::Webkit {
        if !snapshot.accept_emulation_limitations {
            return Err(CoreError::invalid(
                "iOS-like mode on a non-WebKit engine requires explicit testing-only acceptance",
            ));
        }
        warnings.push("Selected engine is not WebKit or Safari; engine family is preserved");
    }
    warnings.push("Identity settings are validated only; no engine has applied these settings");
    Ok(warnings)
}

/// Stable across tabs/restarts, scoped to versioned identity inputs. Not a
/// fingerprint-obfuscation claim and not an authentication credential.
pub fn identity_key(snapshot: &LaunchSnapshot) -> String {
    let mut hash = Sha256::new();
    hash.update(b"numera.identity.v1\0");
    for field in [
        snapshot.profile_id.as_bytes(),
        snapshot.fingerprint_seed.to_ascii_lowercase().as_bytes(),
        serde_json::to_string(&snapshot.engine)
            .expect("enum serializes")
            .as_bytes(),
        serde_json::to_string(&snapshot.device_mode)
            .expect("enum serializes")
            .as_bytes(),
        serde_json::to_vec(&snapshot.identity)
            .expect("identity serializes")
            .as_slice(),
    ] {
        hash.update((field.len() as u64).to_le_bytes());
        hash.update(field);
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Production keychain namespace. Keep this equal to the Tauri bundle
/// identifier so the native x0x app never probes the inherited
/// `buzz-desktop` entry. That legacy entry may have an ACL bound to an older
/// signing identity, which makes Finder launches prompt on every read.
///
/// Decision (2026-08-12, v0.5.1): do **not** migrate secrets from
/// `buzz-desktop`. v0.5.0 installs were team-only; copying those items into
/// the new service would re-introduce the Buzz ACL prompt this split exists
/// to stop. Users on a v0.5.0 identity re-create agent secrets on first
/// 0.5.1 launch. See `docs/releasing-macos.md`.
const PRODUCTION_KEYRING_SERVICE: &str = "com.saorsalabs.tictactoe";

/// Service name for the desktop OS keyring. Debug builds default to a distinct
/// service, while standalone worktree launches may request a scoped dev service.
fn dev_keyring_service(configured: Option<String>) -> String {
    configured
        .filter(|service| service.starts_with("buzz-desktop-dev."))
        .unwrap_or_else(|| "buzz-desktop-dev".to_string())
}

fn keyring_service_for_build(debug_assertions: bool, configured: Option<String>) -> String {
    if debug_assertions {
        dev_keyring_service(configured)
    } else {
        PRODUCTION_KEYRING_SERVICE.to_string()
    }
}

pub(crate) fn keyring_service() -> &'static str {
    static SERVICE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SERVICE
        .get_or_init(|| {
            keyring_service_for_build(
                cfg!(debug_assertions),
                std::env::var("BUZZ_DEV_KEYRING_SERVICE").ok(),
            )
        })
        .as_str()
}

pub(super) fn migration_marker_name(service: &str, default_name: &str) -> String {
    if service == "buzz-desktop" || service == "buzz-desktop-dev" {
        default_name.to_string()
    } else {
        format!("identity.{service}.migrated")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dev_keyring_service, keyring_service_for_build, migration_marker_name,
        PRODUCTION_KEYRING_SERVICE,
    };

    #[test]
    fn production_keyring_namespace_matches_native_bundle_not_legacy_buzz() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            config.get("identifier").and_then(serde_json::Value::as_str),
            Some(PRODUCTION_KEYRING_SERVICE)
        );
        assert_ne!(PRODUCTION_KEYRING_SERVICE, "buzz-desktop");
        assert_eq!(
            keyring_service_for_build(false, Some("buzz-desktop".to_string())),
            PRODUCTION_KEYRING_SERVICE
        );
        assert_eq!(
            migration_marker_name(PRODUCTION_KEYRING_SERVICE, "identity.migrated"),
            "identity.com.saorsalabs.tictactoe.migrated"
        );
    }

    #[test]
    fn standalone_scope_must_remain_under_dev_service() {
        assert_eq!(
            dev_keyring_service(Some("buzz-desktop-dev.example".to_string())),
            "buzz-desktop-dev.example"
        );
        assert_eq!(
            dev_keyring_service(Some("buzz-desktop".to_string())),
            "buzz-desktop-dev"
        );
    }

    #[test]
    fn standalone_scope_uses_its_own_migration_marker() {
        assert_eq!(
            migration_marker_name("buzz-desktop", "identity.migrated"),
            "identity.migrated"
        );
        assert_eq!(
            migration_marker_name("buzz-desktop-dev", "identity.migrated"),
            "identity.migrated"
        );
        assert_eq!(
            migration_marker_name("buzz-desktop-dev.example", "identity.migrated"),
            "identity.buzz-desktop-dev.example.migrated"
        );
    }
}

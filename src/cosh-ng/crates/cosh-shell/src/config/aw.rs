//! User-owned AW references retain independent digests and one configuration source.

use serde::Deserialize;
use std::{ffi::OsString, path::Path};

pub(crate) const ENVIRONMENT_KEYS: [&str; 4] = [
    "COSH_AW_CONFIG",
    "COSH_AW_CONFIG_SHA256",
    "COSH_AW_HERDR",
    "COSH_AW_HERDR_SHA256",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AwConfig {
    pub config: String,
    pub config_sha256: String,
    pub herdr: Option<String>,
    pub herdr_sha256: Option<String>,
}

impl AwConfig {
    pub fn parse(value: Option<&toml::Value>) -> Result<Option<Self>, String> {
        let Some(value) = value else {
            return Ok(None);
        };
        let config: Self = value.clone().try_into().map_err(|_| {
            "invalid [aw] configuration: require config/config_sha256 strings and optional herdr/herdr_sha256 strings; unknown fields are rejected".to_owned()
        })?;
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> Result<(), String> {
        if !Path::new(&self.config).is_absolute() || self.config.contains('\0') {
            return Err("AW config must be an absolute path".into());
        }
        if !digest(&self.config_sha256) {
            return Err("AW config_sha256 must be a reviewed lowercase SHA-256 digest".into());
        }
        match (&self.herdr, &self.herdr_sha256) {
            (None, None) => Ok(()),
            (Some(path), Some(hash)) if Path::new(path).is_absolute() && !path.contains('\0') && digest(hash) => Ok(()),
            _ => Err("AW herdr and herdr_sha256 must specify an absolute path and lowercase SHA-256 together".into()),
        }
    }

    #[cfg(any(feature = "aw", test))]
    pub fn environment(&self) -> Vec<(String, String)> {
        let mut values = vec![
            (ENVIRONMENT_KEYS[0].into(), self.config.clone()),
            (ENVIRONMENT_KEYS[1].into(), self.config_sha256.clone()),
        ];
        if let (Some(path), Some(hash)) = (&self.herdr, &self.herdr_sha256) {
            values.push((ENVIRONMENT_KEYS[2].into(), path.clone()));
            values.push((ENVIRONMENT_KEYS[3].into(), hash.clone()));
        }
        values
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn select(
    user: &Result<Option<AwConfig>, String>,
    environment: [Option<OsString>; 4],
) -> Result<Option<AwConfig>, String> {
    if environment.iter().all(Option::is_none) {
        return user.clone();
    }
    let mut values = environment.into_iter().map(|value| {
        value
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| "AW environment values must be UTF-8".to_owned())
            })
            .transpose()
    });
    let config = AwConfig {
        config: values
            .next()
            .transpose()?
            .flatten()
            .ok_or("explicit AW environment requires COSH_AW_CONFIG")?,
        config_sha256: values
            .next()
            .transpose()?
            .flatten()
            .ok_or("explicit AW environment requires COSH_AW_CONFIG_SHA256")?,
        herdr: values.next().transpose()?.flatten(),
        herdr_sha256: values.next().transpose()?.flatten(),
    };
    config.validate()?;
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> AwConfig {
        AwConfig {
            config: "/trusted/aw.json".into(),
            config_sha256: "a".repeat(64),
            herdr: Some("/trusted/herdr".into()),
            herdr_sha256: Some("b".repeat(64)),
        }
    }

    #[test]
    fn user_profile_is_selected_without_environment_and_exports_complete_values() {
        let expected = configured();
        let selected = select(&Ok(Some(expected.clone())), [None, None, None, None]).unwrap();
        assert_eq!(selected, Some(expected.clone()));
        assert_eq!(expected.environment().len(), 4);
        assert_eq!(select(&Ok(None), [None, None, None, None]).unwrap(), None);
    }

    #[test]
    fn explicit_environment_never_borrows_user_digest_or_herdr() {
        let user = Ok(Some(configured()));
        assert!(select(&user, [Some("/override.json".into()), None, None, None]).is_err());
        assert!(select(
            &user,
            [
                None,
                None,
                Some("/herdr".into()),
                Some("b".repeat(64).into())
            ]
        )
        .is_err());
        let chosen = select(
            &user,
            [
                Some("/override.json".into()),
                Some("c".repeat(64).into()),
                None,
                None,
            ],
        )
        .unwrap()
        .unwrap();
        assert_eq!(chosen.config, "/override.json");
        assert_eq!(chosen.herdr, None);
        assert_eq!(chosen.environment().len(), 2);
    }

    #[test]
    fn malformed_or_partial_user_configuration_is_visible() {
        for source in [
            "aw = false",
            "[aw]",
            "[aw]\nconfig = '/aw.json'",
            "[aw]\nconfig = 3\nconfig_sha256 = 'wrong'",
        ] {
            let value: toml::Value = source.parse().unwrap();
            assert!(AwConfig::parse(value.get("aw")).is_err(), "{source}");
        }
        let source = format!(
            "[aw]\nconfig = '/aw.json'\nconfig_sha256 = '{}'\nherdr = '/herdr'",
            "a".repeat(64)
        );
        let value: toml::Value = source.parse().unwrap();
        assert!(AwConfig::parse(value.get("aw")).is_err());
    }

    #[test]
    fn user_configuration_requires_absolute_paths_and_explicit_valid_digests() {
        let mut config = configured();
        config.config = "relative.json".into();
        assert!(config.validate().is_err());
        config = configured();
        config.config_sha256 = "A".repeat(64);
        assert!(config.validate().is_err());
        config = configured();
        config.herdr_sha256 = None;
        assert!(config.validate().is_err());
        let source = format!(
            "[aw]\nconfig='/aw.json'\nconfig_sha256='{}'\nherdr='/herdr'\nherdr_sha256='{}'",
            "a".repeat(64),
            "b".repeat(64)
        );
        let value: toml::Value = source.parse().unwrap();
        assert!(AwConfig::parse(value.get("aw")).unwrap().is_some());
    }
    #[test]
    fn user_toml_loader_preserves_aw_errors_and_complete_environment_can_override() {
        let mut config = crate::config::CoshConfig::default();
        let source = format!(
            "[aw]\nconfig='/aw.json'\nconfig_sha256='{}'",
            "a".repeat(64)
        );
        super::super::parse::parse_toml_config(&source, &mut config);
        assert_eq!(
            config.aw.as_ref().unwrap().as_ref().unwrap().config,
            "/aw.json"
        );
        super::super::parse::parse_toml_config("[aw]\nconfig = [", &mut config);
        assert!(select(&config.aw, [None, None, None, None]).is_err());
        let override_profile = select(
            &config.aw,
            [
                Some("/override.json".into()),
                Some("b".repeat(64).into()),
                None,
                None,
            ],
        )
        .unwrap()
        .unwrap();
        assert_eq!(override_profile.config, "/override.json");
    }
}

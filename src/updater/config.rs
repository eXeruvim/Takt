use std::{path::Path, str::FromStr};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use semver::Version;
use url::Url;

use super::{
    manifest::{PackageKind, ReleaseChannel},
    service::CheckRequest,
};

const MANIFEST_URL: &str = env!("TAKT_UPDATE_MANIFEST_URL");
const PUBLIC_KEY: &str = env!("TAKT_UPDATE_PUBLIC_KEY");

pub fn current_request() -> Result<Option<CheckRequest>, String> {
    if MANIFEST_URL.trim().is_empty() {
        return Ok(None);
    }
    let manifest_url = Url::parse(MANIFEST_URL)
        .map_err(|error| format!("некорректный адрес обновлений: {error}"))?;
    let key = URL_SAFE_NO_PAD
        .decode(PUBLIC_KEY.trim())
        .map_err(|_| "некорректный публичный ключ обновлений".to_owned())?;
    let public_key: [u8; 32] = key
        .try_into()
        .map_err(|_| "публичный ключ обновлений имеет неверную длину".to_owned())?;
    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    let parsed = Version::from_str(&current_version)
        .map_err(|error| format!("некорректная версия приложения: {error}"))?;

    Ok(Some(CheckRequest {
        manifest_url,
        public_key,
        current_version,
        channel: release_channel(&parsed),
        package_kind: installed_package_kind(),
        architecture: format!("{}-windows", std::env::consts::ARCH),
        allowed_download_hosts: vec![
            "github.com".to_owned(),
            "objects.githubusercontent.com".to_owned(),
        ],
    }))
}

fn release_channel(version: &Version) -> ReleaseChannel {
    let prerelease = version.pre.as_str();
    if prerelease.starts_with("alpha") {
        ReleaseChannel::Alpha
    } else if prerelease.starts_with("beta") || prerelease.starts_with("rc") {
        ReleaseChannel::Beta
    } else {
        ReleaseChannel::Stable
    }
}

fn installed_package_kind() -> PackageKind {
    let installer_marker = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .is_some_and(|directory| directory.join("unins000.exe").is_file());
    if installer_marker {
        PackageKind::Installer
    } else {
        PackageKind::Portable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_prerelease_channels_without_treating_rc_as_stable() {
        assert_eq!(
            release_channel(&Version::parse("1.0.0-alpha.1").unwrap()),
            ReleaseChannel::Alpha
        );
        assert_eq!(
            release_channel(&Version::parse("1.0.0-beta.2").unwrap()),
            ReleaseChannel::Beta
        );
        assert_eq!(
            release_channel(&Version::parse("1.0.0-rc.1").unwrap()),
            ReleaseChannel::Beta
        );
        assert_eq!(
            release_channel(&Version::parse("1.0.0").unwrap()),
            ReleaseChannel::Stable
        );
    }
}

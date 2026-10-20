use std::{
    fmt,
    fs::File,
    io::{self, Read},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

const MANIFEST_DOMAIN: &[u8] = b"takt-update-manifest-v1\0";
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_RELEASE_NOTES: usize = 24;
pub const MAX_RELEASE_NOTE_BYTES: usize = 4 * 1024;
pub const MAX_RELEASE_NOTES_BYTES: usize = 32 * 1024;
pub const MAX_PACKAGES: usize = 4;
pub const MAX_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Alpha,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Installer,
    Portable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePackage {
    pub kind: PackageKind,
    pub architecture: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    pub schema: u32,
    pub channel: ReleaseChannel,
    pub version: String,
    pub published_at: String,
    pub notes: Vec<String>,
    pub packages: Vec<UpdatePackage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEnvelope {
    pub signed: SignedManifest,
    pub signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePolicy<'a> {
    pub current_version: &'a str,
    pub channel: ReleaseChannel,
    pub package_kind: PackageKind,
    pub architecture: &'a str,
    pub allowed_download_hosts: &'a [&'a str],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedUpdate {
    pub version: Version,
    pub published_at: String,
    pub notes: Vec<String>,
    pub package: UpdatePackage,
}

#[derive(Debug)]
pub enum ManifestError {
    TooLarge,
    InvalidJson(String),
    InvalidPublicKey,
    InvalidSignatureEncoding,
    SignatureRejected,
    UnsupportedSchema(u32),
    InvalidVersion(String),
    NotNewer,
    WrongChannel,
    MissingPackage,
    DuplicatePackage,
    InvalidPackage(&'static str),
    UntrustedDownloadHost,
    InvalidNotes,
    FileIo(io::Error),
    PackageSizeMismatch,
    PackageHashMismatch,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(formatter, "манифест обновления слишком большой"),
            Self::InvalidJson(error) => {
                write!(formatter, "некорректный манифест обновления: {error}")
            }
            Self::InvalidPublicKey => write!(formatter, "некорректный публичный ключ обновлений"),
            Self::InvalidSignatureEncoding => write!(formatter, "некорректная подпись манифеста"),
            Self::SignatureRejected => write!(formatter, "подпись манифеста не прошла проверку"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "неподдерживаемая схема манифеста: {schema}")
            }
            Self::InvalidVersion(version) => {
                write!(formatter, "некорректная версия обновления: {version}")
            }
            Self::NotNewer => write!(formatter, "манифест не содержит более новой версии"),
            Self::WrongChannel => {
                write!(formatter, "манифест относится к другому каналу обновлений")
            }
            Self::MissingPackage => write!(formatter, "в манифесте нет подходящего пакета"),
            Self::DuplicatePackage => write!(formatter, "манифест содержит дублирующий пакет"),
            Self::InvalidPackage(reason) => {
                write!(formatter, "некорректный пакет обновления: {reason}")
            }
            Self::UntrustedDownloadHost => {
                write!(formatter, "источник пакета не входит в доверенный список")
            }
            Self::InvalidNotes => {
                write!(formatter, "описание обновления превышает безопасный размер")
            }
            Self::FileIo(error) => {
                write!(formatter, "не удалось прочитать пакет обновления: {error}")
            }
            Self::PackageSizeMismatch => write!(
                formatter,
                "размер загруженного пакета не совпадает с манифестом"
            ),
            Self::PackageHashMismatch => write!(
                formatter,
                "SHA-256 загруженного пакета не совпадает с манифестом"
            ),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FileIo(error) => Some(error),
            _ => None,
        }
    }
}

pub fn decode_public_key(encoded: &str) -> Result<[u8; 32], ManifestError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| ManifestError::InvalidPublicKey)?;
    decoded
        .try_into()
        .map_err(|_| ManifestError::InvalidPublicKey)
}

pub fn signing_bytes(manifest: &SignedManifest) -> Result<Vec<u8>, ManifestError> {
    let serialized = serde_json::to_vec(manifest)
        .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
    let mut message = Vec::with_capacity(MANIFEST_DOMAIN.len() + serialized.len());
    message.extend_from_slice(MANIFEST_DOMAIN);
    message.extend_from_slice(&serialized);
    Ok(message)
}

pub fn verify_and_select(
    input: &[u8],
    public_key: &[u8; 32],
    policy: &UpdatePolicy<'_>,
) -> Result<VerifiedUpdate, ManifestError> {
    if input.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let envelope: ManifestEnvelope = serde_json::from_slice(input)
        .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
    let verifying_key =
        VerifyingKey::from_bytes(public_key).map_err(|_| ManifestError::InvalidPublicKey)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(&envelope.signature)
        .map_err(|_| ManifestError::InvalidSignatureEncoding)?;
    let signature_bytes: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| ManifestError::InvalidSignatureEncoding)?;
    let signature = Signature::from_bytes(&signature_bytes);
    let message = signing_bytes(&envelope.signed)?;
    verifying_key
        .verify_strict(&message, &signature)
        .map_err(|_| ManifestError::SignatureRejected)?;

    validate_and_select(envelope.signed, policy)
}

fn validate_and_select(
    manifest: SignedManifest,
    policy: &UpdatePolicy<'_>,
) -> Result<VerifiedUpdate, ManifestError> {
    if manifest.schema != 1 {
        return Err(ManifestError::UnsupportedSchema(manifest.schema));
    }
    if manifest.channel != policy.channel {
        return Err(ManifestError::WrongChannel);
    }
    let version = Version::parse(&manifest.version)
        .map_err(|_| ManifestError::InvalidVersion(manifest.version.clone()))?;
    let current = Version::parse(policy.current_version)
        .map_err(|_| ManifestError::InvalidVersion(policy.current_version.to_owned()))?;
    if version <= current {
        return Err(ManifestError::NotNewer);
    }
    if manifest.published_at.is_empty() || manifest.published_at.len() > 64 {
        return Err(ManifestError::InvalidPackage(
            "дата публикации отсутствует или слишком длинная",
        ));
    }
    if manifest.notes.len() > MAX_RELEASE_NOTES
        || manifest
            .notes
            .iter()
            .any(|note| note.len() > MAX_RELEASE_NOTE_BYTES)
        || manifest.notes.iter().map(String::len).sum::<usize>() > MAX_RELEASE_NOTES_BYTES
    {
        return Err(ManifestError::InvalidNotes);
    }
    if manifest.packages.is_empty() || manifest.packages.len() > MAX_PACKAGES {
        return Err(ManifestError::MissingPackage);
    }

    let mut selected = None;
    for package in manifest.packages {
        validate_package(&package, policy)?;
        if package.kind == policy.package_kind
            && package.architecture == policy.architecture
            && selected.replace(package).is_some()
        {
            return Err(ManifestError::DuplicatePackage);
        }
    }
    let package = selected.ok_or(ManifestError::MissingPackage)?;
    Ok(VerifiedUpdate {
        version,
        published_at: manifest.published_at,
        notes: manifest.notes,
        package,
    })
}

fn validate_package(
    package: &UpdatePackage,
    policy: &UpdatePolicy<'_>,
) -> Result<(), ManifestError> {
    if package.size == 0 || package.size > MAX_PACKAGE_BYTES {
        return Err(ManifestError::InvalidPackage("недопустимый размер"));
    }
    if package.architecture.is_empty() || package.architecture.len() > 32 {
        return Err(ManifestError::InvalidPackage("недопустимая архитектура"));
    }
    if package.sha256.len() != 64 || !package.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::InvalidPackage("некорректный SHA-256"));
    }
    let url =
        Url::parse(&package.url).map_err(|_| ManifestError::InvalidPackage("некорректный URL"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ManifestError::InvalidPackage(
            "разрешён только HTTPS URL без credentials и fragment",
        ));
    }
    let host = url.host_str().ok_or(ManifestError::UntrustedDownloadHost)?;
    if !policy
        .allowed_download_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return Err(ManifestError::UntrustedDownloadHost);
    }
    Ok(())
}

pub fn verify_package_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), ManifestError> {
    let mut file = File::open(path).map_err(ManifestError::FileIo)?;
    let metadata = file.metadata().map_err(ManifestError::FileIo)?;
    if metadata.len() != expected_size {
        return Err(ManifestError::PackageSizeMismatch);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(ManifestError::FileIo)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex_lower(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(ManifestError::PackageHashMismatch);
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;

    fn signed_manifest(mut manifest: SignedManifest, key: &SigningKey) -> Vec<u8> {
        manifest.packages.sort_by_key(|package| match package.kind {
            PackageKind::Installer => 0,
            PackageKind::Portable => 1,
        });
        let signature = key.sign(&signing_bytes(&manifest).unwrap());
        serde_json::to_vec(&ManifestEnvelope {
            signed: manifest,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        })
        .unwrap()
    }

    fn manifest() -> SignedManifest {
        SignedManifest {
            schema: 1,
            channel: ReleaseChannel::Stable,
            version: "1.2.0".to_owned(),
            published_at: "2026-10-14T00:00:00Z".to_owned(),
            notes: vec!["Быстрее открывается очередь".to_owned()],
            packages: vec![
                UpdatePackage {
                    kind: PackageKind::Installer,
                    architecture: "x86_64-windows".to_owned(),
                    url: "https://github.com/example/takt/releases/download/v1.2.0/setup.exe"
                        .to_owned(),
                    sha256: "11".repeat(32),
                    size: 42,
                },
                UpdatePackage {
                    kind: PackageKind::Portable,
                    architecture: "x86_64-windows".to_owned(),
                    url: "https://github.com/example/takt/releases/download/v1.2.0/portable.zip"
                        .to_owned(),
                    sha256: "22".repeat(32),
                    size: 84,
                },
            ],
        }
    }

    fn policy(kind: PackageKind) -> UpdatePolicy<'static> {
        UpdatePolicy {
            current_version: "1.1.0",
            channel: ReleaseChannel::Stable,
            package_kind: kind,
            architecture: "x86_64-windows",
            allowed_download_hosts: &["github.com"],
        }
    }

    #[test]
    fn accepts_valid_signature_and_selects_exact_package_kind() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let input = signed_manifest(manifest(), &key);
        let update = verify_and_select(
            &input,
            &key.verifying_key().to_bytes(),
            &policy(PackageKind::Portable),
        )
        .unwrap();
        assert_eq!(update.version, Version::new(1, 2, 0));
        assert_eq!(update.package.kind, PackageKind::Portable);
        assert_eq!(update.notes, ["Быстрее открывается очередь"]);
    }

    #[test]
    fn rejects_tampering_wrong_key_and_unknown_fields() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut input = signed_manifest(manifest(), &key);
        let position = input
            .windows(5)
            .position(|bytes| bytes == b"1.2.0")
            .unwrap();
        input[position + 2] = b'3';
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::SignatureRejected)
        ));

        let input = signed_manifest(manifest(), &key);
        let other = SigningKey::from_bytes(&[9; 32]);
        assert!(matches!(
            verify_and_select(
                &input,
                &other.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::SignatureRejected)
        ));

        let mut value: serde_json::Value = serde_json::from_slice(&input).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(matches!(
            verify_and_select(
                &serde_json::to_vec(&value).unwrap(),
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::InvalidJson(_))
        ));
    }

    #[test]
    fn rejects_downgrade_channel_source_and_duplicate_selection() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut candidate = manifest();
        candidate.version = "1.1.0".to_owned();
        let input = signed_manifest(candidate, &key);
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::NotNewer)
        ));

        let mut candidate = manifest();
        candidate.channel = ReleaseChannel::Beta;
        let input = signed_manifest(candidate, &key);
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::WrongChannel)
        ));

        let mut candidate = manifest();
        candidate.packages[0].url = "https://downloads.invalid/setup.exe".to_owned();
        let input = signed_manifest(candidate, &key);
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::UntrustedDownloadHost)
        ));

        let mut candidate = manifest();
        candidate.packages.push(candidate.packages[0].clone());
        let input = signed_manifest(candidate, &key);
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::DuplicatePackage)
        ));
    }

    #[test]
    fn package_file_requires_exact_size_and_sha256() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "takt-update-hash-{}-{unique}.bin",
            std::process::id()
        ));
        fs::write(&path, b"verified update payload").unwrap();
        let expected = hex_lower(&Sha256::digest(b"verified update payload"));
        verify_package_file(&path, 23, &expected).unwrap();
        assert!(matches!(
            verify_package_file(&path, 22, &expected),
            Err(ManifestError::PackageSizeMismatch)
        ));
        assert!(matches!(
            verify_package_file(&path, 23, &"00".repeat(32)),
            Err(ManifestError::PackageHashMismatch)
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn manifest_and_notes_have_hard_limits() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(matches!(
            verify_and_select(
                &oversized,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::TooLarge)
        ));

        let mut candidate = manifest();
        candidate.notes = vec!["x".repeat(MAX_RELEASE_NOTE_BYTES + 1)];
        let input = signed_manifest(candidate, &key);
        assert!(matches!(
            verify_and_select(
                &input,
                &key.verifying_key().to_bytes(),
                &policy(PackageKind::Installer)
            ),
            Err(ManifestError::InvalidNotes)
        ));
    }
}

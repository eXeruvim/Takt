use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use oxide::updater::manifest::{
    MAX_MANIFEST_BYTES, ManifestEnvelope, PackageKind, ReleaseChannel, SignedManifest,
    UpdatePolicy, decode_public_key, signing_bytes, verify_and_select,
};

const PRIVATE_KEY_ENV: &str = "OXIDE_UPDATE_SIGNING_KEY";
const PRIVATE_KEY_FILE_ENV: &str = "OXIDE_UPDATE_SIGNING_KEY_FILE";

fn main() {
    if let Err(error) = run(env::args_os().skip(1).collect()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(arguments: Vec<std::ffi::OsString>) -> Result<(), String> {
    let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
        return Err(usage());
    };
    match command {
        "generate-key" if arguments.len() == 3 => {
            generate_key(Path::new(&arguments[1]), Path::new(&arguments[2]))
        }
        "public-key" if arguments.len() == 1 => {
            println!("{}", encode_public_key(&load_signing_key()?));
            Ok(())
        }
        "sign" if arguments.len() == 3 => {
            sign_manifest(Path::new(&arguments[1]), Path::new(&arguments[2]))
        }
        "verify" if arguments.len() == 8 => verify_manifest(&arguments),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    format!(
        "Использование:\n  oxide-update-manifest generate-key <private.txt> <public.txt>\n  oxide-update-manifest public-key\n  oxide-update-manifest sign <unsigned.json> <signed.json>\n  oxide-update-manifest verify <signed.json> <public-key> <current-version> <channel> <kind> <architecture> <allowed-host>\n\nДля sign/public-key задайте {PRIVATE_KEY_ENV} или {PRIVATE_KEY_FILE_ENV}."
    )
}

fn generate_key(private_path: &Path, public_path: &Path) -> Result<(), String> {
    ensure_distinct_paths(private_path, public_path)?;
    if private_path.exists() || public_path.exists() {
        return Err("Файл ключа уже существует; существующие ключи не перезаписываются".to_owned());
    }
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|error| format!("Не удалось получить системную случайность: {error}"))?;
    let key = SigningKey::from_bytes(&seed);
    write_new(private_path, &format!("{}\n", URL_SAFE_NO_PAD.encode(seed)))?;
    if let Err(error) = write_new(public_path, &format!("{}\n", encode_public_key(&key))) {
        let _ = fs::remove_file(private_path);
        return Err(error);
    }
    seed.fill(0);
    Ok(())
}

fn sign_manifest(input_path: &Path, output_path: &Path) -> Result<(), String> {
    ensure_distinct_paths(input_path, output_path)?;
    let input = read_bounded(input_path, MAX_MANIFEST_BYTES)?;
    let signed: SignedManifest = serde_json::from_slice(&input)
        .map_err(|error| format!("Некорректный unsigned manifest: {error}"))?;
    let key = load_signing_key()?;
    let signature = key.sign(&signing_bytes(&signed).map_err(|error| error.to_string())?);
    let envelope = ManifestEnvelope {
        signed,
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    };
    let mut output = serde_json::to_vec_pretty(&envelope)
        .map_err(|error| format!("Не удалось сериализовать манифест: {error}"))?;
    output.push(b'\n');
    fs::write(output_path, output)
        .map_err(|error| format!("Не удалось записать {}: {error}", output_path.display()))
}

fn verify_manifest(arguments: &[std::ffi::OsString]) -> Result<(), String> {
    let input = read_bounded(Path::new(&arguments[1]), MAX_MANIFEST_BYTES)?;
    let public_key_text = arguments[2]
        .to_str()
        .ok_or_else(|| "Публичный ключ должен быть UTF-8".to_owned())?;
    let public_key = decode_public_key(public_key_text).map_err(|error| error.to_string())?;
    let channel = parse_channel(&arguments[4])?;
    let kind = parse_kind(&arguments[5])?;
    let current_version = arguments[3]
        .to_str()
        .ok_or_else(|| "Текущая версия должна быть UTF-8".to_owned())?;
    let architecture = arguments[6]
        .to_str()
        .ok_or_else(|| "Архитектура должна быть UTF-8".to_owned())?;
    let host = arguments[7]
        .to_str()
        .ok_or_else(|| "Имя сервера должно быть UTF-8".to_owned())?;
    let update = verify_and_select(
        &input,
        &public_key,
        &UpdatePolicy {
            current_version,
            channel,
            package_kind: kind,
            architecture,
            allowed_download_hosts: &[host],
        },
    )
    .map_err(|error| error.to_string())?;
    println!("verified {} {}", update.version, update.package.sha256);
    Ok(())
}

fn parse_channel(value: &std::ffi::OsStr) -> Result<ReleaseChannel, String> {
    match value.to_str() {
        Some("stable") => Ok(ReleaseChannel::Stable),
        Some("beta") => Ok(ReleaseChannel::Beta),
        Some("alpha") => Ok(ReleaseChannel::Alpha),
        _ => Err("Канал должен быть stable, beta или alpha".to_owned()),
    }
}

fn parse_kind(value: &std::ffi::OsStr) -> Result<PackageKind, String> {
    match value.to_str() {
        Some("installer") => Ok(PackageKind::Installer),
        Some("portable") => Ok(PackageKind::Portable),
        _ => Err("Вид пакета должен быть installer или portable".to_owned()),
    }
}

fn load_signing_key() -> Result<SigningKey, String> {
    let encoded = match env::var(PRIVATE_KEY_ENV) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            let path = env::var_os(PRIVATE_KEY_FILE_ENV)
                .ok_or_else(|| format!("Задайте {PRIVATE_KEY_ENV} или {PRIVATE_KEY_FILE_ENV}"))?;
            fs::read_to_string(&path).map_err(|error| {
                format!(
                    "Не удалось прочитать {}: {error}",
                    PathBuf::from(path).display()
                )
            })?
        }
    };
    let mut decoded = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|_| "Приватный ключ должен быть base64url без padding".to_owned())?;
    if decoded.len() != 32 {
        decoded.fill(0);
        return Err("Приватный ключ должен содержать ровно 32 байта".to_owned());
    }
    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&decoded);
    decoded.fill(0);
    let key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    Ok(key)
}

fn encode_public_key(key: &SigningKey) -> String {
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes())
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
    if metadata.len() > maximum as u64 {
        return Err(format!("Файл {} превышает {maximum} байт", path.display()));
    }
    fs::read(path).map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))
}

fn write_new(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Не удалось создать {}: {error}", parent.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("Не удалось создать {}: {error}", path.display()))?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Не удалось записать {}: {error}", path.display()))
}

fn ensure_distinct_paths(first: &Path, second: &Path) -> Result<(), String> {
    let first = absolute(first)?;
    let second = absolute(second)?;
    if first == second {
        Err("Входной и выходной пути должны отличаться".to_owned())
    } else {
        Ok(())
    }
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| format!("Не удалось определить текущий каталог: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_unknown_channel_and_kind() {
        assert!(parse_channel(std::ffi::OsStr::new("nightly")).is_err());
        assert!(parse_kind(std::ffi::OsStr::new("archive")).is_err());
    }

    #[test]
    fn generated_public_key_has_expected_wire_size() {
        let key = SigningKey::from_bytes(&[3; 32]);
        assert_eq!(
            decode_public_key(&encode_public_key(&key)).unwrap().len(),
            32
        );
    }
}

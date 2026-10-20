use std::{fmt, io, thread};

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use url::Url;

use super::{
    http::{self, HttpError},
    manifest::{
        MAX_MANIFEST_BYTES, ManifestError, PackageKind, ReleaseChannel, UpdatePolicy,
        VerifiedUpdate, verify_and_select,
    },
};

const COMMAND_CAPACITY: usize = 1;
const EVENT_CAPACITY: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckRequest {
    pub manifest_url: Url,
    pub public_key: [u8; 32],
    pub current_version: String,
    pub channel: ReleaseChannel,
    pub package_kind: PackageKind,
    pub architecture: String,
    pub allowed_download_hosts: Vec<String>,
}

impl CheckRequest {
    fn policy(&self) -> UpdatePolicy<'_> {
        UpdatePolicy {
            current_version: &self.current_version,
            channel: self.channel,
            package_kind: self.package_kind,
            architecture: &self.architecture,
            allowed_download_hosts: &[],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckEvent {
    Checking,
    Available(VerifiedUpdate),
    UpToDate,
    Failed(CheckFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckFailure {
    Network(String),
    Verification(String),
}

impl fmt::Display for CheckFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(message) | Self::Verification(message) => formatter.write_str(message),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Busy,
    Unavailable,
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(formatter, "проверка обновлений уже ожидает выполнения"),
            Self::Unavailable => write!(formatter, "служба обновлений недоступна"),
        }
    }
}

impl std::error::Error for RequestError {}

pub struct CheckService {
    commands: Option<Sender<CheckRequest>>,
    events: Receiver<CheckEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl CheckService {
    pub fn spawn() -> io::Result<Self> {
        Self::spawn_with(http::get_https)
    }

    pub fn request(&self, request: CheckRequest) -> Result<(), RequestError> {
        let Some(commands) = &self.commands else {
            return Err(RequestError::Unavailable);
        };
        match commands.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(RequestError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(RequestError::Unavailable),
        }
    }

    pub fn try_event(&self) -> Result<CheckEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub fn event_receiver(&self) -> Receiver<CheckEvent> {
        self.events.clone()
    }

    fn spawn_with<F>(fetch: F) -> io::Result<Self>
    where
        F: Fn(&Url, usize) -> Result<Vec<u8>, HttpError> + Send + 'static,
    {
        let (commands, worker_commands) = bounded(COMMAND_CAPACITY);
        let (worker_events, events) = bounded(EVENT_CAPACITY);
        let worker = thread::Builder::new()
            .name("takt-update-check".to_owned())
            .spawn(move || run_worker(worker_commands, worker_events, fetch))?;
        Ok(Self {
            commands: Some(commands),
            events,
            worker: Some(worker),
        })
    }
}

impl Drop for CheckService {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker<F>(commands: Receiver<CheckRequest>, events: Sender<CheckEvent>, fetch: F)
where
    F: Fn(&Url, usize) -> Result<Vec<u8>, HttpError>,
{
    while let Ok(request) = commands.recv() {
        send_latest(&events, CheckEvent::Checking);
        let event = check_once(&request, &fetch);
        send_latest(&events, event);
    }
}

fn check_once<F>(request: &CheckRequest, fetch: &F) -> CheckEvent
where
    F: Fn(&Url, usize) -> Result<Vec<u8>, HttpError>,
{
    let input = match fetch(&request.manifest_url, MAX_MANIFEST_BYTES) {
        Ok(input) => input,
        Err(error) => return CheckEvent::Failed(CheckFailure::Network(error.to_string())),
    };
    let host_storage: Vec<&str> = request
        .allowed_download_hosts
        .iter()
        .map(String::as_str)
        .collect();
    let mut policy = request.policy();
    policy.allowed_download_hosts = &host_storage;
    match verify_and_select(&input, &request.public_key, &policy) {
        Ok(update) => CheckEvent::Available(update),
        Err(ManifestError::NotNewer) => CheckEvent::UpToDate,
        Err(error) => CheckEvent::Failed(CheckFailure::Verification(error.to_string())),
    }
}

fn send_latest(events: &Sender<CheckEvent>, event: CheckEvent) {
    let _ = events.try_send(event);
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;
    use crate::updater::manifest::{
        ManifestEnvelope, SignedManifest, UpdatePackage, signing_bytes,
    };

    fn fixture(version: &str) -> (CheckRequest, Vec<u8>) {
        let key = SigningKey::from_bytes(&[11; 32]);
        let signed = SignedManifest {
            schema: 1,
            channel: ReleaseChannel::Stable,
            version: version.to_owned(),
            published_at: "2026-10-14T00:00:00Z".to_owned(),
            notes: vec!["Новая версия".to_owned()],
            packages: vec![UpdatePackage {
                kind: PackageKind::Installer,
                architecture: "x86_64-windows".to_owned(),
                url: "https://github.com/example/takt/setup.exe".to_owned(),
                sha256: "11".repeat(32),
                size: 1024,
            }],
        };
        let signature = key.sign(&signing_bytes(&signed).unwrap());
        let input = serde_json::to_vec(&ManifestEnvelope {
            signed,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        })
        .unwrap();
        (
            CheckRequest {
                manifest_url: Url::parse("https://updates.example/manifest.json").unwrap(),
                public_key: key.verifying_key().to_bytes(),
                current_version: "1.0.0".to_owned(),
                channel: ReleaseChannel::Stable,
                package_kind: PackageKind::Installer,
                architecture: "x86_64-windows".to_owned(),
                allowed_download_hosts: vec!["github.com".to_owned()],
            },
            input,
        )
    }

    #[test]
    fn worker_reports_checking_then_verified_update_without_blocking_caller() {
        let (request, input) = fixture("1.1.0");
        let input = Arc::new(input);
        let service = CheckService::spawn_with(move |_, _| Ok(input.as_ref().clone())).unwrap();
        service.request(request).unwrap();
        assert_eq!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::Checking
        );
        assert!(matches!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::Available(_)
        ));
    }

    #[test]
    fn equal_version_is_a_quiet_up_to_date_result() {
        let (request, input) = fixture("1.0.0");
        let service = CheckService::spawn_with(move |_, _| Ok(input.clone())).unwrap();
        service.request(request).unwrap();
        assert_eq!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::Checking
        );
        assert_eq!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::UpToDate
        );
    }

    #[test]
    fn fetch_and_signature_failures_are_distinct_and_bounded() {
        let (request, _) = fixture("1.1.0");
        let service = CheckService::spawn_with(|_, _| {
            Err(HttpError::Request(io::Error::new(
                io::ErrorKind::TimedOut,
                "timeout",
            )))
        })
        .unwrap();
        service.request(request).unwrap();
        let _ = service.events.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::Failed(CheckFailure::Network(_))
        ));

        let (request, _) = fixture("1.1.0");
        let service = CheckService::spawn_with(|_, _| Ok(b"{}".to_vec())).unwrap();
        service.request(request).unwrap();
        let _ = service.events.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            service.events.recv_timeout(Duration::from_secs(1)).unwrap(),
            CheckEvent::Failed(CheckFailure::Verification(_))
        ));
    }
}

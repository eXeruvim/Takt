use std::{fmt, io};

use url::Url;

#[derive(Debug)]
pub enum HttpError {
    InvalidUrl(&'static str),
    Request(io::Error),
    Status(u32),
    ResponseTooLarge,
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(reason) => {
                write!(formatter, "некорректный адрес обновления: {reason}")
            }
            Self::Request(error) => write!(formatter, "не удалось получить обновление: {error}"),
            Self::Status(status) => write!(formatter, "сервер обновлений вернул HTTP {status}"),
            Self::ResponseTooLarge => write!(
                formatter,
                "ответ сервера обновлений превышает допустимый размер"
            ),
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            _ => None,
        }
    }
}

pub fn get_https(url: &Url, maximum_bytes: usize) -> Result<Vec<u8>, HttpError> {
    validate_url(url)?;
    platform::get_https(url, maximum_bytes)
}

fn validate_url(url: &Url) -> Result<(), HttpError> {
    if url.scheme() != "https" {
        return Err(HttpError::InvalidUrl("разрешён только HTTPS"));
    }
    if url.host_str().is_none() {
        return Err(HttpError::InvalidUrl("не указан сервер"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::InvalidUrl("логин и пароль в адресе запрещены"));
    }
    if url.fragment().is_some() {
        return Err(HttpError::InvalidUrl("fragment в адресе запрещён"));
    }
    Ok(())
}

fn append_bounded(
    output: &mut Vec<u8>,
    bytes: &[u8],
    maximum_bytes: usize,
) -> Result<(), HttpError> {
    if bytes.len() > maximum_bytes.saturating_sub(output.len()) {
        return Err(HttpError::ResponseTooLarge);
    }
    output.extend_from_slice(bytes);
    Ok(())
}

#[cfg(windows)]
mod platform {
    use std::{ffi::c_void, io, ptr};

    use url::Url;
    use windows_sys::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY,
        WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
    };

    use super::{HttpError, append_bounded};

    const RESOLVE_TIMEOUT_MS: i32 = 5_000;
    const CONNECT_TIMEOUT_MS: i32 = 10_000;
    const SEND_TIMEOUT_MS: i32 = 10_000;
    const RECEIVE_TIMEOUT_MS: i32 = 15_000;
    const READ_BUFFER_BYTES: usize = 32 * 1024;

    struct InternetHandle(*mut c_void);

    impl InternetHandle {
        fn new(value: *mut c_void) -> Result<Self, HttpError> {
            if value.is_null() {
                Err(HttpError::Request(io::Error::last_os_error()))
            } else {
                Ok(Self(value))
            }
        }
    }

    impl Drop for InternetHandle {
        fn drop(&mut self) {
            // SAFETY: the constructor accepts only a non-null WinHTTP handle and
            // ownership is never copied out of this wrapper.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    pub(super) fn get_https(url: &Url, maximum_bytes: usize) -> Result<Vec<u8>, HttpError> {
        let agent = wide("Oxide updater/1");
        // SAFETY: all strings are terminated UTF-16 buffers and optional WinHTTP
        // pointers are null as required by the synchronous API.
        let session = InternetHandle::new(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                ptr::null(),
                ptr::null(),
                0,
            )
        })?;
        // SAFETY: session is a live WinHTTP session handle.
        if unsafe {
            WinHttpSetTimeouts(
                session.0,
                RESOLVE_TIMEOUT_MS,
                CONNECT_TIMEOUT_MS,
                SEND_TIMEOUT_MS,
                RECEIVE_TIMEOUT_MS,
            )
        } == 0
        {
            return Err(last_error());
        }

        let host = wide(url.host_str().expect("URL host was validated"));
        let port = url
            .port_or_known_default()
            .ok_or(HttpError::InvalidUrl("не указан порт"))?;
        // SAFETY: host is terminated and session remains alive for the handle lifetime.
        let connection =
            InternetHandle::new(unsafe { WinHttpConnect(session.0, host.as_ptr(), port, 0) })?;

        let verb = wide("GET");
        let mut path = url.path().to_owned();
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        let path = wide(&path);
        // SAFETY: connection is live; verb/path are terminated; optional pointers are null.
        let request = InternetHandle::new(unsafe {
            WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;

        let redirect_policy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;
        // SAFETY: request is live and the option buffer points to a u32 for its full size.
        if unsafe {
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&raw const redirect_policy).cast(),
                size_of::<u32>() as u32,
            )
        } == 0
        {
            return Err(last_error());
        }

        // SAFETY: request is live; this GET has no additional headers or body.
        if unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null(), 0, 0, 0) } == 0 {
            return Err(last_error());
        }
        // SAFETY: request is live and the reserved argument must be null.
        if unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
            return Err(last_error());
        }

        let mut status = 0_u32;
        let mut status_bytes = size_of::<u32>() as u32;
        // SAFETY: status and its byte count are valid output buffers.
        if unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                (&raw mut status).cast(),
                &raw mut status_bytes,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        if status != 200 {
            return Err(HttpError::Status(status));
        }

        let mut output = Vec::new();
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        loop {
            let mut bytes_read = 0_u32;
            // SAFETY: request is live and buffer/length describe writable storage.
            if unsafe {
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &raw mut bytes_read,
                )
            } == 0
            {
                return Err(last_error());
            }
            if bytes_read == 0 {
                break;
            }
            append_bounded(&mut output, &buffer[..bytes_read as usize], maximum_bytes)?;
        }
        Ok(output)
    }

    fn last_error() -> HttpError {
        HttpError::Request(io::Error::last_os_error())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    use url::Url;

    use super::HttpError;

    pub(super) fn get_https(_url: &Url, _maximum_bytes: usize) -> Result<Vec<u8>, HttpError> {
        Err(HttpError::Request(io::Error::new(
            io::ErrorKind::Unsupported,
            "проверка обновлений пока доступна только в Windows",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_insecure_or_credentialed_urls_before_network_access() {
        for value in [
            "http://updates.example/manifest.json",
            "https://user:password@updates.example/manifest.json",
            "https://updates.example/manifest.json#fragment",
        ] {
            let url = Url::parse(value).unwrap();
            assert!(matches!(validate_url(&url), Err(HttpError::InvalidUrl(_))));
        }
    }

    #[test]
    fn bounded_response_never_grows_past_the_limit() {
        let mut output = vec![1, 2, 3];
        append_bounded(&mut output, &[4, 5], 5).unwrap();
        assert_eq!(output, [1, 2, 3, 4, 5]);
        assert!(matches!(
            append_bounded(&mut output, &[6], 5),
            Err(HttpError::ResponseTooLarge)
        ));
        assert_eq!(output, [1, 2, 3, 4, 5]);
    }
}

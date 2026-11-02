//! Cancellable `WinHTTP` requests. Only the update worker waits or writes response bytes.
use crate::win32::wide;
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::io::Write;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ASYNC_RESULT,
    WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
    WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
    WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
    WINHTTP_FLAG_ASYNC, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_AUTOLOGON_POLICY,
    WINHTTP_OPTION_CONTEXT_VALUE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetStatusCallback, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

const BUFFER_BYTES: u32 = 16 * 1024;
const BUFFER_SIZE: usize = BUFFER_BYTES as usize;
const TIMED_OUT: &str = "The update download timed out";

struct Internet(*mut c_void);
impl Internet {
    fn new(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(windows::core::Error::from_thread().to_string())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        // SAFETY: owns this handle exactly once; children are dropped before their parents.
        if let Err(error) = unsafe { WinHttpCloseHandle(self.0) } {
            eprintln!("Could not close an update connection: {error}");
        }
    }
}

type Completion = Result<(u32, u32), u32>;

struct Callback {
    sender: SyncSender<Completion>,
    // WinHTTP writes only during an outstanding read. The worker reads only after its
    // completion crosses the channel. Cancellation retains this allocation until HANDLE_CLOSING.
    buffer: UnsafeCell<[u8; BUFFER_SIZE]>,
}

unsafe extern "system" fn completed(
    _handle: *mut c_void,
    context: usize,
    status: u32,
    information: *mut c_void,
    length: u32,
) {
    // No panic may cross the native callback boundary. A failed callback leaves the worker's
    // absolute deadline in force; callbacks never wait, perform I/O, or drive the next request.
    let _contained = std::panic::catch_unwind(|| {
        let state = context as *mut Callback;
        if state.is_null() {
            return;
        }
        if status == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
            // SAFETY: this is the request's final callback. Its successful context registration
            // transferred this Box to WinHTTP, and no worker uses it after closing the handle.
            drop(unsafe { Box::from_raw(state) });
            return;
        }
        let event = match status {
            WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
            | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
            | WINHTTP_CALLBACK_STATUS_READ_COMPLETE => Ok((status, length)),
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
                if !information.is_null()
                    && length as usize >= size_of::<WINHTTP_ASYNC_RESULT>() =>
            {
                // SAFETY: this notification supplies a WINHTTP_ASYNC_RESULT for this callback.
                Err(unsafe { information.cast::<WINHTTP_ASYNC_RESULT>().read_unaligned() }.dwError)
            }
            _ => return,
        };
        // SAFETY: the sender is immutable, thread-safe, and live until the final callback above.
        // One operation is outstanding at a time. After cancellation the receiver may be gone.
        let _delivered = unsafe { &(*state).sender }.try_send(event);
    });
}

struct Request {
    handle: Internet,
    context: *mut Callback,
    receiver: Receiver<Completion>,
}
impl Request {
    fn new(handle: Internet) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut context = Box::new(Callback {
            sender,
            buffer: UnsafeCell::new([0; BUFFER_SIZE]),
        });
        let notifications = WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
            | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
            | WINHTTP_CALLBACK_STATUS_READ_COMPLETE
            | WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
            | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;
        // SAFETY: this new request has no callback or outstanding operation. The callback is
        // static and its context will remain allocated until the final HANDLE_CLOSING callback.
        let previous =
            unsafe { WinHttpSetStatusCallback(handle.0, Some(completed), notifications, 0) };
        if previous.is_some_and(|callback| callback as usize == usize::MAX) {
            return Err(windows::core::Error::from_thread().to_string());
        }
        let address = (&raw mut *context) as usize;
        // SAFETY: the pointer-sized context is copied by WinHTTP. No operation has started, and
        // a successful call binds ownership of the Box to the request's final closing callback.
        unsafe {
            WinHttpSetOption(
                Some(handle.0),
                WINHTTP_OPTION_CONTEXT_VALUE,
                Some(&address.to_ne_bytes()),
            )
        }
        .map_err(|error| error.to_string())?;
        Ok(Self {
            handle,
            context: Box::into_raw(context),
            receiver,
        })
    }

    fn wait(&self, expected: u32, deadline: Instant) -> Result<u32, String> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(TIMED_OUT)?;
        let event = self
            .receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => TIMED_OUT.to_owned(),
                mpsc::RecvTimeoutError::Disconnected => {
                    "The update connection stopped unexpectedly".to_owned()
                }
            })?;
        match event {
            Ok((status, length)) if status == expected => Ok(length),
            Ok(_) => Err("The update connection reported an unexpected completion".to_owned()),
            Err(error) => Err(windows::core::Error::from_hresult(
                windows::core::HRESULT::from_win32(error),
            )
            .to_string()),
        }
    }

    fn read(&self, deadline: Instant, limit: u64, output: &mut dyn Write) -> Result<(), String> {
        if Instant::now() >= deadline {
            return Err(TIMED_OUT.to_owned());
        }
        let headers = "Accept: application/vnd.github+json\r\n"
            .encode_utf16()
            .collect::<Vec<_>>();
        // SAFETY: the async request has a stable context and no body buffer. WinHTTP copies
        // additional headers during this call; context stays live even on cancellation.
        unsafe {
            WinHttpSendRequest(
                self.handle.0,
                Some(&headers),
                None,
                0,
                0,
                self.context as usize,
            )
        }
        .map_err(|error| error.to_string())?;
        self.wait(WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, deadline)?;
        // SAFETY: send completion was received. Async calls require a null reserved pointer.
        unsafe { WinHttpReceiveResponse(self.handle.0, std::ptr::null_mut()) }
            .map_err(|error| error.to_string())?;
        self.wait(WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, deadline)?;
        let mut status = 0_u32;
        let mut length = 4;
        // SAFETY: headers are available and the numeric status output has the declared capacity.
        unsafe {
            WinHttpQueryHeaders(
                self.handle.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&raw mut status).cast()),
                &raw mut length,
                std::ptr::null_mut(),
            )
        }
        .map_err(|error| error.to_string())?;
        if status != 200 {
            return Err(format!("GitHub update request failed with HTTP {status}"));
        }
        let mut total = 0_u64;
        loop {
            if Instant::now() >= deadline {
                return Err(TIMED_OUT.to_owned());
            }
            // SAFETY: this request owns the context until HANDLE_CLOSING, including after a
            // timeout. No previous read is outstanding. The async byte-count pointer is null.
            unsafe {
                WinHttpReadData(
                    self.handle.0,
                    (*self.context).buffer.get().cast(),
                    BUFFER_BYTES,
                    std::ptr::null_mut(),
                )
            }
            .map_err(|error| error.to_string())?;
            let read = self.wait(WINHTTP_CALLBACK_STATUS_READ_COMPLETE, deadline)? as usize;
            if read == 0 {
                return Ok(());
            }
            total += read as u64;
            if total > limit || read > BUFFER_SIZE {
                return Err("The update response exceeds its size limit".to_owned());
            }
            // SAFETY: read completion synchronized with the worker through the channel. The
            // buffer has the checked capacity and WinHTTP won't write until the next read.
            let bytes = unsafe {
                std::slice::from_raw_parts((*self.context).buffer.get().cast::<u8>(), read)
            };
            output.write_all(bytes).map_err(|error| error.to_string())?;
        }
    }
}

pub(super) fn get(
    url: &str,
    limit: u64,
    timeout: Duration,
    output: &mut dyn Write,
) -> Result<(), String> {
    let url = url
        .strip_prefix("https://")
        .ok_or("Updates require HTTPS")?;
    let (host, path) = url.split_once('/').ok_or("Invalid update URL")?;
    if !["api.github.com", "github.com"].contains(&host) {
        return Err("Untrusted update host".to_owned());
    }
    let deadline = Instant::now() + timeout;
    let host = wide(host);
    let path = wide(&format!("/{path}"));
    // SAFETY: strings are terminated and borrowed for these setup calls. WinHttpConnect only
    // stores the server; DNS, connection, redirects, and reads run asynchronously after send.
    let session = Internet::new(unsafe {
        WinHttpOpen(
            w!("AltTabio"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            WINHTTP_FLAG_ASYNC,
        )
    })?;
    // SAFETY: session is live; these per-operation bounds supplement the absolute deadline.
    unsafe { WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000) }
        .map_err(|error| error.to_string())?;
    // SAFETY: the session outlives its connection, and the connection outlives its request.
    let connection =
        Internet::new(unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0) })?;
    // SAFETY: all strings are terminated; no request operation has started yet.
    let handle = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    for (option, value) in [
        (
            WINHTTP_OPTION_REDIRECT_POLICY,
            WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP,
        ),
        (
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        ),
    ] {
        // SAFETY: handle is live and the DWORD option value is copied during the call.
        unsafe { WinHttpSetOption(Some(handle.0), option, Some(&value.to_ne_bytes())) }
            .map_err(|error| error.to_string())?;
    }
    // On deadline or error this async request closes first, cancelling pending work. The
    // callback-owned buffer survives until WinHTTP finishes closing it, without another wait.
    let request = Request::new(handle)?;
    request.read(deadline, limit, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_OPEN_REQUEST_FLAGS,
    };

    struct LocalRequest {
        request: Request,
        _connection: Internet,
        _session: Internet,
    }
    impl LocalRequest {
        fn new(port: u16) -> Self {
            // SAFETY: test-only HTTP uses an explicitly bound loopback port with no proxy.
            // Each child handle is declared before its parent in the owning fixture below.
            unsafe {
                let session = Internet::new(WinHttpOpen(
                    w!("AltTabio-test"),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    WINHTTP_FLAG_ASYNC,
                ))
                .unwrap_or_else(|error| panic!("{error}"));
                let connection = Internet::new(WinHttpConnect(session.0, w!("127.0.0.1"), port, 0))
                    .unwrap_or_else(|error| panic!("{error}"));
                let handle = Internet::new(WinHttpOpenRequest(
                    connection.0,
                    w!("GET"),
                    w!("/"),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_OPEN_REQUEST_FLAGS::default(),
                ))
                .unwrap_or_else(|error| panic!("{error}"));
                Self {
                    request: Request::new(handle).unwrap_or_else(|error| panic!("{error}")),
                    _connection: connection,
                    _session: session,
                }
            }
        }
    }

    fn read_request(stream: &mut TcpStream) {
        stream
            .set_nonblocking(false)
            .unwrap_or_else(|error| panic!("{error}"));
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap_or_else(|error| panic!("{error}"));
        let mut request = Vec::new();
        let mut byte = [0];
        while request.len() < 16 * 1024 && !request.ends_with(b"\r\n\r\n") {
            stream
                .read_exact(&mut byte)
                .unwrap_or_else(|error| panic!("{error}"));
            request.push(byte[0]);
        }
        assert!(request.ends_with(b"\r\n\r\n"));
    }

    #[test]
    fn deadline_cancels_stalled_headers_redirects_and_body_reads() {
        for phase in ["headers", "redirect", "body"] {
            let listener =
                TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|error| panic!("{error}"));
            let port = listener
                .local_addr()
                .unwrap_or_else(|error| panic!("{error}"))
                .port();
            listener
                .set_nonblocking(true)
                .unwrap_or_else(|error| panic!("{error}"));
            let (release, stop) = mpsc::channel();
            let (reached, at_stall) = mpsc::channel();
            let server = std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(3);
                let mut redirected = false;
                loop {
                    if stop.try_recv().is_ok() || Instant::now() >= until {
                        return;
                    }
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    read_request(&mut stream);
                    if phase == "redirect" && !redirected {
                        let response = format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        stream
                            .write_all(response.as_bytes())
                            .unwrap_or_else(|error| panic!("{error}"));
                        redirected = true;
                        continue;
                    }
                    if phase == "body" {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n")
                            .unwrap_or_else(|error| panic!("{error}"));
                    }
                    reached.send(()).unwrap_or_else(|error| panic!("{error}"));
                    let _released = stop.recv_timeout(Duration::from_secs(3));
                    return;
                }
            });
            let client = LocalRequest::new(port);
            let started = Instant::now();
            let result =
                client
                    .request
                    .read(started + Duration::from_millis(250), 100, &mut Vec::new());
            // Closing an async request must return without waiting for the stalled server.
            drop(client);
            let elapsed = started.elapsed();
            let reached = at_stall.recv_timeout(Duration::from_secs(1));
            let _released = release.send(());
            server
                .join()
                .unwrap_or_else(|error| std::panic::resume_unwind(error));
            assert!(reached.is_ok(), "fixture did not reach {phase}");
            assert!(result.is_err_and(|error| error == TIMED_OUT), "{phase}");
            assert!(elapsed < Duration::from_secs(1), "{phase}: {elapsed:?}");
        }
    }

    #[test]
    fn native_async_reads_complete_responses_and_enforces_size_limits() {
        for limit in [2, 3] {
            let listener =
                TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|error| panic!("{error}"));
            let port = listener
                .local_addr()
                .unwrap_or_else(|error| panic!("{error}"))
                .port();
            listener
                .set_nonblocking(true)
                .unwrap_or_else(|error| panic!("{error}"));
            let server = std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < until =>
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                read_request(&mut stream);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
                    )
                    .unwrap_or_else(|error| panic!("{error}"));
            });
            let client = LocalRequest::new(port);
            let mut output = Vec::new();
            let result =
                client
                    .request
                    .read(Instant::now() + Duration::from_secs(2), limit, &mut output);
            drop(client);
            server
                .join()
                .unwrap_or_else(|error| std::panic::resume_unwind(error));
            assert_eq!(result.is_ok(), limit == 3);
            assert_eq!(output, if limit == 3 { b"abc".as_slice() } else { b"" });
        }
    }
}

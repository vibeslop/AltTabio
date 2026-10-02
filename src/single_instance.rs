use std::mem::size_of;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    CreateWellKnownSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser, WinHighLabelSid,
};
use windows::Win32::System::Threading::{
    AddIntegrityLabelToBoundaryDescriptor, AddSIDToBoundaryDescriptor, ClosePrivateNamespace,
    CreateBoundaryDescriptorW, CreateMutexW, CreatePrivateNamespaceW, DeleteBoundaryDescriptor,
    GetCurrentProcess, OpenPrivateNamespaceW, OpenProcessToken, ReleaseMutex,
};
use windows::core::{Result, w};

pub struct SingleInstance {
    handle: HANDLE,
    _namespace: Namespace,
}

impl SingleInstance {
    pub fn acquire() -> Result<Option<Self>> {
        let namespace = Namespace::open()?;
        let handle = unsafe {
            // SAFETY: no security descriptor is supplied and the static UTF-16 name is valid for
            // the synchronous call. The returned handle is uniquely owned by this guard.
            CreateMutexW(None, true, w!("AltTabio.Private\\SingleInstance"))
        }?;
        let already_exists = unsafe {
            // SAFETY: CreateMutexW has just returned successfully, so its last-error value still
            // reports whether the named object existed before this call.
            GetLastError()
        } == ERROR_ALREADY_EXISTS;
        if already_exists {
            close_handle(handle, "duplicate-instance mutex");
            Ok(None)
        } else {
            Ok(Some(Self {
                handle,
                _namespace: namespace,
            }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let release_result = unsafe {
            // SAFETY: this guard acquired initial ownership from CreateMutexW and releases it once
            // on the same process before closing the handle.
            ReleaseMutex(self.handle)
        };
        if let Err(error) = release_result {
            eprintln!("Could not release the single-instance mutex: {error}");
        }
        close_handle(self.handle, "single-instance mutex");
    }
}

fn close_handle(handle: HANDLE, description: &str) {
    let result = unsafe {
        // SAFETY: the caller transfers one uniquely owned kernel handle for exactly one close.
        CloseHandle(handle)
    };
    if let Err(error) = result {
        eprintln!("Could not close the {description}: {error}");
    }
}

/// The boundary includes the caller's user and high integrity. A medium-integrity
/// process cannot enter or precreate this namespace, even if it knows the names.
struct Namespace {
    handle: HANDLE,
    created: bool,
}

struct Boundary(HANDLE);
impl Drop for Boundary {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the descriptor, including additions to it.
        unsafe {
            DeleteBoundaryDescriptor(self.0);
        }
    }
}

struct Token(HANDLE);
impl Drop for Token {
    fn drop(&mut self) {
        close_handle(self.0, "namespace token");
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: conversion allocated the uniquely owned descriptor with LocalAlloc.
        if !unsafe { LocalFree(Some(HLOCAL(self.0.0))) }.is_invalid() {
            eprintln!("Could not free the single-instance security descriptor");
        }
    }
}

impl Namespace {
    fn open() -> Result<Self> {
        unsafe {
            // SAFETY: names are static terminated UTF-16. Every owned handle/descriptor
            // has a guard; aligned SID buffers stay live until the descriptor copies them.
            let mut boundary = Boundary(CreateBoundaryDescriptorW(w!("AltTabio.Private"), 0));
            if boundary.0.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token)?;
            let token = Token(token);
            let mut user = [0_usize; 128];
            let mut length = 0;
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(user.as_mut_ptr().cast()),
                u32::try_from(size_of_val(&user)).unwrap_or_default(),
                &raw mut length,
            )?;
            let user = &*user.as_ptr().cast::<TOKEN_USER>();
            AddSIDToBoundaryDescriptor(&raw mut boundary.0, user.User.Sid)?;
            let mut high = [0_usize; 16];
            let mut length = u32::try_from(high.len() * size_of::<usize>()).unwrap_or_default();
            let sid = PSID(high.as_mut_ptr().cast());
            CreateWellKnownSid(WinHighLabelSid, None, Some(sid), &raw mut length)?;
            AddIntegrityLabelToBoundaryDescriptor(&raw mut boundary.0, sid)?;
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;HI)"),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )?;
            let descriptor = Descriptor(descriptor);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or_default(),
                lpSecurityDescriptor: descriptor.0.0,
                bInheritHandle: false.into(),
            };
            let handle = CreatePrivateNamespaceW(
                Some(&raw const attributes),
                boundary.0.0,
                w!("AltTabio.Private"),
            );
            if !handle.is_invalid() {
                return Ok(Self {
                    handle,
                    created: true,
                });
            }
            if GetLastError() != ERROR_ALREADY_EXISTS {
                return Err(windows::core::Error::from_thread());
            }
            let handle = OpenPrivateNamespaceW(boundary.0.0, w!("AltTabio.Private"));
            if handle.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            Ok(Self {
                handle,
                created: false,
            })
        }
    }
}

impl Drop for Namespace {
    fn drop(&mut self) {
        // SAFETY: this guard closes one private-namespace handle. Only its creator
        // destroys the namespace; duplicate-instance probes close their own reference.
        if !unsafe { ClosePrivateNamespace(self.handle, u32::from(self.created)) } {
            eprintln!(
                "Could not close the single-instance namespace: {}",
                windows::core::Error::from_thread()
            );
        }
    }
}
